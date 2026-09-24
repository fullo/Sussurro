/* Background worker (Chrome: service worker, Firefox: event page) — #128.
 *
 * Owns, per tab, the capture session (session.ts) and the WebSocket to the
 * app's `/live` (ws://127.0.0.1:<port>/live?token=…, port and token from
 * the pairing in `storage.local`). It:
 *
 * - checks the app with `GET /app/version` before starting and before each
 *   reconnect ("app not running", bad token, meetings off, other protocol);
 * - on the side panel's Start, asks the page to connect and arm the hook,
 *   opens the socket, sends `start {title, url, platform, rate, channels}`
 *   and forwards the audio frames, numbering `seq` per connection;
 * - buffers audio while (re)connecting (bounded), reconnects with backoff,
 *   sends `ping` when no audio went out for a while;
 * - sends `stop` on Stop, tab close or navigation;
 * - shows a REC badge on the toolbar button of a tab being captured;
 * - relays the Meet page's speaker events (#131) on the connection's
 *   audio clock, and replays what the page already said to a new
 *   connection (shared/speakers.ts);
 * - (Chrome) falls back to `tabCapture` through an offscreen document for
 *   the remote channel when the page shows no remote audio at all.
 *
 * Nothing is captured before an explicit Start. */
import browser, { type Runtime } from "webextension-polyfill";
import { CHANNEL, SeqCounter, encodeFrame, type ChannelByte } from "../shared/frame";
import { getPairing, liveUrl, onPairingChanged } from "../shared/pairing";
import { testConnection } from "../shared/connection";
import { toAppCheck, type AppCheck } from "../shared/appcheck";
import { decodePayload, makeProbe, type TransportMode } from "../shared/transport";
import type { CaptureSnapshot, FromBackground, PageInfo, PanelBroadcast, PanelRequest, PanelState, ToBackground, ToOffscreen, ToPage } from "../shared/messages";
import type { Platform } from "../shared/platform";
import { initialSession, isCapturing, shouldTabCapture, step, type Effect, type SessionEvent, type Session } from "./session";
import { SpeakerRelay, sanitizePageSpeaker } from "../shared/speakers";

// ---- toolbar button → panel ----------------------------------------------------

if (__BROWSER__ === "chrome") {
  // Chrome: the toolbar button opens the side panel.
  chrome?.sidePanel?.setPanelBehavior({ openPanelOnActionClick: true }).catch((e: unknown) => {
    console.error("Sussurro: cannot bind the side panel to the toolbar button", e);
  });
} else {
  // Firefox: toggle the sidebar (allowed here: we are inside a user action).
  browser.action.onClicked.addListener(() => {
    browser.sidebarAction.toggle().catch((e: unknown) => {
      console.error("Sussurro: cannot toggle the sidebar", e);
    });
  });
}

// ---- per-tab state ---------------------------------------------------------------

/** Audio kept while the socket is not open: ~30 s per channel at 48 kHz. */
const MAX_QUEUE = 1400;
/** Socket backlog above which new frames are dropped (and counted as lost). */
const MAX_BUFFERED_BYTES = 4 * 1024 * 1024;
/** `ping` after this long without sending anything. */
const IDLE_PING_MS = 10_000;
/** The app check is reused for this long by the side panel. */
const CHECK_TTL_MS = 5_000;
const CHECK_TIMEOUT_MS = 3_000;

interface Queued {
  ch: ChannelByte;
  producer: "page" | "tab";
  pseq: number;
  buf: ArrayBuffer;
}

interface Tab {
  tabId: number;
  session: Session;
  /** The meeting page's runtime port (while connected). */
  port: Runtime.Port | null;
  page: { title: string; url: string; platform: Platform | null } | null;
  transport: TransportMode | null;
  capture: CaptureSnapshot | null;
  ws: WebSocket | null;
  seq: SeqCounter;
  queue: Queued[];
  lastSentAt: number;
  armedAt: number;
  retryTimer?: ReturnType<typeof setTimeout>;
  pingTimer?: ReturnType<typeof setInterval>;
  tabCapture: PanelState["tabCapture"];
  tabCaptureTried: boolean;
  removed: boolean;
  /** Meet names (#131): the page's speaker state and clock mapping. */
  speakers: SpeakerRelay;
}

const tabs = new Map<number, Tab>();

