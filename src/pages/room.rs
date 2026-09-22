use std::time::Duration;

use leptos::prelude::*;
use leptos_router::hooks::use_params_map;
use wasm_bindgen::JsValue;
use wasm_bindgen_futures::spawn_local;

use crate::components::{Controls, VideoTile};
use crate::pages::home::{get_cookie, set_cookie};
use crate::services::session_id;
use crate::services::signaling::{RoomStatus, SseStream, join_room, send_chat};
use crate::services::webrtc::{Mesh, get_local_media};
use crate::types::{ChatMessage, JoinRequest, JoinResponse, PeerInfo, SseEvent};

/// A chat message as displayed in the UI
#[derive(Debug, Clone, PartialEq)]
struct UiChatMsg {
    from: String,
    sender_name: String,
    text: String,
    /// Local HH:MM, for display
    time: String,
    /// Server timestamp, part of the list key (with the index)
    timestamp_ms: u64,
    own: bool,
}

/// Everything the event handler may write about chat, bundled because the three
/// move together: the list, whether anyone is looking at it, and what arrived
/// while nobody was.
#[derive(Clone, Copy)]
struct ChatUi {
    messages: RwSignal<Vec<UiChatMsg>>,
    open: RwSignal<bool>,
    unread: RwSignal<u32>,
}

/// How to authenticate a join attempt. The server distinguishes "room does not
/// exist, claim it" from "room is locked, unlock it", so the UI can ask for the
/// right thing without guessing.
#[derive(Debug, Clone)]
enum Creds {
    /// No password (works for a room that has none, and is how we discover
    /// whether a claim or a verification is needed).
    Anonymous,
    /// Claim a brand-new room under this password.
    Claim(String),
    /// Unlock an existing room with this password.
    Verify(String),
}

/// The storage [`StoredValue::new_local`] picks. The mesh holds JS handles, which
/// are not `Send`, so it cannot go in the default (synchronised) storage.
type LocalStore<T> = StoredValue<T, leptos::reactive::owner::LocalStorage>;

/// How often the stage asks the meter who is loudest. Fast enough that a change of
/// speaker lands before you finish noticing the old one; slow enough that the poll
/// is not itself something you can hear.
const LEVEL_POLL_MS: u64 = 350;

/// DOM id of the chat list, so the scroll-to-bottom effect can find it. Must match
/// the `id` on the `<ul>` in the view.
const CHAT_LOG_ID: &str = "chat-log";

