#!/bin/sh
# API smoke test: drives the real Axum handlers over HTTP, no browser involved.
#
#   scripts/api-smoke.sh                 # against http://127.0.0.1:3000
#   BASE_URL=http://host:3000 scripts/api-smoke.sh
#
# Exit 0 means every assertion held; the first failure aborts with a message.
# Assumes jq and curl, and a server whose site root holds the built client.
#
# Everything here is room-scoped and uses a fresh room name per run, so it is
# safe against a server that is already serving other rooms.

set -eu

BASE_URL="${BASE_URL:-http://127.0.0.1:3000}"
# The room is claimed by its first joiner, so a reused name would make the
# "need-password" step depend on state left over from an earlier run.
ROOM="${ROOM:-smoke-$(date +%s)-$$}"
API="$BASE_URL/api/room/$ROOM"

# Session ids are opaque to the server (it only requires uniqueness and that
# they look like the ones it issued), so uuidgen is not needed — these just
# have to differ from each other and from the impostor below.
SID_A="11111111-1111-4111-8111-111111111111"
SID_B="22222222-2222-4222-8222-222222222222"
SID_NOBODY="00000000-0000-4000-8000-000000000000"
PASSWORD="let-me-in"
HELLO="hello-from-smoke-$(date +%s)"

pass=0
fail() {
    printf 'FAIL: %s\n' "$1" >&2
    exit 1
}
ok() {
    pass=$((pass + 1))
    printf 'ok %2d - %s\n' "$pass" "$1"
}

body=$(mktemp)
sse_a=$(mktemp)
sse_b=$(mktemp)
trap 'rm -f "$body" "$sse_a" "$sse_b"' EXIT HUP INT TERM

# post <method> <path> [json-body] -> status code on stdout, body in $body
post() {
    if [ "$#" -ge 3 ]; then
        curl -s -o "$body" -w '%{http_code}' -X "$1" "$2" \
            -H 'content-type: application/json' -d "$3"
    else
        curl -s -o "$body" -w '%{http_code}' -X "$1" "$2"
    fi
}

# ---------------------------------------------------------------- liveness ---
code=$(post GET "$BASE_URL/api/turn-credentials")
[ "$code" = 200 ] || fail "GET /api/turn-credentials returned $code"
# Shape per the TURN REST API: username is "<expiry>:<id>", credential an HMAC.
if ! jq -e '
        (.urls | type == "array" and length > 0) and
        (.username | type == "string" and test(":")) and
        (.credential | type == "string" and length > 0) and
        (.ttl_secs | type == "number" and . > 0)
    ' "$body" >/dev/null; then
    fail "turn-credentials payload shape: $(cat "$body")"
fi
ok "turn-credentials serves ephemeral-shaped credentials"

# A page load must work too, or we would be testing an API with no client.
code=$(post GET "$BASE_URL/")
[ "$code" = 200 ] || fail "GET / returned $code — is target/site populated?"
if ! grep -q 'webrtc-room.css' "$body"; then
    fail "index.html does not link the built stylesheet: $(head -c 300 "$body")"
fi
if grep -qi 'jsdelivr\|unpkg\|cdnjs' "$body"; then
    fail "index.html references a CDN — Pico must stay vendored"
fi
ok "index shell serves with the vendored stylesheet and no CDN"

# ------------------------------------------------------ addressed signaling --
# Push and signaling are addressed: both must be refused for a session that
# never joined, or any visitor could inject offers into a room.
code=$(post POST "$API/signal?session_id=$SID_NOBODY" \
    '{"type":"offer","to":"'"$SID_B"'","sdp":"v=0"}')
case $code in
400 | 403 | 404) ok "signal from an unknown session rejected ($code)" ;;
*) fail "signal from unknown session returned $code, expected 400/403/404" ;;
esac

code=$(post GET "$API/events?session_id=$SID_NOBODY")
case $code in
400 | 403 | 404) ok "SSE for an unknown session rejected ($code)" ;;
*) fail "SSE for unknown session returned $code, expected 400/403/404" ;;
esac

# Room names are validated by pattern, not merely by length.
code=$(post POST "$BASE_URL/api/room/Nope!/join" '{"session_id":"'"$SID_A"'"}')
[ "$code" = 400 ] || fail "invalid room name returned $code, expected 400"
ok "invalid room name rejected"

# ------------------------------------------------------------- room claim ----
code=$(post POST "$API/join" '{"session_id":"'"$SID_A"'","display_name":"Smoke A"}')
[ "$code" = 200 ] || fail "first join returned $code, expected 200"
if [ "$(jq -r .status "$body")" != "need-password" ]; then
    fail "a new room should ask to set a password, got: $(cat "$body")"
fi
ok "a new room asks its first visitor to set a password"

code=$(post POST "$API/join" \
    '{"session_id":"'"$SID_A"'","claim_password":"'"$PASSWORD"'","display_name":"Smoke A"}')
[ "$code" = 200 ] || fail "claim join returned $code"
[ "$(jq -r .status "$body")" = "ok" ] || fail "claim join status: $(cat "$body")"
if [ "$(jq -r .self_id "$body")" != "$SID_A" ]; then
    fail "self_id should echo the session id, got $(jq -r .self_id "$body")"
fi
ok "claiming a password creates the room and echoes self_id"

