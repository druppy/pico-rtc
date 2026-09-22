use leptos::prelude::*;

/// One participant's video: the `<video>` that receives its stream, and the name
/// under it.
///
/// The mesh finds this element by `id` (`document.getElementById`), which is why
/// the id is the participant's session id and not something the view invents.
#[component]
pub fn VideoTile(
    #[prop(into)] id: String,
    #[prop(into)] label: String,
    /// The self preview: mirrored, and muted, or the room hears its own mic.
    is_local: bool,
    /// Whether this feed is the one shown large. Reactive because it changes as
    /// someone else starts talking; absent for the local preview, which never
    /// takes the stage.
    #[prop(optional)]
    is_stage: Option<Signal<bool>>,
) -> impl IntoView {
    let on_stage = Signal::derive(move || is_stage.as_ref().is_some_and(Signal::get));

    view! {
        <figure class="video-tile" class:local=is_local class:is-stage=on_stage>
            <video id=id autoplay=true playsinline=true muted=is_local/>
            <figcaption>{label}</figcaption>
        </figure>
    }
}
