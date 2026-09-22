// Browser smoke test: drives N headless-Chromium tabs over CDP and asserts on
// the real room UI — the stage split, the speaker/gallery switch, the mobile
// gallery rule, and the chat overlay with its unread badge.
//
//   node scripts/ui-smoke.mjs [peers]
//
//   BASE_URL=http://127.0.0.1:3000   where the server listens
//   CDP_URL=http://127.0.0.1:9333    Chromium's DevTools endpoint
//
// Chromium must be started with fake media devices, or no frames ever arrive:
//   chromium --headless=new --remote-debugging-port=9333 \
//     --user-data-dir=/tmp/chrome-prof --use-fake-ui-for-media-stream \
//     --use-fake-device-for-media-stream \
//     --autoplay-policy=no-user-gesture-required --disable-gpu --no-sandbox
//
// Exit 0 means every check passed.

const BASE = process.env.BASE_URL || "http://127.0.0.1:3000";
const CDP = process.env.CDP_URL || "http://127.0.0.1:9333";
// Fresh room per run: a reused name would still hold the previous run's
// participants, which would throw off every peer-count assertion.
const ROOM = process.env.ROOM || `ui-${Date.now().toString(36)}`;
const PEERS = Number(process.argv[2] || process.env.PEERS || 2);
const PW = "pw123";

class CdpClient {
  constructor(ws) {
    this.ws = ws;
    this.id = 0;
    this.pending = new Map();
    this.pageErrors = new Map();
    ws.addEventListener("message", (ev) => {
      const msg = JSON.parse(ev.data);
      if (msg.id && this.pending.has(msg.id)) {
        const { resolve, reject } = this.pending.get(msg.id);
        this.pending.delete(msg.id);
        msg.error
          ? reject(new Error(JSON.stringify(msg.error)))
          : resolve(msg.result);
        return;
      }
      if (msg.method === "Runtime.exceptionThrown") {
        const sid = msg.sessionId;
        const d = msg.params?.exceptionDetails?.exception?.description ?? "?";
        if (!this.pageErrors.has(sid)) this.pageErrors.set(sid, []);
        this.pageErrors.get(sid).push(d.slice(0, 300));
      }
    });
  }

  send(method, params = {}, sessionId) {
    const id = ++this.id;
    const payload = { id, method, params };
    if (sessionId) payload.sessionId = sessionId;
    return new Promise((resolve, reject) => {
      this.pending.set(id, { resolve, reject });
      this.ws.send(JSON.stringify(payload));
      setTimeout(() => {
        if (this.pending.has(id)) {
          this.pending.delete(id);
          reject(new Error(`timeout: ${method}`));
        }
      }, 25000);
    });
  }

  async evaluate(sessionId, expression) {
    try {
      const r = await this.send(
        "Runtime.evaluate",
        { expression, returnByValue: true },
        sessionId,
      );
      if (r.exceptionDetails) {
        return `EVAL-THREW: ${r.exceptionDetails.exception?.description ?? "err"}`;
      }
      return r.result.value === undefined ? "(undefined)" : r.result.value;
    } catch (e) {
      return `EVAL-REJECTED: ${e.message.slice(0, 80)}`;
    }
  }
}

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

// Everything the assertions read about one tab, in one round trip.
const STATE = `JSON.stringify({
  stage: document.querySelector(".stage")?.className ?? null,
  stageKids: Array.from(document.querySelector(".stage")?.children ?? [])
    .map((e) => e.tagName.toLowerCase() + "." + e.className),
  gridCols: getComputedStyle(document.querySelector(".stage") ?? document.body).gridTemplateColumns,
  feeds: Array.from(document.querySelectorAll(".feeds > .video-tile")).map((e) => ({
    id: e.querySelector("video")?.id,
    stage: e.classList.contains("is-stage"),
    shown: getComputedStyle(e).display,
    w: e.querySelector("video")?.videoWidth ?? 0,
  })),
  empty: document.querySelector(".stage-empty")?.textContent ?? null,
  modeBtn: document.querySelector(".stage-mode")?.textContent.trim() ?? null,
  modeShown: getComputedStyle(document.querySelector(".stage-mode") ?? document.body).display,
  badge: document.querySelector(".chat-badge")?.textContent ?? null,
  overlayOpen: document.querySelector("#chat-overlay")?.classList.contains("is-open") ?? null,
  overlayShown: getComputedStyle(document.querySelector("#chat-overlay") ?? document.body).display,
  msgs: document.querySelectorAll("#chat-log .chat-msg").length,
  scrolledToBottom: (() => {
    const el = document.querySelector("#chat-log");
    return el ? el.scrollTop + el.clientHeight >= el.scrollHeight - 4 : null;
  })(),
})`;

