pub mod home;
pub mod room;

pub use home::{Home, get_cookie, set_cookie};
pub use room::RoomPage;

use leptos::prelude::*;

#[component]
pub fn NotFound() -> impl IntoView {
    view! {
        <article>
            <h2>"404 — Not Found"</h2>
            <p>"That page does not exist."</p>
            <a href="/">"Go home"</a>
        </article>
    }
}
