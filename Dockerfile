# Build stage
# rust-toolchain.toml pins `channel = "stable"` — keep this tag in sync with it.
FROM rust:1.90-slim AS builder
RUN rustup target add wasm32-unknown-unknown

# cargo-leptos drives the wasm build, and needs a wasm-bindgen-cli whose version
# matches the `wasm-bindgen` crate pinned in Cargo.toml. Keep these in sync:
# a schema mismatch fails the front-end build outright.
RUN cargo install cargo-leptos \
 && cargo install -f --locked wasm-bindgen-cli --version 0.2.128

WORKDIR /build
COPY . .

# Builds the wasm client and the server binary; static output lands in target/site
RUN cargo leptos build --release

# Runtime stage
FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*

COPY --from=builder /build/target/release/webrtc-room /usr/local/bin/webrtc-room
COPY --from=builder /build/target/site /srv/site

WORKDIR /srv
# The server reads these on startup; the defaults are dev-oriented (loopback bind,
# target/site), so a container must override them.
ENV LEPTOS_SITE_ROOT=/srv/site \
    LEPTOS_SITE_ADDR=0.0.0.0:3000 \
    LEPTOS_OUTPUT_NAME=webrtc-room
EXPOSE 3000

CMD ["webrtc-room"]
