use leptos::prelude::*;
use leptos_router::hooks::use_params_map;
use crate::pages::home::{get_cookie, set_cookie};

/// A chat message as displayed in the UI
#[derive(Debug, Clone, PartialEq)]
struct UiChatMsg {
    sender_name: String,
    text: String,
    time: String,
}

#[component]
pub fn RoomPage() -> impl IntoView {
    let params = use_params_map();
    let room_id = move || params.with(|p| p.get("room_id").unwrap_or_default());

    // Optional display name from cookie
    let display_name = RwSignal::new(get_cookie("wr_name").unwrap_or_default());
    // Stable user ID (UUID in cookie)
    let user_id = RwSignal::new(get_or_create_user_id());

    // Chat state
    let chat_messages = RwSignal::new(Vec::<UiChatMsg>::new());
    let chat_input = RwSignal::new(String::new());

    let send_chat = move || {
        let text = chat_input.get_untracked();
        if text.is_empty() {
            return;
        }
        chat_input.set(String::new());

        let name = {
            let n = display_name.get_untracked();
            if n.is_empty() { "Anonymous".to_string() } else { n }
        };
        chat_messages.update(|msgs| {
            msgs.push(UiChatMsg {
                sender_name: name.clone(),
                text: text.clone(),
                time: String::new(),
            });
        });

        let room = room_id();
        let uid = user_id.get_untracked();
        wasm_bindgen_futures::spawn_local(async move {
            let _ = crate::services::signaling::send_chat(&room, &uid, &text, Some(&name)).await;
        });
    };

    view! {
        <header>
            <nav>
                <h2>{room_id}</h2>
                <a href="/">"Leave"</a>
            </nav>
        </header>

        // Identity bar
        <section class="identity-bar" aria-label="Your identity">
            <label>
                "Name: "
                <input
                    type="text"
                    placeholder="Your name"
                    maxlength="64"
                    size="16"
                    prop:value=move || display_name.get()
                    on:input=move |ev| {
                        let val = event_target_value(&ev);
                        set_cookie("wr_name", &val);
                        display_name.set(val);
                    }
                />
            </label>
        </section>

        // Video area (placeholder until WebRTC wired)
        <section class="video-grid" aria-label="Video feeds">
            <figure class="video-tile local">
                <video id="local" autoplay=true playsinline=true muted=true></video>
                <figcaption>"You"</figcaption>
            </figure>
        </section>

        // Chat panel
        <aside class="chat-panel" aria-label="Chat">
            <h3>"Chat"</h3>
            <ul class="chat-messages" aria-live="polite">
                <For
                    each=move || {
                        let items: Vec<(usize, UiChatMsg)> = chat_messages
                            .get()
                            .into_iter()
                            .enumerate()
                            .collect();
                        items
                    }
                    key=|(i, _)| *i
                    children=move |(_, msg)| {
                        view! {
                            <li class="chat-msg">
                                <strong>{msg.sender_name}</strong>
                                <span>{msg.text}</span>
                            </li>
                        }
                    }
                />
            </ul>
            <form on:submit=move |ev| { ev.prevent_default(); send_chat(); }>
                <input
                    type="text"
                    placeholder="Type a message..."
                    prop:value=move || chat_input.get()
                    on:input=move |ev| chat_input.set(event_target_value(&ev))
                />
                <button type="submit">"Send"</button>
            </form>
        </aside>
    }
}

/// Get or generate a stable user ID, stored in a cookie.
fn get_or_create_user_id() -> String {
    if let Some(id) = get_cookie("wr_uid") {
        if id.len() >= 8 {
            return id;
        }
    }
    let id = uuid::Uuid::new_v4().to_string();
    set_cookie("wr_uid", &id);
    id
}