#[component]
pub fn RoomPage() -> impl IntoView {
    let params = use_params_map();
    // Tracked read, for the view.
    let room_id = move || params.with(|p| p.get("room_id").unwrap_or_default());
    // Untracked read, for imperative use (join/chat requests don't need to
    // subscribe to the route).
    let current_room = move || params.with_untracked(|p| p.get("room_id").unwrap_or_default());

    // Optional display name from cookie
    let display_name = RwSignal::new(get_cookie("wr_name").unwrap_or_default());
    // Stable user ID (UUID in cookie)
    let user_id = StoredValue::new(get_or_create_user_id());

    let status = RwSignal::new(RoomStatus::Idle);
    // Session id issued by the join; required by every other room endpoint
    // (signal, chat, chat history) as `session_id`.
    let self_id = RwSignal::new(None::<String>);
    let peers = RwSignal::new(Vec::<PeerInfo>::new());
    let chat_messages = RwSignal::new(Vec::<UiChatMsg>::new());
    let chat_input = RwSignal::new(String::new());
    let chat_error = RwSignal::new(None::<String>);
    let password = RwSignal::new(String::new());
    // The last verification attempt was rejected, so the gate can say so instead
    // of silently redrawing the same form.
    let bad_password = RwSignal::new(false);

    // Stage layout: `false` gives the large tile to whoever speaks, `true` shows
    // everyone at once. Small screens ignore this and are always a gallery — see
    // `.stage` in the stylesheet.
    let gallery = RwSignal::new(false);
    // Who the level meter currently hands the large tile to. `None` while the
    // room is quiet, which is exactly when the stage needs a fallback.
    let speaker = RwSignal::new(None::<String>);
    // The chat is an overlay the user opens, not part of the page flow, so it
    // carries its own open flag and its backlog.
    let chat_open = RwSignal::new(false);
    let unread = RwSignal::new(0u32);

    // The feed shown large: whoever the meter picked or, in a quiet room, the
    // first peer. The fallback is not cosmetic — a stage with no owner is a
    // blank tile — and the memo means a re-poll that changes nothing
    // re-renders nothing either.
    let on_stage = Memo::new(move |_| {
        let list = peers.get();
        // The meter is polled, so it can name a peer that left a moment ago; in
        // speaker mode that would hide every feed, hence the membership check.
        speaker
            .get()
            .filter(|who| list.iter().any(|peer| peer.peer_id == *who))
            .or_else(|| list.first().map(|peer| peer.peer_id.clone()))
    });

    // Handle for the poll that keeps `speaker` fresh. Cleared in `on_cleanup`.
    let ticker = StoredValue::new_local(None::<IntervalHandle>);

    // Keeps the chat list pinned to its newest message. Tracked on both the open
    // flag and the list, so it runs when a message arrives *or* when the overlay
    // is opened, and never while the overlay is hidden.
    Effect::new(move || {
        if !chat_open.get() {
            return;
        }
        let _ = chat_messages.get().len();
        // Found by id, the same way the mesh finds its `<video>` elements: this
        // whole branch is rebuilt whenever the join status changes, so a `NodeRef`
        // captured at mount would point at a detached node.
        if let Some(list) = web_sys::window()
            .and_then(|window| window.document())
            .and_then(|document| document.get_element_by_id(CHAT_LOG_ID))
        {
            let bottom = list.scroll_height();
            list.set_scroll_top(bottom);
        }
    });

    // Owned by this component and closed in `on_cleanup` below. `StoredValue`
    // rather than a plain local because the join completes asynchronously.
    let stream = StoredValue::new_local(None::<SseStream>);
    // The peer connections. Not a signal: a `MediaStream` is neither `Send` nor
    // `Sync`, so it cannot live in one, and nothing in the view reads this.
    let mesh = StoredValue::new_local(None::<Mesh>);
    // Room events that arrive while the mesh is still being built (camera + ICE
    // config), replayed into it as soon as it exists.
    let early = StoredValue::new(Vec::<SseEvent>::new());

    // Join the room, then open the event stream with the session id the join
    // issued. Every capture is `Copy`, so this is callable from the mount and
    // from the password form without cloning anything into each handler.
    let connect = move |creds: Creds| {
        // Ignore re-entrant attempts (double submit, or a submit racing mount).
        if matches!(
            status.get_untracked(),
            RoomStatus::Joining | RoomStatus::Connected
        ) {
            return;
        }
        status.set(RoomStatus::Joining);
        bad_password.set(false);

        let room = current_room();
        let sid = session_id();
        let name = display_name.get_untracked();
        let uid = user_id.get_value();
        // Distinguishes "this room is locked" from "you typed the wrong password".
        let rejected = matches!(creds, Creds::Verify(_));
        let (existing, claim_password) = match creds {
            Creds::Anonymous => (None, None),
            Creds::Claim(pw) => (None, Some(pw)),
            Creds::Verify(pw) => (Some(pw), None),
        };

        spawn_local(async move {
            let req = JoinRequest {
                password: existing,
                claim_password,
                session_id: sid,
                display_name: (!name.is_empty()).then_some(name),
                user_id: Some(uid),
            };
            let joined = join_room(&room, req).await;
            // Bail out if the room was left while the request was in flight: the
            // reactive scope is gone, and writing to a disposed signal panics.
            if status.is_disposed() {
                return;
            }
            match joined {
                Err(e) => status.set(RoomStatus::Error(e)),
                Ok(JoinResponse::NeedPassword {}) => status.set(RoomStatus::NeedPassword),
                Ok(JoinResponse::PasswordRequired {}) => {
                    status.set(RoomStatus::PasswordRequired);
                    if rejected {
                        bad_password.set(true);
                    }
                }
                Ok(JoinResponse::Full {}) => status.set(RoomStatus::Full),
                Ok(JoinResponse::Ok { self_id: id, .. }) => {
                    // Peers and chat history are not read from the join response:
                    // the stream's opening `resync` carries both, with names.
                    self_id.set(Some(id.clone()));
                    status.set(RoomStatus::Connected);
                    let handler = sse_handler(
                        peers,
                        ChatUi {
                            messages: chat_messages,
                            open: chat_open,
                            unread,
                        },
                        status,
                        self_id,
                        mesh,
                        early,
                    );
                    match SseStream::open(&room, &id, handler) {
                        Ok(s) => stream.set_value(Some(s)),
                        Err(e) => status.set(RoomStatus::Error(e)),
                    }

                    // Media and ICE configuration come after the stream is open on
                    // purpose: until the mesh exists the handler buffers what the
                    // room sends, so an offer that lands during the camera prompt is
                    // replayed rather than dropped.
                    let local = match get_local_media().await {
                        Ok(stream) => Some(stream),
                        Err(e) => {
                            web_sys::console::warn_1(&format!("joining without media: {e}").into());
                            None
                        }
                    };
                    let mut built = Mesh::new(&room, &id, local).await;
                    if status.is_disposed() {
                        // Left the room while the camera was being set up.
                        built.close_all();
                        return;
                    }
                    let mut queued = Vec::new();
                    early.update_value(|list| std::mem::swap(list, &mut queued));
                    for ev in &queued {
                        built.on_event(ev);
                    }
                    built.attach_pending();
                    mesh.set_value(Some(built));

                    // Levels live in the mesh, not in a signal, so something has to
                    // ask them regularly. Once per visit is enough: a reconnect
                    // rebuilds the mesh but keeps this poll.
                    if ticker.with_value(|slot| slot.is_none()) {
                        match set_interval_with_handle(
                            move || {
                                // A later tick can outlive the component.
                                if speaker.is_disposed() {
                                    return;
                                }
                                let mut who = None;
                                mesh.update_value(|slot| {
                                    if let Some(mesh) = slot {
                                        who = mesh.dominant_speaker();
                                    }
                                });
                                speaker.set(who);
                            },
                            Duration::from_millis(LEVEL_POLL_MS),
                        ) {
                            Ok(handle) => ticker.set_value(Some(handle)),
                            Err(e) => web_sys::console::warn_1(
                                &format!("speaker detection unavailable: {e:?}").into(),
                            ),
                        }
                    }
                }
            }
        });
    };

    // Leaving the room must close the stream. Navigating in an SPA does not
    // unload the page, so an EventSource left open holds one of the six
    // connections the browser allows per host until the tab is closed — and
    // every visit to a room would add another.
    on_cleanup(move || {
        stream.update_value(|slot| {
            if let Some(mut s) = slot.take() {
                s.close();
            }
        });
        // Closing the peer connections also releases the camera: a route change in
        // an SPA does not stop the tracks on its own, and the light would stay on
        // until the tab closed.
        mesh.update_value(|slot| {
            if let Some(mut m) = slot.take() {
                m.close_all();
            }
        });
        // An interval left running keeps its closure, and the mesh it reaches for,
        // alive for the life of the tab.
        ticker.update_value(|slot| {
            if let Some(handle) = slot.take() {
                handle.clear();
            }
        });
    });

    connect(Creds::Anonymous);

    let send_chat_msg = move || {
        let text = chat_input.get_untracked();
        if text.trim().is_empty() {
            return;
        }
        // Chat is gated on being an active participant, so the only usable
        // session id is the one the join issued (not the `wr_uid` cookie).
        let Some(sid) = self_id.get_untracked() else {
            chat_error.set(Some("Not connected to the room yet.".to_string()));
            return;
        };
        chat_input.set(String::new());
        chat_error.set(None);

        let room = current_room();
        let name = {
            let n = display_name.get_untracked();
            if n.is_empty() {
                "Anonymous".to_string()
            } else {
                n
            }
        };
        // No optimistic append: the server broadcasts the message back to the
        // sender too, and that echo is what renders it.
        spawn_local(async move {
            if let Err(e) = send_chat(&room, &sid, &text, Some(&name)).await {
                // Same hazard as the join: the component may be gone by now.
                if !chat_error.is_disposed() {
                    chat_error.set(Some(format!("Message not sent: {e}")));
                }
            }
        });
    };

    // Password gate: claim a new room, or unlock an existing one.
    let gate = move || {
        let state = status.get();
        let claim = state.password_prompt() == Some(true);
        let wrong = state == RoomStatus::PasswordRequired && bad_password.get();
        view! {
            <section class="room-gate" aria-label="Room password">
                <form on:submit=move |ev| {
                    ev.prevent_default();
                    let pw = password.get();
                    if pw.is_empty() {
                        return;
                    }
                    if claim {
                        connect(Creds::Claim(pw));
                    } else {
                        connect(Creds::Verify(pw));
                    }
                }>
                    <label>
                        {if claim {
                            "Create a password for this room"
                        } else {
                            "This room has a password"
                        }}
                        <input
                            type="password"
                            maxlength="128"
                            required=true
                            placeholder="Room password"
                            prop:value=move || password.get()
                            on:input=move |ev| password.set(event_target_value(&ev))
                        />
                        {claim.then(|| {
                            view! {
                                <small>"Share it with the people you invite here."</small>
                            }
                        })}
                    </label>
                    {wrong.then(|| {
                        view! {
                            <p class="error">"That password does not match this room."</p>
                        }
                    })}
                    <button type="submit">{if claim { "Create room" } else { "Join room" }}</button>
                </form>
            </section>
        }
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

        {move || {
            match status.get() {
                RoomStatus::Idle | RoomStatus::Joining => view! {
                    <p class="muted">"Joining room…"</p>
                }
                .into_any(),
                RoomStatus::NeedPassword | RoomStatus::PasswordRequired => gate().into_any(),
                RoomStatus::Full => view! {
                    <section class="room-gate" aria-label="Room full">
                        <p>"This room is full."</p>
                        <a href="/" role="button">"Back to the lobby"</a>
                    </section>
                }
                .into_any(),
                RoomStatus::Error(message) => view! {
                    <section class="room-gate" aria-label="Error">
                        <p class="error">{format!("Could not join the room: {message}")}</p>
                        <button
                            type="button"
                            on:click=move |_| connect(Creds::Anonymous)
                        >
                            "Try again"
                        </button>
                    </section>
                }
                .into_any(),
                RoomStatus::Connected => view! {
                    // One fifth of the width for the self preview, four for the room.
                    // Which remote feed owns that space is `on_stage`'s decision; the
                    // mode class on `.stage` decides whether there is only one.
                    <section
                        class="stage"
                        class:is-speaker=move || !gallery.get()
                        class:is-gallery=move || gallery.get()
                        aria-label="Video feeds"
                    >
                        <VideoTile id="local" label="You" is_local=true/>

                        <div class="feeds">
                            <For
                                each=move || peers.get()
                                key=|peer| peer.peer_id.clone()
                                children=move |peer| {
                                    let id = peer.peer_id.clone();
                                    let label = peer_label(&peer);
                                    let is_stage = {
                                        let on_stage = on_stage;
                                        let watched = id.clone();
                                        Signal::derive(move || {
                                            on_stage.get().as_deref() == Some(watched.as_str())
                                        })
                                    };
                                    view! {
                                        <VideoTile
                                            id=id
                                            label=label
                                            is_local=false
                                            is_stage=is_stage
                                        />
                                    }
                                }
                            />
                            {move || {
                                peers
                                    .get()
                                    .is_empty()
                                    .then(|| view! {
                                        <p class="muted stage-empty">
                                            "Nobody else is here yet. Share the room link."
                                        </p>
                                    })
                            }}
                        </div>
                    </section>

                    <Controls gallery=gallery chat_open=chat_open unread=unread/>

                    <aside
                        id="chat-overlay"
                        class="chat-overlay"
                        class:is-open=move || chat_open.get()
                        aria-label="Chat"
                    >
                        <header class="chat-overlay-head">
                            <h3>"Chat"</h3>
                            <button
                                type="button"
                                class="chat-close"
                                aria-label="Close chat"
                                on:click=move |_| chat_open.set(false)
                            >
                                "Close"
                            </button>
                        </header>
                        <ul id=CHAT_LOG_ID class="chat-messages" aria-live="polite">
                            <For
                                // Keyed by index *and* content identity: a `resync`
                                // replaces the whole list, and rows whose key survived
                                // keep their DOM while changed rows are rebuilt.
                                each=move || {
                                    chat_messages
                                        .get()
                                        .into_iter()
                                        .enumerate()
                                        .collect::<Vec<_>>()
                                }
                                key=|(i, msg)| (*i, msg.from.clone(), msg.timestamp_ms)
                                children=move |(_, msg)| {
                                    view! {
                                        <li class="chat-msg" class:own=msg.own>
                                            <strong>{msg.sender_name}</strong>
                                            <span>{msg.text}</span>
                                            <time>{msg.time}</time>
                                        </li>
                                    }
                                }
                            />
                        </ul>
                        <form on:submit=move |ev| {
                            ev.prevent_default();
                            send_chat_msg();
                        }>
                            <input
                                type="text"
                                placeholder="Type a message..."
                                prop:value=move || chat_input.get()
                                on:input=move |ev| chat_input.set(event_target_value(&ev))
                            />
                            <button type="submit">"Send"</button>
                        </form>
                        {move || {
                            chat_error
                                .get()
                                .map(|e| view! { <small class="error">{e}</small> })
                        }}
                    </aside>
                }
                .into_any(),
            }
        }}
    }
}

