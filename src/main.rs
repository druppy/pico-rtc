// Default binary. For wasm32 (Trunk), mounts the Leptos CSR app.
// For native, use `cargo build --bin server` instead.

#[cfg(target_arch = "wasm32")]
fn main() {
    use webrtc_room::app::App;
    leptos::mount::mount_to_body(App);
}

#[cfg(not(target_arch = "wasm32"))]
fn main() {
    eprintln!("This binary is for wasm32 only. Use: cargo run --bin server");
    std::process::exit(1);
}
