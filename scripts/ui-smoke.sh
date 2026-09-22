#!/bin/sh
# Launches headless Chromium with fake media devices, runs the browser suite
# (scripts/ui.test.mjs) against it, and shuts the browser down again.
#
#   scripts/ui-smoke.sh [peers]              # default 3
#
#   BASE_URL   where the app server listens   (default http://127.0.0.1:3000)
#   BROWSER    which browser binary to use    (default: first one found)
#   PEERS      number of tabs                 (default 3)
#
# The fake-device flags are what makes this possible at all: without them the
# page asks for camera permission, gets no frames, and every media assertion
# fails. --no-sandbox is for containers, where the sandbox cannot unshare.
#
# Two reporters at once: `spec` for whoever is watching, JUnit into reports/ for
# CI to publish. Node's test runner does this natively, which is why there is no
# npm project in this repository.

set -eu

BASE_URL="${BASE_URL:-http://127.0.0.1:3000}"
CDP_PORT="${CDP_PORT:-9333}"
CDP_URL="http://127.0.0.1:$CDP_PORT"
PEERS="${1:-${PEERS:-3}}"

# Runner images ship Google Chrome; developer machines usually have Chromium.
find_browser() {
    if [ -n "${BROWSER:-}" ]; then
        command -v "$BROWSER" || { printf 'BROWSER=%s not found\n' "$BROWSER" >&2; exit 1; }
        return
    fi
    for candidate in chromium chromium-browser google-chrome google-chrome-stable; do
        if command -v "$candidate" >/dev/null 2>&1; then
            command -v "$candidate"
            return
        fi
    done
    printf 'no Chromium/Chrome found — set BROWSER=/path/to/browser\n' >&2
    exit 1
}

browser=$(find_browser)
profile=$(mktemp -d)
chrome_log=$(mktemp)
trap 'rm -rf "$profile" "$chrome_log"' EXIT HUP INT TERM

printf 'browser: %s\n' "$browser"
"$browser" \
    --headless=new \
    --remote-debugging-port="$CDP_PORT" \
    --user-data-dir="$profile" \
    --use-fake-ui-for-media-stream \
    --use-fake-device-for-media-stream \
    --autoplay-policy=no-user-gesture-required \
    --disable-gpu \
    --disable-dev-shm-usage \
    --no-sandbox \
    about:blank >"$chrome_log" 2>&1 &
chrome_pid=$!

cleanup() {
    kill "$chrome_pid" 2>/dev/null || true
    wait "$chrome_pid" 2>/dev/null || true
}
trap 'cleanup; rm -rf "$profile" "$chrome_log"' EXIT HUP INT TERM

# The DevTools endpoint takes a moment to answer; polling beats a fixed sleep.
i=0
until curl -sf "$CDP_URL/json/version" >/dev/null 2>&1; do
    i=$((i + 1))
    [ "$i" -gt 30 ] && { printf 'Chromium never answered on %s:\n%s\n' "$CDP_URL" "$(cat "$chrome_log")" >&2; exit 1; }
    sleep 1
done

status=0
mkdir -p reports
BASE_URL="$BASE_URL" CDP_URL="$CDP_URL" PEERS="$PEERS" node --test \
    --test-reporter=spec --test-reporter-destination=stdout \
    --test-reporter=junit --test-reporter-destination=reports/ui-junit.xml \
    scripts/ui.test.mjs || status=$?
cleanup
exit "$status"