async function openTab(cdp, url) {
  const { targetId } = await cdp.send("Target.createTarget", { url: "about:blank" });
  const { sessionId } = await cdp.send("Target.attachToTarget", { targetId, flatten: true });
  await cdp.send("Runtime.enable", {}, sessionId);
  await cdp.send("Page.enable", {}, sessionId);
  await cdp.send("Page.navigate", { url }, sessionId);
  return sessionId;
}

function state(cdp, sid) {
  return cdp.evaluate(sid, STATE).then((raw) => {
    try {
      return JSON.parse(raw);
    } catch {
      return { parseError: String(raw).slice(0, 200) };
    }
  });
}

// Writes a framework-controlled input through its native setter, so Leptos'
// own `input` handler observes the change, then submits the surrounding form.
function setField(cdp, sid, selector, value) {
  return cdp.evaluate(
    sid,
    `(() => {
      const el = document.querySelector(${JSON.stringify(selector)});
      if (!el) return "no-input";
      const setter = Object.getOwnPropertyDescriptor(
        window.HTMLInputElement.prototype, "value",
      ).set;
      setter.call(el, ${JSON.stringify(value)});
      el.dispatchEvent(new Event("input", { bubbles: true }));
      el.closest("form").requestSubmit();
      return "submitted";
    })()`,
  );
}

let failures = 0;
let checks = 0;
function check(name, ok, detail = "") {
  checks++;
  console.log(`  ${ok ? "PASS" : "FAIL"}  ${name}${detail ? "  " + detail : ""}`);
  if (!ok) failures++;
}

