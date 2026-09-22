# Build stage.
#
# rust-toolchain.toml is the real pin (`channel = "stable"` plus the wasm32
# target); this image only has to be new enough to bootstrap rustup, which then
# installs the toolchain that file names. The tag below is a floor, not the
# version that compiles the project.
FROM rust:1.90-slim AS builder

# Needed to fetch the pinned build tools below; the slim base ships neither.
RUN apt-get update \
 && apt-get install -y --no-install-recommends curl ca-certificates \
 && rm -rf /var/lib/apt/lists/*

WORKDIR /build

# Copied before the source so the toolchain download is its own cached layer:
# re-fetched when the pin changes, not on every source edit. Installing the
# channel also pulls the targets the file declares (wasm32-unknown-unknown).
COPY rust-toolchain.toml .
RUN rustc --version && rustup target list --installed

# cargo-leptos drives the wasm build, wasm-bindgen-cli must match the
# `=`-pinned wasm-bindgen crate, and Dart Sass compiles style/main.scss. All
# three come from scripts/ci-tools.sh, the same script CI uses, so the versions
# live in exactly one place.
#
# `cargo install cargo-leptos` is deliberately not used: unpinned it resolves
# the newest 0.3.x, whose dependency tree now requires a newer rustc than this
# image bootstraps, so the build fails before it starts. Even pinned, it
# compiles ~300 crates here that nothing else needs.
COPY scripts/ci-tools.sh scripts/ci-tools.sh
RUN sh scripts/ci-tools.sh /usr/local/bin

# Builds the wasm client and the server binary; static output lands in target/site.
COPY . .
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