function tabState(tabId: number): Tab {
  let t = tabs.get(tabId);
  if (!t) {
    t = {
      tabId,
      session: initialSession(),
      port: null,
      page: null,
      transport: null,
      capture: null,
      ws: null,
      seq: new SeqCounter(),
      queue: [],
      lastSentAt: 0,
      armedAt: 0,
      tabCapture: "off",
      tabCaptureTried: false,
      removed: false,
      speakers: new SpeakerRelay(),
    };
    tabs.set(tabId, t);
  }
  return t;
}

// ---- app check -------------------------------------------------------------------

let lastCheck: { at: number; result: AppCheck } | null = null;

/** `GET /app/version` with the stored pairing (#127's testConnection). */
async function checkApp(): Promise<AppCheck> {
  const pairing = await getPairing().catch(() => null);
  const result = toAppCheck(pairing ? await testConnection(pairing, { timeoutMs: CHECK_TIMEOUT_MS }) : null);
  lastCheck = { at: Date.now(), result };
  return result;
}

onPairingChanged(() => {
  lastCheck = null;
  for (const t of tabs.values()) void broadcastState(t);
});

// ---- the state machine's effects ---------------------------------------------------

function dispatch(t: Tab, ev: SessionEvent) {
  const before = t.session.phase;
  const { s, effects } = step(t.session, ev);
  t.session = s;
  for (const e of effects) run(t, e);
  if (s.phase !== before) updateBadge(t);
  void broadcastState(t);
  if (t.removed && !isCapturing(s) && s.phase !== "stopping" && s.phase !== "checking") forget(t);
}

function run(t: Tab, e: Effect) {
  switch (e.type) {
    case "check":
      void checkApp().then((result) => dispatch(t, { type: "check", result }));
      return;
    case "arm":
      void arm(t);
      return;
    case "disarm":
      toPage(t, { type: "disarm" });
      stopTabCapture(t);
      t.queue = [];
      // The page's observer stops with the capture (#131).
      t.speakers.clear();
      return;
    case "connect":
      void connect(t);
      return;
    case "send-start":
      sendStart(t);
      return;
    case "send-stop":
      sendText(t, { type: "stop" });
      return;
    case "close":
      closeSocket(t);
      return;
    case "schedule-retry":
      clearTimeout(t.retryTimer);
      t.retryTimer = setTimeout(() => dispatch(t, { type: "retry" }), e.ms);
      return;
    case "cancel-retry":
      clearTimeout(t.retryTimer);
      return;
  }
}

function toPage(t: Tab, m: FromBackground) {
  try {
    t.port?.postMessage(m);
  } catch {
    /* the page went away: its onDisconnect reports it */
  }
}

async function arm(t: Tab) {
  t.armedAt = Date.now();
  t.tabCaptureTried = false;
  t.tabCapture = "off";
  if (t.port) {
    toPage(t, { type: "arm", remote: true });
    return;
  }
  // Ask the page to open its port; `onConnect` then sends `arm`.
  try {
    await browser.tabs.sendMessage(t.tabId, { type: "page:connect" } satisfies ToPage);
  } catch {
    dispatch(t, {
      type: "arm-failed",
      error: "This tab is not a supported meeting page, or it was open before the extension was installed: reload it.",
    });
  }
}

async function connect(t: Tab) {
  const pairing = await getPairing().catch(() => null);
  if (t.session.phase !== "connecting") return;
  if (!pairing) {
    // Unpaired meanwhile: a failed connection; the retry's check says why.
    dispatch(t, { type: "ws-closed" });
    return;
  }
  closeSocket(t);
  let ws: WebSocket;
  try {
    ws = new WebSocket(liveUrl(pairing));
  } catch {
    dispatch(t, { type: "ws-closed" });
    return;
  }
  ws.binaryType = "arraybuffer";
  t.ws = ws;
  const mine = () => t.ws === ws;
  ws.onopen = () => mine() && dispatch(t, { type: "ws-open" });
  ws.onmessage = (e) => {
    if (!mine() || typeof e.data !== "string") return;
    let msg: Record<string, unknown>;
    try {
      msg = JSON.parse(e.data);
    } catch {
      return;
    }
    void broadcast({ type: "panel:live", tabId: t.tabId, message: msg });
    if (msg.type === "status") {
      dispatch(t, {
        type: "ws-status",
        state: String(msg.state),
        itemId: typeof msg.item_id === "string" ? msg.item_id : undefined,
        message: typeof msg.message === "string" ? msg.message : undefined,
      });
    }
  };
  let reported = false;
  const closed = () => {
    if (reported || !mine()) return;
    reported = true;
    t.ws = null;
    stopPing(t);
    dispatch(t, { type: "ws-closed" });
  };
  ws.onclose = closed;
  ws.onerror = closed;
}

