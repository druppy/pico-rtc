pub mod audio_level;
pub mod signaling;
pub mod webrtc;

/// This tab's participant slot: a UUID v4 kept in `sessionStorage`.
///
/// Not a cookie on purpose: a cookie is shared by every tab, but each tab is a
/// separate participant. `sessionStorage` survives a reload (so a refresh
/// re-claims the same slot instead of taking another one from the room's
/// capacity) while a second tab still gets its own id.
pub fn session_id() -> String {
    const KEY: &str = "wr_sid";
    let Some(store) = web_sys::window().and_then(|w| w.session_storage().ok().flatten()) else {
        // Storage can be unavailable (blocked cookies, Safari private mode).
        return uuid::Uuid::new_v4().to_string();
    };

    if let Some(id) = store
        .get_item(KEY)
        .ok()
        .flatten()
        .filter(|id| id.len() >= 8)
    {
        return id;
    }
    let id = uuid::Uuid::new_v4().to_string();
    let _ = store.set_item(KEY, &id);
    id
}
