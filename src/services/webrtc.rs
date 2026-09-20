// WebRTC peer connection management.
// This module is a scaffold — the full peer connection wiring requires
// careful async handling of RTCPeerConnection state machines.
// The signaling transport (SSE + POST) is fully working; incoming SSE events
// are handed to a callback owned by the room component (see
// `services::signaling::SseStream::open`), which is where peer connections
// will be driven from once they exist.

use wasm_bindgen::prelude::*;

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
