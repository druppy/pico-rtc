pub mod auth;
pub mod chat;
pub mod rooms;
pub mod turn;

use axum::{
    Router,
    extract::{Path, Query, State, Json},
    http::StatusCode,
    response::{IntoResponse, Sse, sse::Event},
    routing::{get, post},
};
use dashmap::DashMap;
use std::convert::Infallible;
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio_stream::wrappers::UnboundedReceiverStream;

use crate::types::{
    ChatSendRequest, JoinRequest, JoinResponse, SignalMessage, SseEvent, TurnCredentials,
};
use self::chat::{ChatMessage, ChatStore};
use self::rooms::RoomState;

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
) -> Json<JoinResponse> {
    if !is_valid_room_name(&room_id) {
        return Json(JoinResponse::Full {});
    }
    if let Some(ref pw) = req.claim_password {
        if pw.len() > 128 {
            return Json(JoinResponse::Full {});
        }
    }

    let is_new = !state.rooms.contains_key(&room_id);

    if is_new {
        match req.claim_password {
            Some(ref pw) if !pw.is_empty() => {
                state.rooms.insert(room_id.clone(), RoomState::new(Some(pw.clone())));
            }
            _ => return Json(JoinResponse::NeedPassword {}),
        }
    }

    let Some(mut room) = state.rooms.get_mut(&room_id) else {
        return Json(JoinResponse::NeedPassword {});
    };

    // Auth check
    match auth::check_password(
        &room.password,
        req.password.as_deref(),
        req.claim_password.as_deref(),
        is_new,
    ) {
        auth::AuthResult::Granted => {}
        auth::AuthResult::NeedPassword => return Json(JoinResponse::NeedPassword {}),
        auth::AuthResult::PasswordRequired => return Json(JoinResponse::PasswordRequired {}),
    }

    // Capacity
    if room.participants.len() >= state.max_participants {
        return Json(JoinResponse::Full {});
    }

    let self_id = req.session_id.clone();
    let peers: Vec<String> = room.participants.iter().map(|e| e.key().clone()).collect();

    // Store display name for this participant
    if let Some(ref name) = req.display_name {
        let name = name.chars().take(64).collect::<String>();
        room.display_names.insert(self_id.clone(), name);
    }

    // Notify existing peers about the new joiner
    let event = SseEvent::PeerJoined {
        peer_id: self_id.clone(),
        peer_name: req.display_name.clone(),
    };
    room.broadcast(&event, None);

    // Create SSE channel: tx goes to room, rx parked for SSE endpoint
    let (tx, rx) = mpsc::unbounded_channel::<Result<Event, Infallible>>();
    room.participants.insert(self_id.clone(), tx);
    room.pending_receivers.push(rx);

    // Include chat history in response
    let chat_history = state.chat.history(&room_id, 50);

    Json(JoinResponse::Ok {
        self_id,
        peers,
        chat: chat_history,
    })
}

// ─── SSE Handler ─────────────────────────────────────────────────────────────

async fn handle_sse(
    State(state): State<Arc<AppState>>,
    Path(room_id): Path<String>,
    Query(_params): Query<std::collections::HashMap<String, String>>,
) -> impl IntoResponse {
    let rx = {
        match state.rooms.get_mut(&room_id) {
            Some(mut room) => room.pending_receivers.pop(),
            None => None,
        }
    };

    let stream = match rx {
        Some(rx) => UnboundedReceiverStream::new(rx),
        None => {
            let (_tx, rx) = mpsc::unbounded_channel::<Result<Event, Infallible>>();
            UnboundedReceiverStream::new(rx)
        }
    };

    Sse::new(stream).keep_alive(axum::response::sse::KeepAlive::new())
}

// ─── Signal Relay ────────────────────────────────────────────────────────────

async fn handle_signal(
    State(state): State<Arc<AppState>>,
    Path(room_id): Path<String>,
    Query(params): Query<std::collections::HashMap<String, String>>,
    Json(signal): Json<SignalMessage>,
) -> StatusCode {
    let from = params.get("session_id").cloned().unwrap_or_default();

    let Some(room) = state.rooms.get(&room_id) else {
        return StatusCode::NOT_FOUND;
    };

    let event = match signal {
        SignalMessage::Offer { sdp } => SseEvent::Offer {
            from: from.clone(),
            sdp,
        },
        SignalMessage::Answer { sdp } => SseEvent::Answer {
            from: from.clone(),
            sdp,
        },
        SignalMessage::IceCandidate {
            candidate,
            sdp_mid,
            sdp_mline_index,
        } => SseEvent::IceCandidate {
            from: from.clone(),
            candidate,
            sdp_mid,
            sdp_mline_index,
        },
        SignalMessage::Renegotiate => return StatusCode::NOT_IMPLEMENTED,
    };

    room.broadcast(&event, Some(&from));
    StatusCode::OK
}

// ─── Chat ────────────────────────────────────────────────────────────────────

async fn handle_chat_send(
    State(state): State<Arc<AppState>>,
    Path(room_id): Path<String>,
    Query(params): Query<std::collections::HashMap<String, String>>,
    Json(body): Json<ChatSendRequest>,
) -> StatusCode {
    let sender_id = params.get("session_id").cloned().unwrap_or_default();

    let Some(room) = state.rooms.get(&room_id) else {
        return StatusCode::NOT_FOUND;
    };

    let text: String = body.text.chars().take(2000).collect();
    if text.is_empty() {
        return StatusCode::BAD_REQUEST;
    }

    let msg = ChatMessage {
        room: room_id.clone(),
        sender_id: sender_id.clone(),
        sender_name: body.sender_name.clone(),
        text: text.clone(),
        timestamp_ms: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64,
    };

    // Persist (in-memory now, DB later)
    state.chat.append(&msg);

    // Broadcast to all participants
    let event = SseEvent::ChatMessage {
        from: sender_id,
        sender_name: msg.sender_name,
        text: msg.text,
        timestamp_ms: msg.timestamp_ms,
    };
    room.broadcast(&event, None);

    StatusCode::OK
}

// ─── TURN Credentials ────────────────────────────────────────────────────────

async fn handle_chat_history(
    State(state): State<Arc<AppState>>,
    Path(room_id): Path<String>,
) -> Json<Vec<ChatMessage>> {
    Json(state.chat.history(&room_id, 100))
}

async fn handle_turn(State(state): State<Arc<AppState>>) -> Json<TurnCredentials> {
    let creds = turn::generate_credentials(
        &state.turn_secret,
        &state.turn_host,
        state.turn_port,
        3600,
    );
    Json(creds)
}

// ─── Helpers ─────────────────────────────────────────────────────────────────

fn is_valid_room_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() >= 3
        && name.len() <= 48
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}
