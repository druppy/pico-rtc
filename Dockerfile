# Build stage
FROM rust:1.85-slim AS builder
RUN rustup target add wasm32-unknown-unknown
RUN cargo install trunk

WORKDIR /build
COPY . .

# Build client (Trunk → dist/)
RUN trunk build --release

# Build server binary
RUN cargo build --release --bin server --features ssr

# Runtime stage
FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*

COPY --from=builder /build/target/release/server /usr/local/bin/webrtc-room
COPY --from=builder /build/dist /srv/dist

WORKDIR /srv
EXPOSE 3000

CMD ["webrtc-room"]
