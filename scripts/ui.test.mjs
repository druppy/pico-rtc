// Browser tests: drives N headless-Chromium tabs over CDP and asserts on the
// real room UI — the stage split, the speaker/gallery switch, the mobile
// gallery rule, and the chat overlay with its unread badge.
//
//   node --test scripts/ui.test.mjs
//
// Normally you run it through the launcher, which starts Chromium with the fake
// media devices this suite needs:
//
//   sh scripts/ui-smoke.sh 3
//
//   BASE_URL=http://127.0.0.1:3000   where the server listens
//   CDP_URL=http://127.0.0.1:9333    a Chromium already running with
//                                    --use-fake-device-for-media-stream
//   ROOM=...                         room to join (fresh per run by default)
//   PEERS=3                          number of tabs (2-6)
//
// Every assertion is its own test case, so a failure lands in the JUnit report
// next to the checks that passed instead of aborting the run halfway.

import { describe, before, after, test } from "node:test";
import assert from "node:assert/strict";

import { Cdp, openTab, setField, click, sleep, roomState } from "./lib/browser.mjs";

const BASE = process.env.BASE_URL || "http://127.0.0.1:3000";
const CDP_URL = process.env.CDP_URL || "http://127.0.0.1:9333";
// A fresh room per run: a reused name would still hold the previous run's
// participants, which would throw off every peer-count assertion.
const ROOM = process.env.ROOM || `ui-${Date.now().toString(36)}`;

// Read from the environment rather than argv: `node --test file.mjs extra`
// treats the extra argument as another test file, not as an argument to this one.
const PEERS = Math.min(Math.max(Number(process.env.PEERS || 3) || 3, 2), 6);
const PW = "pw123";

// Media over fake cameras plus mesh negotiation is the slow part of this suite;
// everything after it settles is sub-second.
const MESH_TIMEOUT = Number(process.env.MESH_TIMEOUT || 60000);

/**
 * Polls an async condition until `done` accepts the observation.
 *
 * The lib's `waitFor` takes a synchronous predicate, which cannot express "ask
 * the page". Here the observation itself is kept so a timeout prints what the
 * page last reported instead of a bare `false`.
 */
async function until(what, observe, done, ms = 15000) {
  const deadline = Date.now() + ms;
  for (;;) {
    const seen = await observe();
    if (done(seen)) return seen;
    if (Date.now() > deadline) {
      throw new Error(
        `timed out after ${ms}ms waiting for ${what}; last saw ${JSON.stringify(seen)}`,
      );
    }
    await sleep(150);
  }
}

/**
 * Submits the room gate, retrying until it exists.
 *
 * The form is rendered by wasm a beat after navigation and `setField` throws
 * while the input is absent, so this is the retry that lets the suite survive
 * a slow first paint — and report honestly when the gate never appeared.
 */
async function join(cdp, sessionId, ms = 25000) {
  const deadline = Date.now() + ms;
  while (Date.now() < deadline) {
    try {
      await setField(cdp, sessionId, ".room-gate input[type=password]", PW);
      return true;
    } catch {
      await sleep(250);
    }
  }
  return false;
}

const feedsOf = (s) => s.feeds ?? [];
const columnsOf = (s) => (s.gridCols ?? "").split(" ").filter(Boolean);

/**
 * Clicks, and fails at once if nothing matched.
 *
 * `click` reports a missing selector as `"missing"` rather than throwing, which
 * would otherwise turn a renamed button into a confusing poll timeout waiting
 * for a class that the absent button could never set.
 */
async function tap(cdp, sessionId, selector) {
  assert.notEqual(
    await click(cdp, sessionId, selector),
    "missing",
    `no element matching ${selector}`,
  );
}

