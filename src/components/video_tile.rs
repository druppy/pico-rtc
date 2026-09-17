use leptos::prelude::*;

#[component]
pub fn VideoTile(
    #[prop(into)] id: String,
    #[prop(into)] label: String,
    is_local: bool,
) -> impl IntoView {
    let cls = if is_local { "video-tile local" } else { "video-tile" };

    view! {
        <figure class=cls>
            <video id=id autoplay=true playsinline=true muted=is_local />
            <figcaption>{label}</figcaption>
        </figure>
    }
}
