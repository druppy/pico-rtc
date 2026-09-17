use std::sync::Arc;
use tower_http::services::{ServeDir, ServeFile};
use tracing_subscriber::EnvFilter;
use webrtc_room::server::{build_router, chat::InMemoryChat, AppState};

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    let turn_secret = match std::env::var("TURN_SECRET") {
        Ok(v) => v,
        Err(_) => {
            tracing::warn!("TURN_SECRET not set — using insecure dev default; set it for production");
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

    // API routes first, then static files as SPA fallback.
    // ServeDir handles files that exist in dist/; fallback serves index.html for SPA routes.
    let api = build_router(state.clone());
    let statics = ServeDir::new("dist").fallback(ServeFile::new("dist/index.html"));

    let app = axum::Router::new()
        .nest("/api", api)
        .fallback_service(statics);

    let addr = "0.0.0.0:3000";
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
