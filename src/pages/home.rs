use leptos::prelude::*;
use leptos_router::hooks::use_navigate;
use wasm_bindgen::JsCast;
use web_sys::HtmlDocument;

#[component]
pub fn Home() -> impl IntoView {
    let room_name = RwSignal::new(String::new());
    let display_name = RwSignal::new(get_cookie("wr_name").unwrap_or_default());
    let navigate = use_navigate();

    let on_submit = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        let name = room_name.get().trim().to_lowercase();
        if !name.is_empty() {
            let path = format!("/room/{name}");
            navigate(&path, Default::default());
        }
    };

    view! {
        <header>
            <h1>"WebRTC Room"</h1>
            <p>"Peer-to-peer video chat. No account needed."</p>
        </header>

        <section>
            <form on:submit=on_submit>
                <label>
                    "Room name"
                    <input
                        type="text"
                        placeholder="e.g. weekly-sync"
                        maxlength="48"
                        required=true
                        prop:value=move || room_name.get()
                        on:input=move |ev| room_name.set(event_target_value(&ev))
                    />
                    <small>"Lowercase letters, digits, and hyphens. 3–48 chars."</small>
                </label>

                <label>
                    "Your name "
                    <span class="muted">"(optional, remembered in a cookie)"</span>
                    <input
                        type="text"
                        placeholder="e.g. Alice"
                        maxlength="64"
                        prop:value=move || display_name.get()
                        on:input=move |ev| {
                            let val = event_target_value(&ev);
                            set_cookie("wr_name", &val);
                            display_name.set(val);
                        }
                    />
                </label>

                <button type="submit">"Join Room"</button>
            </form>
        </section>

        <footer>
            <p><small>"Rooms are created on first visit. First person sets the password. Share the link and password to invite others."</small></p>
        </footer>
    }
}

fn html_document() -> Option<HtmlDocument> {
    web_sys::window()?.document()?.dyn_into().ok()
}

/// Read a cookie value by name.
pub fn get_cookie(name: &str) -> Option<String> {
    let cookie_str = html_document()?.cookie().ok()?;
    cookie_str.split(';').find_map(|c| {
        let c = c.trim();
        c.strip_prefix(&format!("{name}=")).map(|v| v.to_string())
    })
}

/// Set a cookie (1 year expiry, same-site lax).
pub fn set_cookie(name: &str, value: &str) {
    if let Some(doc) = html_document() {
        let _ = doc.set_cookie(&format!(
            "{name}={value}; Max-Age=31536000; Path=/; SameSite=Lax"
        ));
    }
}
