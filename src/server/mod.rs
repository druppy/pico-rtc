pub mod auth;
pub mod chat;
pub mod rooms;
pub mod turn;

use axum::{
    Router,
    extract::{Json, Path, Query, State},
    http::StatusCode,
    response::{Sse, sse::Event, sse::KeepAlive},
    routing::{get, post},
};
use dashmap::DashMap;
use dashmap::mapref::entry::Entry;
use futures::{Stream, StreamExt};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::broadcast;
use tokio_stream::wrappers::BroadcastStream;

use self::chat::{ChatMessage, ChatStore};
use self::rooms::RoomState;
use crate::types::{
    ChatSendRequest, JoinRequest, JoinResponse, PeerInfo, SignalMessage, SseEvent, TurnCredentials,
};

/// Error that terminates an SSE stream (client fell behind). The connection is
/// closed; the browser's EventSource auto-reconnects and re-subscribes.
#[derive(Debug)]
struct SseStreamError;

impl std::fmt::Display for SseStreamError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SSE stream ended (client fell behind)")
    }
}

impl std::error::Error for SseStreamError {}

/// Shared application state
pub struct AppState {
    pub rooms: DashMap<String, RoomState>,
    pub chat: Box<dyn ChatStore>,
    pub turn_secret: String,
    pub turn_host: String,
    pub turn_port: u16,
    pub max_participants: usize,
}

pub fn build_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/room/{room_id}/join", post(handle_join))
        .route("/room/{room_id}/signal", post(handle_signal))
        .route("/room/{room_id}/chat", post(handle_chat_send))
        .route("/room/{room_id}/chat/history", get(handle_chat_history))
        .route("/room/{room_id}/events", get(handle_sse))
        .route("/turn-credentials", get(handle_turn))
        .with_state(state)
}

// ─── Join Handler ────────────────────────────────────────────────────────────

async fn handle_join(
    State(state): State<Arc<AppState>>,
    Path(room_id): Path<String>,
    Json(req): Json<JoinRequest>,
) -> Result<Json<JoinResponse>, StatusCode> {
    if !is_valid_room_name(&room_id) {
        return Err(StatusCode::BAD_REQUEST);
    }
    if req.claim_password.as_ref().is_some_and(|pw| pw.len() > 128) {
        return Err(StatusCode::BAD_REQUEST);
    }

    // Atomically claim-or-load the room (no TOCTOU between existence check and insert).
    // `is_new` is true only when *this* call created the room with the claim password.
    let (room, is_new) = match state.rooms.entry(room_id.clone()) {
        Entry::Occupied(o) => (o.into_ref(), false),
        Entry::Vacant(v) => match req.claim_password.as_ref() {
            Some(pw) if !pw.is_empty() => (v.insert(RoomState::new(Some(pw.clone()))), true),
            _ => return Ok(Json(JoinResponse::NeedPassword {})),
        },
    };

    // A brand-new room already passed the claim-password gate above.
    if !is_new
        && auth::check_password(&room.password, req.password.as_deref())
            == auth::AuthResult::PasswordRequired
    {
        return Ok(Json(JoinResponse::PasswordRequired {}));
    }

    // Capacity — re-joining with an existing session does not count against the cap
    let self_id = req.session_id.clone();
    if room.participants.len() >= state.max_participants
        && !room.participants.contains_key(&self_id)
    {
        return Ok(Json(JoinResponse::Full {}));
    }

    let peers: Vec<String> = room.participants.iter().map(|e| e.key().clone()).collect();

    // Display name for this participant (truncated once). Announced from the SSE
    // endpoint, not here: offering before the joiner has a subscribed stream
    // drops the offer and strands the pair.
    if let Some(name) = req
        .display_name
        .as_ref()
        .map(|n| n.chars().take(64).collect::<String>())
    {
        room.display_names.insert(self_id.clone(), name);
    }

    let (tx, _rx) = broadcast::channel(rooms::SSE_BUFFER_SIZE);
    room.participants.insert(self_id.clone(), tx);

    Ok(Json(JoinResponse::Ok { self_id, peers }))
}

// ─── SSE Handler ─────────────────────────────────────────────────────────────

