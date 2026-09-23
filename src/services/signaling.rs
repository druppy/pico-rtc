use gloo_net::http::Request;
use wasm_bindgen::prelude::*;
use web_sys::{EventSource, MessageEvent};

use crate::types::{JoinRequest, JoinResponse, SseEvent};

/// Room connection status
#[derive(Debug, Clone, PartialEq)]
pub enum RoomStatus {
    /// Nothing attempted yet (or a previous attempt was abandoned).
    Idle,
    Joining,
    /// Room does not exist yet; the caller must supply a password to claim it.
    NeedPassword,
    /// Room exists and is locked; the password was missing or wrong.
    PasswordRequired,
    /// Room is at capacity.
    Full,
    Connected,
    /// Transport/protocol failure, with a message for the UI.
    Error(String),
}

impl RoomStatus {
    /// `Some(true)` when the user should *set* a new room password,
    /// `Some(false)` when they should *enter* an existing one, `None` otherwise.
    pub fn password_prompt(&self) -> Option<bool> {
        match self {
            RoomStatus::NeedPassword => Some(true),
            RoomStatus::PasswordRequired => Some(false),
            _ => None,
        }
    }
}

/// A live SSE subscription to `GET /api/room/:id/events`.
///
/// Owns both the browser's `EventSource` and the `onmessage` `Closure` so the
/// connection can be torn down for real. Dropping an `EventSource` does *not*
/// close its HTTP connection, and a `forget()`-ten `Closure` is leaked forever;
/// in an SPA, leaving a room is a client-side route change, so an unowned stream
/// costs a permanent socket per visit until the browser's 6-connections-per-host
/// cap stalls the tab. Callers should `close()` it from `on_cleanup`.
pub struct SseStream {
    source: EventSource,
    on_message: Option<Closure<dyn FnMut(MessageEvent)>>,
}

impl SseStream {
    /// Opens the room's event stream, parsing every SSE `data:` payload as
    /// [`SseEvent`] and handing it to `on_event`.
    pub fn open<F>(room_id: &str, session_id: &str, mut on_event: F) -> Result<Self, String>
    where
        F: FnMut(SseEvent) + 'static,
    {
        let url = format!("/api/room/{room_id}/events?session_id={session_id}");
        let source = EventSource::new(&url).map_err(|e| format!("EventSource: {e:?}"))?;

        let on_message = Closure::wrap(Box::new(move |ev: MessageEvent| {
            let data = ev.data().as_string().unwrap_or_default();
            match serde_json::from_str::<SseEvent>(&data) {
                Ok(event) => on_event(event),
                // Unknown/rotated payloads must not kill the stream silently.
                Err(e) => web_sys::console::warn_1(
                    &format!("ignoring unparseable SSE event ({e}): {data}").into(),
                ),
            }
        }) as Box<dyn FnMut(MessageEvent)>);

        source.set_onmessage(Some(on_message.as_ref().unchecked_ref()));
        Ok(Self {
            source,
            on_message: Some(on_message),
        })
    }

    /// Closes the connection and releases the message handler.
    pub fn close(&mut self) {
        // Detach the handler before dropping the closure: JS must not be able to
        // call it afterwards.
        self.source.set_onmessage(None);
        self.source.close();
        self.on_message = None;
    }
}

impl Drop for SseStream {
    fn drop(&mut self) {
        self.close();
    }
}

/// POST /api/room/:id/join
pub async fn join_room(room_id: &str, req: JoinRequest) -> Result<JoinResponse, String> {
    post_json(&format!("/api/room/{room_id}/join"), &req)
        .await?
        .json::<JoinResponse>()
        .await
        .map_err(|e| e.to_string())
}

/// POST /api/room/:id/signal?session_id=...
pub async fn send_signal(
    room_id: &str,
    session_id: &str,
    signal: &crate::types::SignalMessage,
) -> Result<(), String> {
    post_json(
        &format!("/api/room/{room_id}/signal?session_id={session_id}"),
        signal,
    )
    .await?;
    Ok(())
}

/// POST /api/room/:id/chat?session_id=...
pub async fn send_chat(
    room_id: &str,
    session_id: &str,
    text: &str,
    sender_name: Option<&str>,
) -> Result<(), String> {
    let body = crate::types::ChatSendRequest {
        text: text.to_string(),
        sender_name: sender_name.map(str::to_string),
    };
    post_json(
        &format!("/api/room/{room_id}/chat?session_id={session_id}"),
        &body,
    )
    .await?;
    Ok(())
}

async fn post_json<T: serde::Serialize>(
    url: &str,
    body: &T,
) -> Result<gloo_net::http::Response, String> {
    let resp = Request::post(url)
        .header("Content-Type", "application/json")
        .json(body)
        .map_err(|e| e.to_string())?
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !resp.ok() {
        return Err(format!("HTTP {}", resp.status()));
    }
    Ok(resp)
}
