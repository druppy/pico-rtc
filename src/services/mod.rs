pub mod signaling;
pub mod webrtc;

/// Get or create a stable session ID (survives page reload via sessionStorage).
pub fn get_or_create_session_id() -> String {
    let storage = web_sys::window().and_then(|w| w.session_storage().ok().flatten());

    if let Some(store) = storage {
        if let Ok(Some(id)) = store.get_item("session_id") {
            if !id.is_empty() {
                return id;
            }
        }
        let id = uuid::Uuid::new_v4().to_string();
        let _ = store.set_item("session_id", &id);
        return id;
    }
    // Fallback (shouldn't happen in browser)
    uuid::Uuid::new_v4().to_string()
}
