// WebRTC peer connection management.
// This module is a scaffold — the full peer connection wiring requires
// careful async handling of RTCPeerConnection state machines.
// The signaling transport (SSE + POST) is fully working; this file
// provides the structure to plug in the RTCPeerConnection logic.

use wasm_bindgen::prelude::*;
use crate::types::SseEvent;

// Global handler registered by the room component on mount.
// When an SSE event arrives, signaling calls dispatch_sse_event which
// forwards here. The room component sets this up.
thread_local! {
    static EVENT_HANDLER: std::cell::RefCell<Option<Box<dyn Fn(SseEvent)>>> = const { std::cell::RefCell::new(None) };
}

/// Register the SSE event handler (called from the room component).
pub fn register_handler<F: Fn(SseEvent) + 'static>(f: F) {
    EVENT_HANDLER.with(|h| *h.borrow_mut() = Some(Box::new(f)));
}

/// Called by the signaling module when an SSE event arrives.
pub fn dispatch_sse_event(event: SseEvent) {
    EVENT_HANDLER.with(|h| {
        if let Some(handler) = &*h.borrow() {
            handler(event);
        }
    });
}

/// Initialize local media (camera + mic).
/// Returns a MediaStream as JsValue (to avoid web-sys type issues in this scaffold).
pub async fn get_local_media() -> Result<JsValue, String> {
    let window = web_sys::window().ok_or("no window")?;
    let navigator = window.navigator();
    let media_devices = navigator.media_devices().map_err(|e| format!("{e:?}"))?;

    let constraints = web_sys::MediaStreamConstraints::new();
    constraints.set_audio(&JsValue::TRUE);
    constraints.set_video(&JsValue::TRUE);

    let promise = media_devices
        .get_user_media_with_constraints(&constraints)
        .map_err(|e| format!("getUserMedia: {e:?}"))?;

    let stream: JsValue = wasm_bindgen_futures::JsFuture::from(promise)
        .await
        .map_err(|e| format!("getUserMedia rejected: {e:?}"))?;

    Ok(stream)
}

/// Fetch TURN credentials from the server.
pub async fn fetch_turn_config() -> Result<crate::types::TurnCredentials, String> {
    let resp = gloo_net::http::Request::get("/api/turn-credentials")
        .send()
        .await
        .map_err(|e| format!("TURN fetch failed: {e}"))?;

    if resp.ok() {
        resp.json::<crate::types::TurnCredentials>()
            .await
            .map_err(|e| e.to_string())
    } else {
        Err(format!("HTTP {}", resp.status()))
    }
}