/// Translates the room's SSE events into UI state, and hands every event to the
/// mesh (or to its buffer, while the mesh is still being built).
fn sse_handler(
    peers: RwSignal<Vec<PeerInfo>>,
    chat: ChatUi,
    status: RwSignal<RoomStatus>,
    self_id: RwSignal<Option<String>>,
    mesh: LocalStore<Option<Mesh>>,
    early: StoredValue<Vec<SseEvent>>,
) -> impl FnMut(SseEvent) {
    move |ev| {
        // Media and presence go to the mesh first: it decides for itself which
        // events concern it, and it has to see them even where the UI has nothing
        // to do (an offer for a peer whose tile is already rendered).
        match &ev {
            SseEvent::Resync { .. }
            | SseEvent::PeerJoined { .. }
            | SseEvent::PeerLeft { .. }
            | SseEvent::Offer { .. }
            | SseEvent::Answer { .. }
            | SseEvent::IceCandidate { .. } => {
                if mesh.with_value(|slot| slot.is_some()) {
                    mesh.update_value(|slot| {
                        if let Some(mesh) = slot {
                            mesh.on_event(&ev);
                        }
                    });
                } else {
                    early.update_value(|list| list.push(ev.clone()));
                }
            }
            SseEvent::ChatMessage { .. } | SseEvent::RoomFull | SseEvent::Error { .. } => {}
        }

        // UI state: presence and chat. The media events are the mesh's business,
        // taken care of above.
        match ev {
            // The authoritative snapshot: sent on every (re)connect, so it replaces
            // whatever the UI had.
            SseEvent::Resync {
                peers: list,
                chat: history,
            } => {
                peers.set(list);
                let mine = self_id.get_untracked();
                chat.messages.set(
                    history
                        .into_iter()
                        .map(|m| to_ui(m, mine.as_deref()))
                        .collect(),
                );
            }
            SseEvent::PeerJoined { peer_id, peer_name } => peers.update(|list| {
                if !list.iter().any(|p| p.peer_id == peer_id) {
                    list.push(PeerInfo { peer_id, peer_name });
                }
            }),
            SseEvent::PeerLeft { peer_id } => {
                peers.update(|list| list.retain(|p| p.peer_id != peer_id));
            }
            SseEvent::ChatMessage {
                from,
                sender_name,
                text,
                timestamp_ms,
            } => {
                let mine = self_id.get_untracked();
                let own = mine.as_deref() == Some(from.as_str());
                // Your own echo is not news, and anything that lands while the
                // overlay is shut is exactly what the badge is for.
                if !own && !chat.open.get_untracked() {
                    chat.unread.update(|count| *count += 1);
                }
                chat.messages.update(|list| {
                    list.push(UiChatMsg {
                        sender_name: sender_name.unwrap_or_else(|| "Anonymous".to_string()),
                        time: format_time(timestamp_ms),
                        from,
                        text,
                        timestamp_ms,
                        own,
                    })
                });
            }
            SseEvent::RoomFull => status.set(RoomStatus::Full),
            SseEvent::Error { message } => status.set(RoomStatus::Error(message)),
            SseEvent::Offer { .. } | SseEvent::Answer { .. } | SseEvent::IceCandidate { .. } => {}
        }
    }
}