function closeSocket(t: Tab) {
  const ws = t.ws;
  t.ws = null;
  stopPing(t);
  if (ws && ws.readyState <= WebSocket.OPEN) {
    try {
      ws.close(1000);
    } catch {
      /* ignore */
    }
  }
}

function sendText(t: Tab, m: unknown) {
  if (t.ws?.readyState !== WebSocket.OPEN) return;
  t.ws.send(JSON.stringify(m));
  t.lastSentAt = Date.now();
}

function sendStart(t: Tab) {
  const page = t.page;
  sendText(t, {
    type: "start",
    title: page?.title ?? "",
    url: page?.url ?? "",
    platform: page?.platform ?? "other",
    rate: t.session.rate,
    channels: 2,
  });
  // What the page already said about speakers goes to the new item first
  // (#131); timed events wait for this connection's first frame.
  for (const m of t.speakers.begin(t.session.rate ?? 48_000)) sendText(t, m);
  // A new connection is a new meeting on the app: `seq` restarts.
  t.seq = new SeqCounter();
  const queued = t.queue;
  t.queue = [];
  for (const q of queued) sendFrame(t, q);
  startPing(t);
}

function sendFrame(t: Tab, q: Queued) {
  const ws = t.ws;
  if (t.session.phase !== "live" || ws?.readyState !== WebSocket.OPEN) {
    t.queue.push(q);
    if (t.queue.length > MAX_QUEUE) t.queue.splice(0, t.queue.length - MAX_QUEUE);
    return;
  }
  const seq = t.seq.take(q.ch, q.producer, q.pseq);
  if (q.ch === CHANNEL.mic && q.producer === "page") {
    // The page's frames set the clock of its speaker events (#131).
    for (const m of t.speakers.frame(q.pseq, seq)) sendText(t, m);
  }
  if (ws.bufferedAmount > MAX_BUFFERED_BYTES) {
    // The app can't keep up: drop, and let the gap become silence.
    return;
  }
  ws.send(encodeFrame(q.ch, seq, q.buf));
  t.lastSentAt = Date.now();
}

function startPing(t: Tab) {
  stopPing(t);
  t.pingTimer = setInterval(() => {
    if (Date.now() - t.lastSentAt >= IDLE_PING_MS) sendText(t, { type: "ping" });
  }, IDLE_PING_MS / 2);
}

function stopPing(t: Tab) {
  clearInterval(t.pingTimer);
  t.pingTimer = undefined;
}

function forget(t: Tab) {
  closeSocket(t);
  clearTimeout(t.retryTimer);
  tabs.delete(t.tabId);
}

// ---- the page's port ------------------------------------------------------------------

function acceptsAudio(t: Tab): boolean {
  const p = t.session.phase;
  return p === "connecting" || p === "live" || p === "reconnecting";
}

