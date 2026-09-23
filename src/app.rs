use crate::pages::{Home, NotFound, RoomPage};
use leptos::prelude::*;
use leptos_router::{components::*, path};

#[component]
pub fn App() -> impl IntoView {
    view! {
        <Router>
            <main class="container">
                <Routes fallback=|| view! { <NotFound/> }>
                    <Route path=path!("") view=Home/>
                    <Route path=path!("room/:room_id") view=RoomPage/>
                </Routes>
            </main>
        </Router>
    }
}
