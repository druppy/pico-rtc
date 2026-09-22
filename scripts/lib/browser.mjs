// A minimal Chrome DevTools Protocol client, just enough to drive several tabs
// of the same page and read their DOM back.
//
// Written against CDP rather than Playwright/Puppeteer on purpose: this project
// has no JS dependency of its own, and the browser is only ever a measurement
// instrument here. If the suite ever needs auto-waiting or traces on failure,
// that is the point to reach for a real driver instead of growing this file.

export const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

/** Waits for a condition instead of sleeping a guessed amount. */
export async function waitFor(what, predicate, ms = 15000) {
  const deadline = Date.now() + ms;
  while (Date.now() < deadline) {
    const value = predicate();
    if (value) return value;
    await sleep(100);
  }
  throw new Error(`timed out after ${ms}ms waiting for ${what}`);
}

export class Cdp {
  constructor(ws) {
    this.ws = ws;
    this.id = 0;
    this.pending = new Map();
    /** Per-session uncaught exceptions, so a test can assert their absence. */
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
        const detail = msg.params?.exceptionDetails?.exception?.description ?? "?";
        if (!this.pageErrors.has(sid)) this.pageErrors.set(sid, []);
        this.pageErrors.get(sid).push(detail.slice(0, 300));
      }
    });
  }

  static async connect(cdpUrl) {
    const { webSocketDebuggerUrl } = await fetch(
      `${cdpUrl.replace(/\/$/, "")}/json/version`,
    ).then((r) => r.json());
    const ws = new WebSocket(webSocketDebuggerUrl);
    await new Promise((resolve, reject) => {
      ws.addEventListener("open", resolve);
      ws.addEventListener("error", () =>
        reject(new Error(`cannot reach Chromium at ${cdpUrl}`)),
      );
    });
    return new Cdp(ws);
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
        throw new Error(
          `page threw: ${r.exceptionDetails.exception?.description ?? "err"}`,
        );
      }
      return r.result.value;
    } catch (e) {
      throw new Error(`evaluate failed: ${e.message.slice(0, 200)}`);
    }
  }

  close() {
    this.ws.close();
  }
}

/** Opens a tab attached to its own session so several peers can coexist. */
export async function openTab(cdp, url) {
  const { targetId } = await cdp.send("Target.createTarget", { url: "about:blank" });
  const { sessionId } = await cdp.send("Target.attachToTarget", {
    targetId,
    flatten: true,
  });
  await cdp.send("Runtime.enable", {}, sessionId);
  await cdp.send("Page.enable", {}, sessionId);
  await cdp.send("Page.navigate", { url }, sessionId);
  return sessionId;
}

/**
 * Writes a framework-controlled input through its native setter, so Leptos'
 * own `input` handler observes the change, then submits the surrounding form.
 * Setting `.value` directly would not register with the framework.
 */
export function setField(cdp, sessionId, selector, value) {
  return cdp.evaluate(
    sessionId,
    `(() => {
      const el = document.querySelector(${JSON.stringify(selector)});
      if (!el) throw new Error("no input matching ${selector}");
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

/**
 * Clicks the first match, reporting whether it was there at all.
 *
 * The check has to be on the element, not on the result: `HTMLElement.click()`
 * returns `undefined`, so `querySelector(s)?.click() ?? "missing"` reports
 * "missing" for a successful click too, and the caller cannot tell the two
 * cases apart.
 */
export function click(cdp, sessionId, selector) {
  return cdp.evaluate(
    sessionId,
    `(() => {
      const el = document.querySelector(${JSON.stringify(selector)});
      if (!el) return "missing";
      el.click();
      return "clicked";
    })()`,
  );
}

/**
 * Everything the room assertions read about one tab, in a single round trip.
 * `w` is videoWidth: zero means no frames have arrived for that peer yet.
 */
export const ROOM_STATE = `JSON.stringify({
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

export async function roomState(cdp, sessionId) {
  const raw = await cdp.evaluate(sessionId, ROOM_STATE);
  if (typeof raw !== "string") throw new Error(`unexpected state payload: ${raw}`);
  return JSON.parse(raw);
}
