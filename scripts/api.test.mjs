// HTTP-level tests against a running server, in the style of an integration
// test rather than a script: every assertion is its own test case, so one
// failure reports alongside the rest instead of aborting the run.
//
//   node --test scripts/api.test.mjs
//   BASE_URL=http://127.0.0.1:3000 node --test scripts/api.test.mjs
//
// The server must already be running (it writes target/site/index.html on
// startup, so it also has to have started after the last build).

import { describe, before, after, test } from "node:test";
import assert from "node:assert/strict";

const BASE = process.env.BASE_URL || "http://127.0.0.1:3000";
// A fresh room per run: the room is claimed by its first joiner, so a reused
// name would make the "need-password" case depend on leftover state.
const ROOM = process.env.ROOM || `api-${Date.now().toString(36)}`;
const API = `${BASE}/api/room/${ROOM}`;

// Session ids are opaque to the server; these only have to differ.
const A = "11111111-1111-4111-8111-111111111111";
const B = "22222222-2222-4222-8222-222222222222";
const NOBODY = "00000000-0000-4000-8000-000000000000";
const PW = "let-me-in";
const HELLO = `hello-from-api-test-${Date.now()}`;
const SDP = `v=0-api-test-${ROOM}`;

const TIMEOUT = Number(process.env.TEST_TIMEOUT || 10000);

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

async function req(method, url, body) {
  const res = await fetch(url, {
    method,
    headers: body ? { "content-type": "application/json" } : undefined,
    body: body ? JSON.stringify(body) : undefined,
    signal: AbortSignal.timeout(TIMEOUT),
  });
  const text = await res.text();
  let json;
  try {
    json = JSON.parse(text);
  } catch {
    /* non-JSON responses are asserted on as text */
  }
  return { status: res.status, text, json };
}

const join = (body) => req("POST", `${API}/join`, body);

/**
 * Opens an SSE stream and collects parsed events as they arrive.
 *
 * The stream never ends on its own, so it is aborted in `after()` rather than
 * by a timeout; the caller reads `events` as it grows.
 */
async function openSse(sessionId) {
  const ctrl = new AbortController();
  const events = [];
  const res = await fetch(`${API}/events?session_id=${sessionId}`, {
    signal: ctrl.signal,
  });
  if (res.ok) {
    (async () => {
      let buf = "";
      const decoder = new TextDecoder();
      try {
        for await (const chunk of res.body) {
          buf += decoder.decode(chunk, { stream: true });
          let cut;
          while ((cut = buf.indexOf("\n\n")) >= 0) {
            const record = buf.slice(0, cut);
            buf = buf.slice(cut + 2);
            for (const line of record.split("\n")) {
              if (!line.startsWith("data:")) continue;
              try {
                events.push(JSON.parse(line.slice(5).trim()));
              } catch {
                /* keep-alive or partial frame */
              }
            }
          }
        }
      } catch {
        /* aborted at cleanup, which is the normal ending */
      }
    })();
  }
  return { status: res.status, events, close: () => ctrl.abort() };
}

/** Waits for a condition instead of sleeping a guessed amount. */
async function waitFor(what, predicate, ms = 5000) {
  const deadline = Date.now() + ms;
  while (Date.now() < deadline) {
    if (predicate()) return true;
    await sleep(50);
  }
  assert.fail(`timed out after ${ms}ms waiting for ${what}`);
}

const of = (events, type) => events.filter((e) => e.event === type);

