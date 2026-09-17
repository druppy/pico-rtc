use gloo_net::http::Request;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use web_sys::{EventSource, MessageEvent};

use crate::types::{JoinRequest, JoinResponse, SseEvent};

/// Room connection status
#[derive(Debug, Clone, PartialEq)]
pub enum RoomStatus {
    Idle,
    Joining,
    NeedPassword,
    PasswordRequired,
    Connected,
    Error,
}

/// POST /api/room/:id/join
pub async fn join_room(room_id: &str, req: JoinRequest) -> Result<JoinResponse, String> {
    let url = format!("/api/room/{room_id}/join");
    let resp = Request::post(&url)
        .header("Content-Type", "application/json")
        .json(&req)
        .map_err(|e| e.to_string())?
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if !resp.ok() {
        return Err(format!("HTTP {}", resp.status()));
    }

    resp.json::<JoinResponse>().await.map_err(|e| e.to_string())
}

/// POST /api/room/:id/signal?session_id=...
pub async fn send_signal(
    room_id: &str,
    session_id: &str,
    signal: &crate::types::SignalMessage,
) -> Result<(), String> {
    let url = format!("/api/room/{room_id}/signal?session_id={session_id}");
    let resp = Request::post(&url)
        .header("Content-Type", "application/json")
        .json(signal)
        .map_err(|e| e.to_string())?
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if !resp.ok() {
        return Err(format!("signal send failed: HTTP {}", resp.status()));
    }
    Ok(())
}

/// Open an EventSource SSE stream for receiving events from the server.
pub fn open_sse_stream(room_id: &str, session_id: &str) -> Result<EventSource, String> {
    let url = format!("/api/room/{room_id}/events?session_id={session_id}");
    let source = EventSource::new(&url).map_err(|e| format!("{e:?}"))?;

    let onmessage = Closure::wrap(Box::new(move |ev: MessageEvent| {
        let data = ev.data().as_string().unwrap_or_default();
        if let Ok(event) = serde_json::from_str::<SseEvent>(&data) {
            crate::services::webrtc::dispatch_sse_event(event);
        }
    }) as Box<dyn FnMut(MessageEvent)>);

    source.set_onmessage(Some(onmessage.as_ref().unchecked_ref()));
    onmessage.forget();

    Ok(source)
}

/// POST /api/room/:id/chat?session_id=... — send a chat message
pub async fn send_chat(
    room_id: &str,
    session_id: &str,
    text: &str,
    sender_name: Option<&str>,
) -> Result<(), String> {
    let url = format!("/api/room/{room_id}/chat?session_id={session_id}");
    let body = serde_json::json!({
        "text": text,
        "sender_name": sender_name,
    });
    let resp = gloo_net::http::Request::post(&url)
        .header("Content-Type", "application/json")
        .json(&body)
        .map_err(|e| e.to_string())?
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if !resp.ok() {
        return Err(format!("chat send failed: HTTP {}", resp.status()));
    }
    Ok(())
}
