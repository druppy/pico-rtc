use leptos::prelude::*;

#[component]
pub fn Controls() -> impl IntoView {
    let mic_on = RwSignal::new(true);
    let cam_on = RwSignal::new(true);
    let sharing = RwSignal::new(false);

    view! {
        <nav class="room-controls" aria-label="Call controls">
            <button
                on:click=move |_| mic_on.update(|v| *v = !*v)
                aria-pressed=move || mic_on.get()
                title="Toggle microphone"
            >
                {move || if mic_on.get() { "🎤" } else { "🔇" }}
            </button>
            <button
                on:click=move |_| cam_on.update(|v| *v = !*v)
                aria-pressed=move || cam_on.get()
                title="Toggle camera"
            >
                {move || if cam_on.get() { "📷" } else { "🚫" }}
            </button>
            <button
                on:click=move |_| sharing.update(|v| *v = !*v)
                aria-pressed=move || sharing.get()
                title="Share screen"
            >
                {move || if sharing.get() { "Stop" } else { "Share" }}
            </button>
            <a href="/" role="button" class="contrast">"Leave"</a>
        </nav>
    }
}
