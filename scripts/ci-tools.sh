#!/bin/sh
# Installs the three build tools cargo-leptos needs but does not ship, from
# their published release binaries, into one directory.
#
#   scripts/ci-tools.sh [BIN_DIR]      # default: $HOME/.local/bin
#
# Versions are pinned here, and the pins are not arbitrary:
#   CARGO_LEPTOS   the 0.3.x line this project is configured against; a minor
#                  bump can change the site layout and the LEPTOS_* env it exports.
#   WASM_BINDGEN   must equal the `=`-pinned wasm-bindgen crate in Cargo.toml —
#                  a mismatch fails the wasm link, so an exact match is required.
#   DART_SASS      >= 1.79: the vendored Pico uses color.channel() and
#                  map.deep-merge(). Without this, cargo-leptos downloads its own
#                  Dart Sass, which unpins the Sass version from build to build.
#
# Installing binaries rather than `cargo install`-ing them is also deliberate:
# an unpinned `cargo install cargo-leptos` resolves the newest 0.3.x, whose
# dependency tree has its own MSRV and can outrun the Rust we build with. Static
# musl builds sidestep that, and they run on any glibc or musl host.
#
# A tool already on PATH is kept only if it satisfies the pin, so a developer
# with a suitable `sass` installed is not reshuffled underneath them.

set -eu

BIN_DIR="${1:-$HOME/.local/bin}"
CARGO_LEPTOS=0.3.5
WASM_BINDGEN=0.2.128
DART_SASS=1.86.0

# Release assets are per-arch, and the three projects disagree on spelling.
case "$(uname -m)" in
x86_64 | amd64) CL_TRIPLE=x86_64-unknown-linux-musl
    WB_TRIPLE=x86_64-unknown-linux-musl
    SASS_ARCH=linux-x64 ;;
aarch64 | arm64) CL_TRIPLE=aarch64-unknown-linux-musl
    WB_TRIPLE=aarch64-unknown-linux-musl
    SASS_ARCH=linux-arm64 ;;
*) printf 'unsupported architecture: %s\n' "$(uname -m)" >&2; exit 1 ;;
esac

mkdir -p "$BIN_DIR"
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT HUP INT TERM

# satisfies <binary> <exact-version|-">  -> 0 if the installed version works
satisfies() {
    bin=$1
    want=$2
    command -v "$bin" >/dev/null 2>&1 || return 1
    [ "$want" = "-" ] && return 0
    "$bin" --version 2>&1 | head -1 | grep -q "$want"
}

# fetch <name> <url> <exact-version|"-
fetch() {
    name=$1
    url=$2
    want=$3
    if satisfies "$name" "$want"; then
        printf '%-14s already on PATH: %s\n' "$name" "$(command -v "$name")"
        return
    fi
    printf '%-14s installing from %s\n' "$name" "$url"
    curl -sSLf --retry 3 -o "$tmp/$name.tar.gz" "$url"
    tar xzf "$tmp/$name.tar.gz" -C "$tmp"
    # Each tarball holds its binaries in one top-level directory, so the binary
    # is at depth 2 and named exactly after the tool.
    inner=$(find "$tmp" -mindepth 2 -maxdepth 2 -name "$name" -type f | head -1)
    if [ -z "$inner" ]; then
        printf 'no %s binary inside %s\n' "$name" "$url" >&2
        exit 1
    fi
    install -m 0755 "$inner" "$BIN_DIR/$name"
    printf '  -> %s\n' "$("$BIN_DIR/$name" --version 2>&1 | head -1)"
    if [ "$want" != "-" ]; then
        "$BIN_DIR/$name" --version 2>&1 | head -1 | grep -q "$want" ||
            printf '  warning: %s does not report %s\n' "$name" "$want"
    fi
}

# Same, but for a tool that is not a standalone binary. Dart Sass ships a `sass`
# shell stub that execs `src/dart` and `src/sass.snapshot` relative to its own
# directory, so copying the stub alone produces a binary that cannot start.
# The whole directory contents have to land together.
fetch_dir() {
    name=$1
    url=$2
    subdir=$3
    if satisfies "$name" "-"; then
        printf '%-14s already on PATH: %s\n' "$name" "$(command -v "$name")"
        return
    fi
    printf '%-14s installing from %s\n' "$name" "$url"
    curl -sSLf --retry 3 -o "$tmp/$name.tar.gz" "$url"
    tar xzf "$tmp/$name.tar.gz" -C "$tmp"
    [ -d "$tmp/$subdir" ] || {
        printf 'no %s directory inside %s\n' "$subdir" "$url" >&2
        exit 1
    }
    cp -R "$tmp/$subdir/." "$BIN_DIR/"
    chmod 0755 "$BIN_DIR/$name"
    printf '  -> %s\n' "$("$BIN_DIR/$name" --version 2>&1 | head -1)"
}

fetch cargo-leptos \
    "https://github.com/leptos-rs/cargo-leptos/releases/download/v${CARGO_LEPTOS}/cargo-leptos-${CL_TRIPLE}.tar.gz" \
    "$CARGO_LEPTOS"

fetch wasm-bindgen \
    "https://github.com/rustwasm/wasm-bindgen/releases/download/${WASM_BINDGEN}/wasm-bindgen-${WASM_BINDGEN}-${WB_TRIPLE}.tar.gz" \
    "$WASM_BINDGEN"

fetch_dir sass \
    "https://github.com/sass/dart-sass/releases/download/${DART_SASS}/dart-sass-${DART_SASS}-${SASS_ARCH}.tar.gz" \
    "dart-sass"

printf '\ntools used:\n'
for t in cargo-leptos wasm-bindgen sass; do
    # Either we just installed it, or the PATH one satisfied the pin.
    if [ -x "$BIN_DIR/$t" ]; then
        printf '  %-14s %s (%s)\n' "$t" "$BIN_DIR/$t" "$("$BIN_DIR/$t" --version 2>&1 | head -1)"
    else
        printf '  %-14s %s (PATH)\n' "$t" "$(command -v "$t")"
    fi
done