function onPagePort(port: Runtime.Port) {
  const tab = port.sender?.tab;
  if (tab?.id === undefined || (port.sender?.frameId ?? 0) !== 0) {
    port.disconnect();
    return;
  }
  const t = tabState(tab.id);
  if (t.port && t.port !== port) t.port.disconnect();
  t.port = port;
  port.postMessage({ type: "hello", probe: makeProbe() } satisfies FromBackground);
  if (t.session.phase === "arming") port.postMessage({ type: "arm", remote: t.tabCapture !== "on" } satisfies FromBackground);

  port.onMessage.addListener((raw: unknown) => {
    const m = raw as ToBackground;
    switch (m.type) {
      case "armed":
        t.page = { title: m.title, url: m.url, platform: m.platform };
        t.transport = m.transport;
        dispatch(t, { type: "armed", rate: m.rate });
        return;
      case "arm-failed":
        dispatch(t, { type: "arm-failed", error: m.error });
        return;
      case "state":
        t.capture = m.state;
        maybeTabCapture(t);
        void broadcastState(t);
        return;
      case "speaker": {
        // Meet names (#131): kept per tab, sent on the connection's clock.
        const msg = sanitizePageSpeaker(m.msg);
        if (!msg || !acceptsAudio(t)) return;
        const live = t.session.phase === "live" && t.ws?.readyState === WebSocket.OPEN;
        for (const w of t.speakers.page(msg, live)) sendText(t, w);
        if (msg.type === "observer_health") void broadcastState(t);
        return;
      }
      case "pcm": {
        if (!acceptsAudio(t)) return;
        const mic = m.mic !== undefined ? decodePayload(m.mic) : null;
        const remote = m.remote !== undefined ? decodePayload(m.remote) : null;
        if (mic) sendFrame(t, { ch: CHANNEL.mic, producer: "page", pseq: m.seq, buf: mic });
        if (remote && t.tabCapture !== "on") sendFrame(t, { ch: CHANNEL.remote, producer: "page", pseq: m.seq, buf: remote });
        return;
      }
    }
  });
  port.onDisconnect.addListener(() => {
    if (t.port !== port) return;
    t.port = null;
    t.capture = null;
    t.speakers.clear();
    // The page unloaded (navigation, reload, tab closed, bfcache).
    dispatch(t, { type: "page-gone" });
  });
}

// ---- Chrome: tab capture through an offscreen document ---------------------------------


/** The tab whose audio the offscreen document captures (one at a time). */
let offscreenTab: number | null = null;

function maybeTabCapture(t: Tab) {
  const go = shouldTabCapture({
    browser: __BROWSER__,
    capturing: t.session.phase === "live" || t.session.phase === "connecting" || t.session.phase === "reconnecting",
    remoteVia: t.capture?.armed ? t.capture.remote.via : undefined,
    armedForMs: Date.now() - t.armedAt,
    alreadyTried: t.tabCaptureTried,
  });
  if (go) void startTabCapture(t);
}

async function startTabCapture(t: Tab) {
  t.tabCaptureTried = true;
  if (__BROWSER__ !== "chrome" || !chrome?.tabCapture || !chrome.offscreen || (offscreenTab !== null && offscreenTab !== t.tabId)) {
    t.tabCapture = "failed";
    return;
  }
  t.tabCapture = "starting";
  void broadcastState(t);
  try {
    const streamId = await chrome.tabCapture.getMediaStreamId({ targetTabId: t.tabId });
    const has = chrome.offscreen.hasDocument ? await chrome.offscreen.hasDocument() : false;
    if (!has) {
      await chrome.offscreen.createDocument({
        url: "offscreen.html",
        reasons: ["USER_MEDIA"],
        justification: "Capture the meeting tab's audio when the page offers no other way to get it.",
      });
    }
    offscreenTab = t.tabId;
    toPage(t, { type: "remote", on: false });
    t.tabCapture = "on";
    const msg: ToOffscreen = { type: "offscreen:start", target: "offscreen", streamId, rate: t.session.rate ?? 48_000 };
    await chrome.runtime.sendMessage(msg);
  } catch (e) {
    t.tabCapture = "failed";
    t.session = { ...t.session, message: `Tab audio capture failed: ${String(e)}` };
    toPage(t, { type: "remote", on: true });
    if (offscreenTab === t.tabId) stopTabCapture(t);
  }
  void broadcastState(t);
}

function stopTabCapture(t: Tab) {
  if (t.tabCapture === "on" || t.tabCapture === "starting") t.tabCapture = "off";
  // Compiled out of the Firefox build (no offscreen documents there).
  if (__BROWSER__ !== "chrome" || offscreenTab !== t.tabId || !chrome?.offscreen) return;
  offscreenTab = null;
  void chrome.runtime.sendMessage({ type: "offscreen:stop", target: "offscreen" } satisfies ToOffscreen).catch(() => {});
  void chrome.offscreen.closeDocument().catch(() => {});
}