describe("Room UI in a real browser", () => {
  let cdp;
  /** label -> CDP sessionId, in join order. */
  const tabs = new Map();
  const labels = "ABCDEF".slice(0, PEERS).split("");

  let a; // the tab most assertions are read from
  let b;

  before(async () => {
    // Like the API suite, the suite title carries no per-run value: JUnit
    // consumers key test identity on it, so putting the room name in here would
    // give every run a new test with no history. The room is logged instead.
    console.log(`  room ${ROOM} on ${BASE} with ${labels.length} peers`);

    const live = await fetch(`${BASE}/`, { signal: AbortSignal.timeout(3000) });
    assert.equal(live.status, 200, "no app server answering on " + BASE);

    cdp = await Cdp.connect(CDP_URL);

    for (const label of labels) {
      const sid = await openTab(cdp, `${BASE}/room/${ROOM}`);
      tabs.set(label, sid);
      assert.ok(await join(cdp, sid), `${label}: the room gate never appeared`);
      // Stagger the joins: the first visitor claims the room by setting its
      // password, and the others must not race that claim.
      await sleep(1200);
    }

    a = tabs.get("A");
    b = tabs.get("B");
  });

  after(async () => {
    if (!cdp) return;
    for (const sid of tabs.values()) {
      // Best effort; the launcher kills the browser right after this.
      await cdp
        .send("Emulation.clearDeviceMetricsOverride", {}, sid)
        .catch(() => {});
    }
    cdp.close();
  });

  // ── The mesh actually carries media ───────────────────────────────────────
  test(
    "media flows into every peer's remote tiles",
    async () => {
      const expected = labels.length - 1;
      await until(
        `${expected} remote feed(s) with frames in every tab`,
        async () => {
          const per = {};
          for (const [label, sid] of tabs) {
            per[label] = feedsOf(await roomState(cdp, sid)).map((f) => f.w);
          }
          return per;
        },
        (per) =>
          Object.values(per).every(
            (widths) => widths.length === expected && widths.every((w) => w > 0),
          ),
        MESH_TIMEOUT,
      );
    },
    { timeout: MESH_TIMEOUT + 30000 },
  );

  // ── Speaker mode (the default) ────────────────────────────────────────────
  test("the stage starts in speaker mode", async () => {
    const s = await roomState(cdp, a);
    assert.ok(String(s.stage).includes("is-speaker"), `stage class: ${s.stage}`);
  });

  test("the local tile is the first child of the stage", async () => {
    const s = await roomState(cdp, a);
    assert.ok(
      String(s.stageKids?.[0]).includes("local"),
      `first child: ${s.stageKids?.[0]}`,
    );
  });

  test("speaker mode stages exactly one remote feed", async () => {
    const s = await roomState(cdp, a);
    assert.equal(
      feedsOf(s).filter((f) => f.stage).length,
      1,
      JSON.stringify(feedsOf(s)),
    );
  });

  test("speaker mode displays only the staged feed", async () => {
    const s = await roomState(cdp, a);
    assert.equal(
      feedsOf(s).filter((f) => f.shown !== "none").length,
      1,
      JSON.stringify(feedsOf(s).map((f) => f.shown)),
    );
  });

  test("the self strip is about a fifth of the stage", async () => {
    const s = await roomState(cdp, a);
    const cols = columnsOf(s).map(parseFloat);
    assert.equal(cols.length, 2, `expected two columns, got: ${s.gridCols}`);
    assert.ok(
      cols[1] / cols[0] > 3.2,
      `remote stage should dwarf the self strip, got ${s.gridCols}`,
    );
  });

  test("the mode button starts on Speaker", async () => {
    const s = await roomState(cdp, a);
    assert.equal(s.modeBtn, "Speaker");
  });

  test("the chat overlay starts closed", async () => {
    const s = await roomState(cdp, a);
    assert.equal(s.overlayOpen, false);
    assert.equal(s.overlayShown, "none");
  });

  // ── Gallery mode ──────────────────────────────────────────────────────────
  test("switching to gallery shows every feed", async () => {
    await tap(cdp, a, ".stage-mode");
    const s = await until(
      "the stage to switch to gallery",
      () => roomState(cdp, a),
      (x) => String(x.stage).includes("is-gallery"),
    );
    const feeds = feedsOf(s);
    assert.ok(feeds.length > 0, "no remote feeds to lay out");
    assert.ok(
      feeds.every((f) => f.shown !== "none"),
      JSON.stringify(feeds.map((f) => f.shown)),
    );
  });

  test("the mode button reads Gallery while in gallery mode", async () => {
    const s = await roomState(cdp, a);
    assert.equal(s.modeBtn, "Gallery");
  });

  test("switching back returns to speaker mode", async () => {
    await tap(cdp, a, ".stage-mode");
    const s = await until(
      "the stage to switch back to speaker",
      () => roomState(cdp, a),
      (x) => String(x.stage).includes("is-speaker"),
    );
    assert.equal(s.modeBtn, "Speaker");
    assert.equal(
      feedsOf(s).filter((f) => f.shown !== "none").length,
      1,
      JSON.stringify(feedsOf(s).map((f) => f.shown)),
    );
  });

  // ── Chat: badge while closed, cleared on open ─────────────────────────────
  test("a message arriving while the overlay is closed raises the badge", async () => {
    await setField(cdp, b, "#chat-overlay input", "hello from B");

    const s = await until(
      "A's unread badge to count one",
      () => roomState(cdp, a),
      (x) => x.badge === "1",
    );
    assert.equal(
      s.overlayOpen,
      false,
      "receiving a message must not open the overlay by itself",
    );
    assert.ok(s.msgs >= 1, `message never reached the (hidden) list: ${s.msgs}`);
  });

  test("the sender does not count its own message as unread", async () => {
    const s = await roomState(cdp, b);
    assert.equal(s.badge, null, `sender's badge read ${s.badge}`);
  });

  test("opening the overlay clears the badge", async () => {
    await tap(cdp, a, ".chat-toggle");
    // Both conditions in one poll: the click handler clears the backlog and
    // opens the overlay together, so a render that shows one without the other
    // would be a real bug rather than a race worth sleeping through.
    const s = await until(
      "the overlay to open with its badge cleared",
      () => roomState(cdp, a),
      (x) => x.overlayOpen === true && x.badge === null,
    );
    assert.equal(s.overlayShown, "flex");
    assert.equal(s.badge, null, "the badge should clear once the chat has been read");
  });

  test("opening the overlay scrolls to the newest message", async () => {
    const s = await roomState(cdp, a);
    assert.equal(s.scrolledToBottom, true, "the chat log should be pinned to the bottom");
  });

  test("the close button shuts the overlay", async () => {
    await tap(cdp, a, ".chat-close");
    await until(
      "the overlay to close",
      () => roomState(cdp, a),
      (x) => x.overlayOpen === false && x.overlayShown === "none",
    );
  });

  // ── Mobile: always a gallery ──────────────────────────────────────────────
  test("a phone-width viewport forces the gallery", async () => {
    await cdp.send(
      "Emulation.setDeviceMetricsOverride",
      { width: 390, height: 844, deviceScaleFactor: 1, mobile: true },
      a,
    );
    try {
      const s = await until(
        "the mobile layout to take effect",
        () => roomState(cdp, a),
        (x) =>
          columnsOf(x).length === 1 && feedsOf(x).every((f) => f.shown !== "none"),
      );
      assert.equal(
        s.modeShown,
        "none",
        "the speaker/gallery choice is meaningless once there is only a gallery",
      );
    } finally {
      await cdp.send("Emulation.clearDeviceMetricsOverride", {}, a);
    }
  });

  // ── What the page reached out to, and what it threw ───────────────────────
  const resourceUrls = async () => {
    await cdp.send("Performance.enable", {}, a);
    const raw = await cdp.evaluate(
      a,
      `JSON.stringify(performance.getEntriesByType("resource").map(e => e.name))`,
    );
    return JSON.parse(raw);
  };

  test("the page makes no third-party requests", async () => {
    const outside = (await resourceUrls()).filter((u) => !u.startsWith(BASE));
    assert.deepEqual(outside, [], "Pico must stay vendored — no CDN");
  });

  test("the compiled Pico stylesheet is what the page loaded", async () => {
    const urls = await resourceUrls();
    assert.ok(
      urls.some((u) => u.includes("/pkg/webrtc-room.css")),
      `expected the Sass output under /pkg/, saw ${JSON.stringify(urls)}`,
    );
  });

  // One case per tab, so a crash names its tab without reading the message.
  for (const label of labels) {
    test(`${label}: no uncaught exceptions`, () => {
      assert.deepEqual(cdp.pageErrors.get(tabs.get(label)) ?? [], []);
    });
  }
});
