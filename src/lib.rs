// ─── Shared Types (compile everywhere) ───────────────────────────────────────

pub mod types;

#[cfg(not(target_arch = "wasm32"))]
pub mod server;

#[cfg(target_arch = "wasm32")]
pub mod app;
#[cfg(target_arch = "wasm32")]
pub mod components;
#[cfg(target_arch = "wasm32")]
pub mod pages;
#[cfg(target_arch = "wasm32")]
pub mod services;

pub use types::*;

/// Client entry point for cargo-leptos.
///
/// The HTML shell rendered by `src/bin/server.rs` includes
/// `leptos::hydration::HydrationScripts`, whose generated script performs
/// `mod.default({ module_or_path })` (the wasm-bindgen init) and then
/// `mod.hydrate()`. The name is fixed by leptos' `hydration_script.js`, so it
/// cannot be renamed despite this being plain CSR with nothing to hydrate.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen::prelude::wasm_bindgen]
pub fn hydrate() {
    use leptos::mount::mount_to_body;

    mount_to_body(app::App);
}