fn to_ui(msg: ChatMessage, mine: Option<&str>) -> UiChatMsg {
    let own = mine == Some(msg.sender_id.as_str());
    UiChatMsg {
        time: format_time(msg.timestamp_ms),
        from: msg.sender_id,
        sender_name: msg.sender_name.unwrap_or_else(|| "Anonymous".to_string()),
        text: msg.text,
        timestamp_ms: msg.timestamp_ms,
        own,
    }
}

/// Local time of day (HH:MM) for a server timestamp in ms since the epoch.
/// A bare `HH:MM` is a valid HTML time string, so `<time>` can render it.
fn format_time(timestamp_ms: u64) -> String {
    let date = js_sys::Date::new(&JsValue::from_f64(timestamp_ms as f64));
    format!("{:02}:{:02}", date.get_hours(), date.get_minutes())
}

/// Display name for a peer, falling back to a short form of the session id.
fn peer_label(peer: &PeerInfo) -> String {
    peer.peer_name.clone().unwrap_or_else(|| {
        let short: String = peer.peer_id.chars().take(4).collect();
        format!("Guest {short}")
    })
}

/// Get or generate a stable user ID, stored in a cookie.
fn get_or_create_user_id() -> String {
    if let Some(id) = get_cookie("wr_uid").filter(|id| id.len() >= 8) {
        return id;
    }
    let id = uuid::Uuid::new_v4().to_string();
    set_cookie("wr_uid", &id);
    id
}