async function main() {
  const { webSocketDebuggerUrl } = await fetch(
    `${CDP.replace(/\/$/, "")}/json/version`,
  ).then((r) => r.json());
  const ws = new WebSocket(webSocketDebuggerUrl);
  await new Promise((res, rej) => {
    ws.addEventListener("open", res);
    ws.addEventListener("error", () => rej(new Error(`cannot reach Chromium at ${CDP}`)));
  });
  const cdp = new CdpClient(ws);

  const labels = "ABCDEF".slice(0, Math.min(PEERS, 6)).split("");
  const tabs = new Map();
  console.log(`room ${ROOM} on ${BASE} with ${labels.length} peers`);

  for (const label of labels) {
    const sid = await openTab(cdp, `${BASE}/room/${ROOM}`);
    tabs.set(label, sid);
    let joined = false;
    for (let i = 0; i < 60 && !joined; i++) {
      joined = (await setField(cdp, sid, ".room-gate input[type=password]", PW)) === "submitted";
      if (!joined) await sleep(250);
    }
    console.log(`  ${label} join: ${joined}`);
    await sleep(1200);
  }

  // Wait for media to actually flow into every remote tile.
  let up = false;
  for (let t = 0; t < 20 && !up; t++) {
    await sleep(2000);
    let all = true;
    for (const [, sid] of tabs) {
      const s = await state(cdp, sid);
      const remote = s.feeds ?? [];
      if (remote.length !== labels.length - 1 || !remote.every((f) => f.w > 0)) all = false;
    }
    up = all;
    console.log(`  mesh ${all ? "up" : "waiting"}`);
  }
  check("mesh flows to every tile", up);

  const a = tabs.get("A");

  // ── Speaker mode (the default) ────────────────────────────────────────────
  let s = await state(cdp, a);
  check("stage has is-speaker", String(s.stage).includes("is-speaker"), s.stage);
  check("local tile is first child of .stage", String(s.stageKids?.[0]).includes("local"), s.stageKids?.[0]);
  check("exactly one staged feed", s.feeds?.filter((f) => f.stage).length === 1, JSON.stringify(s.feeds));
  check(
    "only the staged feed is displayed",
    s.feeds?.filter((f) => f.shown !== "none").length === 1,
    JSON.stringify(s.feeds?.map((f) => f.shown)),
  );
  const cols = (s.gridCols ?? "").split(" ").filter(Boolean).map(parseFloat);
  check("self strip is ~1/5 of the stage", cols.length === 2 && cols[1] / cols[0] > 3.2, s.gridCols);
  check("mode button says Speaker", s.modeBtn === "Speaker", String(s.modeBtn));
  check("chat overlay starts closed", s.overlayOpen === false && s.overlayShown === "none");

  // ── Gallery mode ──────────────────────────────────────────────────────────
  await cdp.evaluate(a, `document.querySelector(".stage-mode").click()`);
  await sleep(400);
  s = await state(cdp, a);
  check("stage has is-gallery", String(s.stage).includes("is-gallery"), s.stage);
  check(
    "every feed is displayed",
    s.feeds?.length > 0 && s.feeds.every((f) => f.shown !== "none"),
    JSON.stringify(s.feeds),
  );
  check("mode button says Gallery", s.modeBtn === "Gallery", String(s.modeBtn));

  // Back to speaker mode for the rest.
  await cdp.evaluate(a, `document.querySelector(".stage-mode").click()`);
  await sleep(400);

  // ── Chat: badge while closed, cleared on open ─────────────────────────────
  const b = tabs.get("B");
  const sent = await setField(cdp, b, "#chat-overlay input", "hello from B");
  console.log(`  chat from B: ${sent}`);
  await sleep(1500);

  s = await state(cdp, a);
  check("badge counted the unread", s.badge === "1", String(s.badge));
  check("overlay still closed", s.overlayOpen === false);
  check("message reached the (hidden) list", s.msgs >= 1, String(s.msgs));

  // Own echo must not count as unread.
  const sb = await state(cdp, b);
  check("sender sees no unread of its own", sb.badge === null, String(sb.badge));

  await cdp.evaluate(a, `document.querySelector(".chat-toggle").click()`);
  await sleep(600);
  s = await state(cdp, a);
  check("overlay open", s.overlayOpen === true && s.overlayShown === "flex");
  check("badge cleared on open", s.badge === null, String(s.badge));
  check("list scrolled to newest", s.scrolledToBottom === true, String(s.scrolledToBottom));

  await cdp.evaluate(a, `document.querySelector(".chat-close").click()`);
  await sleep(400);
  s = await state(cdp, a);
  check("close button shuts the overlay", s.overlayOpen === false && s.overlayShown === "none");

  // ── Mobile: always a gallery ──────────────────────────────────────────────
  await cdp.send(
    "Emulation.setDeviceMetricsOverride",
    { width: 390, height: 844, deviceScaleFactor: 1, mobile: true },
    a,
  );
  await sleep(700);
  s = await state(cdp, a);
  check(
    "mobile forces gallery (every feed shown)",
    s.feeds?.length > 0 && s.feeds.every((f) => f.shown !== "none"),
    JSON.stringify(s.feeds),
  );
  check("mode toggle hidden on mobile", s.modeShown === "none", String(s.modeShown));
  check(
    "one column stage on mobile",
    (s.gridCols ?? "").split(" ").filter(Boolean).length === 1,
    s.gridCols,
  );
  await cdp.send("Emulation.clearDeviceMetricsOverride", {}, a);

  // ── No third-party requests, no page errors ───────────────────────────────
  await cdp.send("Performance.enable", {}, a);
  const reqs = await cdp.evaluate(
    a,
    `JSON.stringify(performance.getEntriesByType("resource").map(e => e.name))`,
  );
  let outside = [];
  try {
    outside = JSON.parse(reqs).filter((u) => !u.startsWith(BASE));
  } catch {
    /* unreadable resource list — the check below reports it */
  }
  check("no third-party requests", outside.length === 0, JSON.stringify(outside));
  check("own stylesheet loaded", String(reqs).includes("/pkg/webrtc-room.css"));

  for (const label of labels) {
    const errs = cdp.pageErrors.get(tabs.get(label)) ?? [];
    check(`${label}: no uncaught exceptions`, errs.length === 0, errs.join(" | ").slice(0, 200));
  }

  ws.close();
  console.log(
    `\nui-smoke: ${checks - failures}/${checks} checks passed` +
      (failures ? ` — ${failures} FAILED` : ""),
  );
  process.exit(failures === 0 ? 0 : 1);
}

main().catch((e) => {
  console.error("ui-smoke aborted:", e.message);
  process.exit(1);
});
