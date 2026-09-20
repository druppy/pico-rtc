use std::path::PathBuf;
use std::sync::Arc;

use axum::Router;
use leptos::prelude::*;
use tower_http::services::{ServeDir, ServeFile};
use tracing_subscriber::EnvFilter;
use webrtc_room::server::{AppState, build_router, chat::InMemoryChat};

/// The HTML shell, written to `<site_root>/index.html` at startup.
///
/// cargo-leptos does not generate HTML for a CSR project, so the server owns it:
/// - `HydrationScripts` emits the `/pkg/*.js` module import plus the `hydrate()`
///   call, and resolves the wasm file name for us (it is `_bg`-suffixed only when
///   the build did not come from cargo-leptos).
/// - `AutoReload` is cargo-leptos' dev-only refresh hook, not application
///   transport. It emits a script only when `LEPTOS_WATCH` is set (i.e. under
///   `cargo leptos watch`), and that script dials cargo-leptos' own reload server
///   on `reload-port` rather than this app; production output contains nothing.
///   It has no SSE variant (`reload-ws-protocol` accepts only ws/wss), so it
///   cannot be converted to SSE. Server to client push for the product itself is
///   exclusively SSE at `/api/room/:id/events`.
///
/// The CSS link below assumes `hash-files = false` (the default). Enabling
/// hashing would suffix that file name with its content hash.
fn shell(options: LeptosOptions) -> impl IntoView {
    let css = format!("/{}/{}.css", options.site_pkg_dir, options.output_name);

    view! {
        <!DOCTYPE html>
        <html lang="en" data-theme="auto">
            <head>
                <meta charset="utf-8"/>
                <meta name="viewport" content="width=device-width, initial-scale=1"/>
                <title>Pico RTC Room</title>
                <link
                    rel="stylesheet"
                    href="https://cdn.jsdelivr.net/npm/@picocss/pico@2/css/pico.min.css"
                />
                <link rel="stylesheet" href=css/>
                <AutoReload options=options.clone()/>
                <HydrationScripts options/>
            </head>
            <body></body>
        </html>
    }
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();

    // cargo-leptos exports the LEPTOS_* variables when it builds and runs us.
    // A standalone run falls back to the same defaults it would configure
    // (target/site, 127.0.0.1:3000).
    let leptos = get_configuration(None)
        .expect("could not read Leptos configuration")
        .leptos_options;

    // CSR has no server-rendered page, so materialise the shell where the
    // static handler below will find it. site_root is wiped on every rebuild,
    // which is why this is written at startup rather than committed.
    let index_path = PathBuf::from(&*leptos.site_root).join("index.html");
    if let Some(dir) = index_path.parent() {
        std::fs::create_dir_all(dir).expect("could not create site root");
    }
    std::fs::write(&index_path, shell(leptos.clone()).to_html())
        .expect("could not write index.html");

    let turn_secret = match std::env::var("TURN_SECRET") {
        Ok(v) => v,
        Err(_) => {
            tracing::warn!(
                "TURN_SECRET not set — using insecure dev default; set it for production"
            );
            "dev-secret-change-me".into()
        }
    };
    let turn_host = std::env::var("TURN_HOST").unwrap_or_else(|_| "localhost".into());
    let turn_port: u16 = std::env::var("TURN_PORT")
        .unwrap_or_else(|_| "3478".into())
        .parse()
        .unwrap_or(3478);

    let state = Arc::new(AppState {
        rooms: dashmap::DashMap::new(),
        chat: Box::new(InMemoryChat::new()),
        turn_secret,
        turn_host,
        turn_port,
        max_participants: 4,
    });

    // API routes first, then the built site with an SPA fallback.
    let api = build_router(state);
    let statics = ServeDir::new(&*leptos.site_root).fallback(ServeFile::new(index_path.clone()));

    let app = Router::new().nest("/api", api).fallback_service(statics);

    let addr = leptos.site_addr;
    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    tracing::info!("Listening on http://{addr}");

    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            tokio::signal::ctrl_c().await.ok();
            tracing::info!("Shutting down...");
        })
        .await
        .unwrap();
}
