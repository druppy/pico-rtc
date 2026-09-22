use leptos::prelude::*;

/// The bar under the stage: media toggles, the stage layout switch, and the chat
/// overlay toggle carrying whatever has gone unread.
///
/// The mic, camera and share buttons only move their own state for now — the mesh
/// does not renegotiate tracks yet. They are here because a call UI without them
/// reads as broken, not because they work.
#[component]
pub fn Controls(
    /// `true` shows every feed at equal size, `false` gives the large tile to
    /// whoever is talking. Ignored on small screens, which are always a gallery.
    gallery: RwSignal<bool>,
    /// The chat overlay is a window the user opens, not part of the page flow.
    chat_open: RwSignal<bool>,
    /// Messages that arrived while the overlay was closed.
    unread: RwSignal<u32>,
) -> impl IntoView {
    let mic_on = RwSignal::new(true);
    let cam_on = RwSignal::new(true);
    let sharing = RwSignal::new(false);

    // Opening the overlay is what acknowledges its backlog, so the count is
    // cleared on the way in rather than on the way out.
    let toggle_chat = move |_| {
        chat_open.update(|open| *open = !*open);
        if chat_open.get() {
            unread.set(0);
        }
    };

    let badge = move || {
        let n = unread.get();
        (n > 0).then(move || view! { <span class="chat-badge" aria-hidden="true">{n}</span> })
    };

    view! {
        <nav class="room-controls" aria-label="Call controls">
            <button
                type="button"
                on:click=move |_| mic_on.update(|v| *v = !*v)
                aria-pressed=move || mic_on.get()
                title="Toggle microphone">

                {move || if mic_on.get() { "🎤" } else { "🔇" }}
            </button>
            <button
                type="button"
                on:click=move |_| cam_on.update(|v| *v = !*v)
                aria-pressed=move || cam_on.get()
                title="Toggle camera">

                {move || if cam_on.get() { "📷" } else { "🚫" }}
            </button>
            <button
                type="button"
                on:click=move |_| sharing.update(|v| *v = !*v)
                aria-pressed=move || sharing.get()
                title="Share screen">

                {move || if sharing.get() { "Stop" } else { "Share" }}
            </button>
            <button
                type="button"
                class="stage-mode"
                aria-pressed=move || gallery.get()
                title="Switch between the one who speaks and a gallery of everyone"
                on:click=move |_| gallery.update(|v| *v = !*v)>

                {move || if gallery.get() { "Gallery" } else { "Speaker" }}
            </button>
            <button
                type="button"
                class="chat-toggle"
                aria-controls="chat-overlay"
                aria-expanded=move || chat_open.get()
                aria-label=move || format!("Chat, {} unread", unread.get())
                on:click=toggle_chat>

                "Chat"
                {badge}
            </button>
        </nav>
    }
}