async fn handle_sse(
    State(state): State<Arc<AppState>>,
    Path(room_id): Path<String>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Sse<impl Stream<Item = Result<Event, SseStreamError>> + Send>, StatusCode> {
    // Only active participants may subscribe. The session_id was issued to the
    // client by the join flow, so this is what makes the room password meaningful.
    let session_id = require_session(&params)?;
    let (live, resync) = {
        let room = require_room(&state, &room_id, session_id)?;
        let tx = room
            .participants
            .get(session_id)
            .map(|e| e.clone())
            .ok_or(StatusCode::FORBIDDEN)?;

        // Snapshot of current room state. Sent before the live stream so the
        // client heals any gap (join → SSE connect, or a reconnect). The
        // subscribe() below happens after the snapshot, so events sent in
        // between are delivered live without duplication.
        let peers: Vec<PeerInfo> = room
            .participants
            .iter()
            .filter(|e| e.key() != session_id)
            .map(|e| PeerInfo {
                peer_id: e.key().clone(),
                peer_name: room.display_names.get(e.key()).map(|n| n.value().clone()),
            })
            .collect();
        let chat = state.chat.history(&room_id, 50);

        // Subscribe first, then announce. A peer that reacts to `peer-joined` by
        // offering must find the joiner already listening: `broadcast` drops the
        // event when the target has no subscriber yet, and a lost offer would
        // strand that pair, because the tie-break says only *that* peer may offer.
        // Reconnecting re-announces; clients treat a known peer as a no-op.
        let live = BroadcastStream::new(tx.subscribe());
        room.broadcast(
            &SseEvent::PeerJoined {
                peer_id: session_id.to_string(),
                peer_name: room
                    .display_names
                    .get(session_id)
                    .map(|n| n.value().clone()),
            },
            Some(session_id),
        );
        (live, SseEvent::Resync { peers, chat })
    };

    let resync_event = Event::default().data(serde_json::to_string(&resync).unwrap_or_default());
    let live = live.map(|item| match item {
        Ok(data) => Ok(Event::default().data(data)),
        // Lagged: client fell behind. End the stream — the browser's EventSource
        // auto-reconnects and re-subscribes (getting a fresh resync).
        Err(_) => Err(SseStreamError),
    });
    let stream = futures::stream::iter(vec![Ok(resync_event)]).chain(live);

    Ok(Sse::new(stream).keep_alive(KeepAlive::new()))
}

// ─── Signal Relay ────────────────────────────────────────────────────────────

/// Relays one media signal to the participant it is addressed to.
///
/// Not a room broadcast on purpose: every peer would otherwise see every offer,
/// answer and candidate, and a peer cannot tell a candidate meant for a third party
/// from one meant for it — both arrive from the same sender, whose single
/// `RTCPeerConnection` it then pollutes with unreachable candidates.
async fn handle_signal(
    State(state): State<Arc<AppState>>,
    Path(room_id): Path<String>,
    Query(params): Query<HashMap<String, String>>,
    Json(signal): Json<SignalMessage>,
) -> Result<StatusCode, StatusCode> {
    let from = require_session(&params)?.to_owned();
    let room = require_room(&state, &room_id, &from)?;

    match signal {
        SignalMessage::Offer { to, sdp } => {
            room.send_to(&to, &SseEvent::Offer { from, sdp });
        }
        SignalMessage::Answer { to, sdp } => {
            room.send_to(&to, &SseEvent::Answer { from, sdp });
        }
        SignalMessage::IceCandidate {
            to,
            candidate,
            sdp_mid,
            sdp_mline_index,
        } => {
            room.send_to(
                &to,
                &SseEvent::IceCandidate {
                    from,
                    candidate,
                    sdp_mid,
                    sdp_mline_index,
                },
            );
        }
    }

    // A peer that left mid-signal is not the sender's problem: the pair is rebuilt
    // from the next `resync`, so reporting it as failure would only invite retries.
    Ok(StatusCode::OK)
}

// ─── Chat ────────────────────────────────────────────────────────────────────

async fn handle_chat_send(
    State(state): State<Arc<AppState>>,
    Path(room_id): Path<String>,
    Query(params): Query<HashMap<String, String>>,
    Json(body): Json<ChatSendRequest>,
) -> Result<StatusCode, StatusCode> {
    let sender_id = require_session(&params)?.to_owned();
    let room = require_room(&state, &room_id, &sender_id)?;

    let text: String = body.text.chars().take(2000).collect();
    if text.is_empty() {
        return Err(StatusCode::BAD_REQUEST);
    }

    let timestamp_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;
    let msg = ChatMessage {
        room: room_id.clone(),
        sender_id: sender_id.clone(),
        sender_name: body.sender_name,
        text,
        timestamp_ms,
    };
    state.chat.append(&msg);
    let event = SseEvent::ChatMessage {
        from: sender_id,
        sender_name: msg.sender_name,
        text: msg.text,
        timestamp_ms,
    };
    room.broadcast(&event, None);

    Ok(StatusCode::OK)
}

// ─── TURN Credentials ────────────────────────────────────────────────────────

async fn handle_chat_history(
    State(state): State<Arc<AppState>>,
    Path(room_id): Path<String>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Json<Vec<ChatMessage>>, StatusCode> {
    let session_id = require_session(&params)?;
    let _room = require_room(&state, &room_id, session_id)?;
    Ok(Json(state.chat.history(&room_id, 100)))
}

async fn handle_turn(State(state): State<Arc<AppState>>) -> Json<TurnCredentials> {
    let creds =
        turn::generate_credentials(&state.turn_secret, &state.turn_host, state.turn_port, 3600);
    Json(creds)
}

// ─── Helpers ─────────────────────────────────────────────────────────────────

fn require_session(params: &HashMap<String, String>) -> Result<&str, StatusCode> {
    params
        .get("session_id")
        .map(String::as_str)
        .filter(|s| !s.is_empty())
        .ok_or(StatusCode::BAD_REQUEST)
}

fn require_room<'a>(
    state: &'a AppState,
    room_id: &str,
    session_id: &str,
) -> Result<dashmap::mapref::one::Ref<'a, String, RoomState>, StatusCode> {
    let room = state.rooms.get(room_id).ok_or(StatusCode::NOT_FOUND)?;
    if !room.is_participant(session_id) {
        return Err(StatusCode::FORBIDDEN);
    }
    Ok(room)
}

fn is_valid_room_name(name: &str) -> bool {
    (3..=48).contains(&name.len())
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn room_name_validation() {
        assert!(is_valid_room_name("abc"));
        assert!(is_valid_room_name("weekly-sync-42"));
        assert!(!is_valid_room_name(""));
        assert!(!is_valid_room_name("ab"));
        assert!(!is_valid_room_name(&"a".repeat(49)));
        assert!(!is_valid_room_name("UPPER"));
        assert!(!is_valid_room_name("has_underscore"));
        assert!(!is_valid_room_name("has space"));
    }
}
