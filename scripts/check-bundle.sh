#!/bin/sh
# Assertions about what `cargo leptos build` produced. Run after the build,
# before serving: every check here fails for a reason that would otherwise only
# show up as a blank page or a leaked connection in a browser.
#
#   scripts/check-bundle.sh [SITE_ROOT]     # default: target/site

set -eu

SITE="${1:-target/site}"
PKG="$SITE/pkg"
NAME=webrtc-room
JS="$PKG/$NAME.js"
WASM="$PKG/$NAME.wasm"
CSS="$PKG/$NAME.css"

fail() {
    printf 'FAIL: %s\n' "$1" >&2
    exit 1
}
ok() {
    printf 'ok      - %s\n' "$1"
}

[ -d "$PKG" ] || fail "$PKG missing — did 'cargo leptos build' run in this directory?"

# ---------------------------------------------------------------- artifacts --
for f in "$JS" "$WASM" "$CSS"; do
    [ -f "$f" ] || fail "$f not produced"
done
ok "client bundle: js, wasm and css all exist"

wasm_bytes=$(wc -c <"$WASM" | tr -d ' ')
[ "$wasm_bytes" -gt 100000 ] || fail "$WASM is only $wasm_bytes bytes"
ok "wasm is a real artifact ($wasm_bytes bytes)"

# A Sass failure that is swallowed somewhere would leave a stylesheet that is
# either tiny or has no Pico in it, and the room would render unstyled.
css_bytes=$(wc -c <"$CSS" | tr -d ' ')
[ "$css_bytes" -gt 30000 ] ||
    fail "$CSS is only $css_bytes bytes — Pico probably did not compile in"
# Counting occurrences, not lines: a release build minifies the whole sheet onto
# one line, so `grep -c` would report 1 no matter how much Pico is in there.
pico_refs=$(grep -o -- '--pico-' "$CSS" | wc -l | tr -d ' ')
[ "$pico_refs" -gt 400 ] ||
    fail "only $pico_refs --pico-* references in the CSS; expected the vendored Pico"
ok "css carries the vendored Pico ($css_bytes bytes, $pico_refs --pico-* refs)"

for own in video-tile stage chat-overlay; do
    grep -q -- "$own" "$CSS" || fail "no rule for .$own in the compiled css"
done
ok "room styles compiled into the same stylesheet"

# ------------------------------------------------------- SSE-only invariant --
# The project rule is SSE for every server→client push, which is also what keeps
# the per-client socket count bounded. cargo-leptos' dev live-reload socket is
# not part of the output (it is emitted only under LEPTOS_WATCH, on loopback),
# so anything matching here is a genuine regression, not the build tool.
#
# Matching on the scheme rather than the word: a bare `ws:` also occurs inside
# ordinary words like "rows:" and "windows:".
hits=$(grep -a -o -E 'wss?://' "$JS" "$WASM" 2>/dev/null | wc -l | tr -d ' ')
[ "$hits" = 0 ] || fail "$hits ws:// or wss:// literal(s) in the client bundle"

hits=$(grep -a -o 'WebSocket' "$JS" "$WASM" 2>/dev/null | wc -l | tr -d ' ')
[ "$hits" = 0 ] || fail "WebSocket mentioned $hits time(s) in the client bundle"

if ! grep -qa 'EventSource' "$JS"; then
    fail "no EventSource in the client — the SSE consumer seems to be gone"
fi
ok "no WebSocket/ws:// in the bundle; SSE (EventSource) present"

# No third-party asset should be reachable from the built client either.
if grep -aq -E 'cdn\.jsdelivr\.net|unpkg\.com|cdnjs\.cloudflare\.com' "$JS" "$CSS"; then
    fail "a public CDN is referenced from the built client"
fi
ok "no CDN references in the built client"

printf '\ncheck-bundle: %s looks as expected\n' "$PKG"