describe("API against the running server", () => {
  const streams = [];
  let sseA;
  let sseB;

  before(async () => {
    // The suite name stays constant on purpose: JUnit consumers key on it, so
    // putting the per-run room name in there would give every run a new test
    // identity and no history to compare against. The room is logged instead.
    console.log(`  room ${ROOM}`);
    const live = await fetch(`${BASE}/`, { signal: AbortSignal.timeout(3000) });
    assert.equal(live.status, 200, "no server answering on " + BASE);
  });

  after(() => {
    for (const s of streams) s.close();
  });

  // ── Liveness and shape ────────────────────────────────────────────────────
  test("turn-credentials serves ephemeral-shaped credentials", async () => {
    const { status, json } = await req("GET", `${BASE}/api/turn-credentials`);
    assert.equal(status, 200);
    assert.ok(Array.isArray(json.urls) && json.urls.length > 0, "urls missing");
    // Per the TURN REST API the username is "<expiry>:<id>".
    assert.match(json.username, /:/, "username should be expiry:id");
    assert.ok(json.credential?.length > 0, "credential missing");
    assert.ok(json.ttl_secs > 0, "ttl missing");
  });

  test("index shell serves with the vendored stylesheet and no CDN", async () => {
    const { text } = await req("GET", `${BASE}/`);
    assert.match(text, /webrtc-room\.css/, "stylesheet not linked");
    assert.doesNotMatch(
      text,
      /jsdelivr|unpkg|cdnjs/i,
      "a CDN is referenced; Pico must stay vendored",
    );
  });

  // ── Push and signaling are addressed ──────────────────────────────────────
  // Both must be refused for a session that never joined, or any visitor could
  // inject offers into a room they are not in.
  test("signal from an unknown session is refused", async () => {
    const { status } = await req("POST", `${API}/signal?session_id=${NOBODY}`, {
      type: "offer",
      to: B,
      sdp: "v=0",
    });
    assert.ok([400, 403, 404].includes(status), `got ${status}`);
  });

  test("SSE for an unknown session is refused", async () => {
    const s = await openSse(NOBODY);
    streams.push(s);
    assert.ok([400, 403, 404].includes(s.status), `got ${s.status}`);
  });

  test("an invalid room name is refused", async () => {
    const { status } = await req("POST", `${BASE}/api/room/Nope!/join`, {
      session_id: A,
    });
    assert.equal(status, 400);
  });

  // ── Room claim ────────────────────────────────────────────────────────────
  test("a new room asks its first visitor to set a password", async () => {
    const { status, json } = await req("POST", `${API}/join`, {
      session_id: A,
      display_name: "API A",
    });
    assert.equal(status, 200);
    assert.equal(json.status, "need-password");
  });

  test("claiming a password creates the room and echoes self_id", async () => {
    const { status, json } = await req("POST", `${API}/join`, {
      session_id: A,
      claim_password: PW,
      display_name: "API A",
    });
    assert.equal(status, 200);
    assert.equal(json.status, "ok");
    assert.equal(json.self_id, A, "self_id should echo the session id");
  });

  test("wrong password is refused", async () => {
    const { json } = await req("POST", `${API}/join`, {
      session_id: B,
      password: "not-it",
    });
    assert.equal(json.status, "password-required");
  });

  test("chat from a non-participant is refused", async () => {
    const { status } = await req("POST", `${API}/chat?session_id=${NOBODY}`, {
      text: "sneak",
    });
    assert.equal(status, 403);
  });

  // ── Mesh: join, announce, relay ───────────────────────────────────────────
  // Order matters: peer-joined is broadcast from the *joiner's* SSE handler,
  // so B has to connect before A can hear about it.
  test("joiners see each other, and never themselves, in peers", async () => {
    sseA = await openSse(A);
    streams.push(sseA);
    assert.equal(sseA.status, 200);

    const joined = await req("POST", `${API}/join`, {
      session_id: B,
      password: PW,
      display_name: "API B",
    });
    assert.equal(joined.json.status, "ok");
    assert.ok(!joined.json.peers.includes(B), "peers should exclude the joiner");
    assert.ok(joined.json.peers.includes(A), "B should already see A");

    sseB = await openSse(B);
    streams.push(sseB);
    assert.equal(sseB.status, 200);
  });

  test("SSE opens with a resync (room state on connect)", async () => {
    await waitFor("resync on A", () => of(sseA.events, "resync").length > 0);
    await waitFor("resync on B", () => of(sseB.events, "resync").length > 0);
  });

  test("an existing participant is told about the joiner", async () => {
    await waitFor("peer-joined on A", () => of(sseA.events, "peer-joined").length > 0);
    const announced = of(sseA.events, "peer-joined").find((e) => e.peer_id === B);
    assert.ok(announced, "B was never announced to A");
    assert.equal(announced.peer_name, "API B");
    assert.equal(
      of(sseB.events, "peer-joined").length,
      0,
      "the joiner must not receive its own peer-joined",
    );
  });

  test("an offer is relayed to the addressee, with to rewritten to from", async () => {
    const { status } = await req("POST", `${API}/signal?session_id=${A}`, {
      type: "offer",
      to: B,
      sdp: SDP,
    });
    assert.equal(status, 200);
    await waitFor("relayed offer", () => of(sseB.events, "offer").length > 0);
    const offer = of(sseB.events, "offer")[0];
    assert.equal(offer.from, A, "relayed offer should say who sent it");
    assert.equal(offer.sdp, SDP);
    assert.equal(
      of(sseA.events, "offer").length,
      0,
      "the signal must not also reach the sender",
    );
  });

  test("chat is persisted, readable, and broadcast", async () => {
    const sent = await req("POST", `${API}/chat?session_id=${A}`, {
      text: HELLO,
      sender_name: "API A",
    });
    assert.equal(sent.status, 200);

    const history = await req("GET", `${API}/chat/history?session_id=${B}`);
    assert.equal(history.status, 200);
    const mine = history.json.filter((m) => m.text === HELLO);
    assert.equal(mine.length, 1, "the message should be stored exactly once");
    assert.equal(mine[0].sender_name, "API A");

    // Broadcast, not addressed: unlike media signals this reaches everyone,
    // including the sender's own stream.
    await waitFor("chat on B", () => of(sseB.events, "chat-message").length > 0);
    await waitFor("chat on A", () => of(sseA.events, "chat-message").length > 0);
  });

  test("an empty chat message is rejected", async () => {
    const { status } = await req("POST", `${API}/chat?session_id=${A}`, { text: "" });
    assert.equal(status, 400);
  });

  test("reconnecting replays recent chat (gap healing)", async () => {
    const late = await openSse(B);
    streams.push(late);
    await waitFor("resync with chat history", () => of(late.events, "resync").length > 0);
    const resync = of(late.events, "resync").at(-1);
    assert.ok(
      resync.chat.some((m) => m.text === HELLO),
      "a reconnect must see messages posted before it connected",
    );
    late.close();
  });
});