function onOffscreenPort(port: Runtime.Port) {
  port.onMessage.addListener((raw: unknown) => {
    const m = raw as ToBackground;
    const t = offscreenTab !== null ? tabs.get(offscreenTab) : undefined;
    if (!t) return;
    if (m.type === "pcm" && m.remote !== undefined && acceptsAudio(t) && t.tabCapture === "on") {
      const remote = decodePayload(m.remote);
      if (remote) sendFrame(t, { ch: CHANNEL.remote, producer: "tab", pseq: m.seq, buf: remote });
    } else if (m.type === "offscreen-error") {
      t.tabCapture = "failed";
      t.session = { ...t.session, message: `Tab audio capture failed: ${m.error}` };
      toPage(t, { type: "remote", on: true });
      void broadcastState(t);
    }
  });
  port.postMessage({ type: "hello", probe: makeProbe() } satisfies FromBackground);
}

browser.runtime.onConnect.addListener((port) => {
  if (port.name === "capture") onPagePort(port);
  else if (port.name === "offscreen" && port.sender?.url?.startsWith(browser.runtime.getURL(""))) onOffscreenPort(port);
  else port.disconnect();
});

// ---- tabs going away ---------------------------------------------------------------------

browser.tabs.onRemoved.addListener((tabId) => {
  const t = tabs.get(tabId);
  if (!t) return;
  t.removed = true;
  dispatch(t, { type: "page-gone" });
});

// ---- badge -----------------------------------------------------------------------------------

function updateBadge(t: Tab) {
  if (t.removed) return;
  const p = t.session.phase;
  const text = isCapturing(t.session) ? "REC" : p === "stopping" ? "…" : p === "error" ? "!" : "";
  const color = p === "error" ? "#595959" : "#e52520";
  const title = isCapturing(t.session) ? "Sussurro — capturing this meeting" : "Sussurro";
  void browser.action.setBadgeText({ tabId: t.tabId, text }).catch(() => {});
  void browser.action.setBadgeBackgroundColor({ tabId: t.tabId, color }).catch(() => {});
  void browser.action.setTitle({ tabId: t.tabId, title }).catch(() => {});
}

// ---- side panel ---------------------------------------------------------------------------------

async function broadcast(m: PanelBroadcast) {
  try {
    await browser.runtime.sendMessage(m);
  } catch {
    /* no panel open */
  }
}

async function pageInfo(tabId: number): Promise<PageInfo | null> {
  try {
    return ((await browser.tabs.sendMessage(tabId, { type: "page:info" } satisfies ToPage)) as PageInfo) ?? null;
  } catch {
    return null;
  }
}

async function panelState(t: Tab, info?: PageInfo | null): Promise<PanelState> {
  const paired = !!(await getPairing().catch(() => null));
  const check = lastCheck?.result ?? null;
  const s = t.session;
  return {
    tabId: t.tabId,
    meetingPage: info === undefined ? !!t.port || !!t.page : !!info,
    platform: info?.platform ?? t.page?.platform ?? null,
    paired,
    app: check === null ? null : check.ok ? { ok: true, version: check.app } : { ok: false, problem: check.reason },
    phase: s.phase,
    problem: s.problem,
    message: s.message,
    itemId: s.itemId,
    serverState: s.serverState,
    attempt: s.attempt,
    capture: t.capture ?? info?.state ?? null,
    tabCapture: t.tabCapture,
    transport: t.transport,
    names: t.speakers.lastHealth,
  };
}

async function broadcastState(t: Tab) {
  await broadcast({ type: "panel:state", state: await panelState(t) });
}

browser.runtime.onMessage.addListener((raw: unknown, sender: Runtime.MessageSender) => {
  const m = raw as PanelRequest;
  // Only the extension's own pages drive capture (never a content script,
  // whose sender URL is the web page's).
  if (!m || typeof m !== "object" || !(sender.url ?? "").startsWith(browser.runtime.getURL(""))) return undefined;
  switch (m.type) {
    case "panel:get":
      return (async () => {
        const info = await pageInfo(m.tabId);
        const t = tabState(m.tabId);
        if (info && !t.page) t.page = { title: info.title, url: info.url, platform: info.platform };
        if (!lastCheck || Date.now() - lastCheck.at > CHECK_TTL_MS) await checkApp();
        return panelState(t, info);
      })();
    case "panel:start":
      dispatch(tabState(m.tabId), { type: "start" });
      return Promise.resolve(true);
    case "panel:stop":
      dispatch(tabState(m.tabId), { type: "stop" });
      return Promise.resolve(true);
    default:
      return undefined;
  }
});