# The room now exists and is protected, so a wrong password must fail.
code=$(post POST "$API/join" '{"session_id":"'"$SID_B"'","password":"not-it"}')
if [ "$(jq -r .status "$body")" != "password-required" ]; then
    fail "wrong password should give password-required, got: $(cat "$body")"
fi
ok "wrong password refused"

# A participant slot is what authorizes chat, not knowledge of the room.
code=$(post POST "$API/chat?session_id=$SID_NOBODY" '{"text":"sneak"}')
[ "$code" = 403 ] || fail "chat from a non-participant returned $code, expected 403"
ok "chat from a non-participant refused"

# ------------------------------------------------------------------ mesh -----
# Subscribe A, then join and subscribe B. Order matters: peer-joined is
# broadcast from the joiner's SSE handler, so B must connect for A to hear it.
curl -s -N --max-time 10 "$API/events?session_id=$SID_A" >"$sse_a" 2>/dev/null &
pid_a=$!
sleep 1

code=$(post POST "$API/join" \
    '{"session_id":"'"$SID_B"'","password":"'"$PASSWORD"'","display_name":"Smoke B"}')
[ "$code" = 200 ] || fail "second join returned $code"
[ "$(jq -r .status "$body")" = "ok" ] || fail "second join status: $(cat "$body")"
if [ "$(jq -r '.peers | index("'"$SID_B"'")' "$body")" != "null" ]; then
    fail "peers list should exclude the joiner itself: $(cat "$body")"
fi
if [ "$(jq -r '.peers | index("'"$SID_A"'") != null' "$body")" != "true" ]; then
    fail "second joiner should see the first participant: $(cat "$body")"
fi
ok "joiners see each other, and never themselves, in peers"

curl -s -N --max-time 10 "$API/events?session_id=$SID_B" >"$sse_b" 2>/dev/null &
pid_b=$!
sleep 1

# Media signals are relayed to exactly one participant, with `to` rewritten to
# `from` so the receiver knows whose connection to answer on.
sdp="v=0-smoke-$ROOM"
code=$(post POST "$API/signal?session_id=$SID_A" \
    '{"type":"offer","to":"'"$SID_B"'","sdp":"'"$sdp"'"}')
[ "$code" = 200 ] || fail "relaying an offer returned $code, expected 200"

# And chat, which is broadcast rather than addressed.
code=$(post POST "$API/chat?session_id=$SID_A" '{"text":"'"$HELLO"'","sender_name":"Smoke A"}')
[ "$code" = 200 ] || fail "sending chat returned $code, expected 200"

code=$(post GET "$API/chat/history?session_id=$SID_B")
[ "$code" = 200 ] || fail "reading chat history returned $code"
if ! jq -e --arg t "$HELLO" 'any(.[]; .text == $t and .sender_name == "Smoke A")' \
    "$body" >/dev/null; then
    fail "chat history missing the message: $(cat "$body")"
fi
ok "chat is persisted and readable by another participant"

# Empty chat is rejected; oversized is truncated (chars().take(2000)).
code=$(post POST "$API/chat?session_id=$SID_A" '{"text":""}')
[ "$code" = 400 ] || fail "empty chat returned $code, expected 400"
ok "empty chat message rejected"

sleep 2
kill "$pid_a" "$pid_b" 2>/dev/null || true
wait "$pid_a" 2>/dev/null || true
wait "$pid_b" 2>/dev/null || true

grep -q '"event":"resync"' "$sse_a" ||
    fail "SSE did not open with a resync: $(head -c 300 "$sse_a")"
ok "SSE opens with a resync (room state on connect)"

if ! grep -q '"event":"peer-joined"' "$sse_a"; then
    fail "A was never told about B: $(head -c 400 "$sse_a")"
fi
if ! grep -q "$SID_B" "$sse_a"; then
    fail "peer-joined did not name the joiner: $(head -c 400 "$sse_a")"
fi
ok "an existing participant is told about the joiner"

if ! grep -q '"event":"offer"' "$sse_b"; then
    fail "offer was not relayed to B: $(head -c 400 "$sse_b")"
fi
if ! grep -q '"from":"'"$SID_A"'"' "$sse_b"; then
    fail "relayed offer should be re-labelled with from: $(head -c 400 "$sse_b")"
fi
ok "offer relayed to the addressee with to rewritten to from"

if ! grep -q '"event":"chat-message"' "$sse_b"; then
    fail "chat was not broadcast to B: $(head -c 400 "$sse_b")"
fi
if ! grep -q "$HELLO" "$sse_a"; then
    fail "chat was not broadcast to the sender's own stream: $(head -c 400 "$sse_a")"
fi
ok "chat is broadcast to the room over SSE"

# Reconnecting must heal: B comes back and gets the message posted before it
# (re)connected, in a resync rather than a replay gap.
heal=$(curl -s -N --max-time 3 "$API/events?session_id=$SID_B" 2>/dev/null || true)
if ! echo "$heal" | grep -q '"event":"resync"'; then
    fail "reconnect did not open with a resync: $(echo "$heal" | head -c 300)"
fi
if ! echo "$heal" | grep -q "$HELLO"; then
    fail "resync after reconnect should replay recent chat"
fi
ok "reconnect replays recent chat (gap healing)"

printf '\napi-smoke: %d assertions passed against %s (room %s)\n' \
    "$pass" "$BASE_URL" "$ROOM"
