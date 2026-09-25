/* Background worker (Chrome: service worker, Firefox: event page) — #128.
 *
 * Owns, per tab, the capture session (session.ts) and the WebSocket to the
 * app's `/live` (ws://127.0.0.1:<port>/live, port and token from the
 * pairing, pairing.ts; the token goes as the socket's first message when
 * the app supports it, else as `?token=`). It:
 *
 * - checks the app with `GET /app/version` before starting and before each
 *   reconnect ("app not running", bad token, app too old, other protocol);
 * - on the side panel's Start, asks the page to connect and arm the hook,
 *   opens the socket, sends `start {title, url, platform, rate, channels,
 *   language?}` (the language chosen in the panel, #288, kept for the
 *   reconnects of that meeting) and forwards the audio frames, numbering
 *   `seq` per connection;
 * - when the scripts run in several frames of the tab (Zoom's meeting
 *   iframe, #287), arms only the frame with the call and sends only its
 *   audio (frames.ts): one capture per tab, shown in the tab's panel;
 * - buffers audio while (re)connecting (bounded), reconnects with backoff,
 *   sends `ping` when no audio went out for a while;
 * - sends `stop` on Stop, tab close or navigation;
 * - shows a REC badge on the toolbar button of a tab being captured;
 * - relays the Meet page's speaker events (#131) on the connection's
 *   audio clock, and replays what the page already said to a new
 *   connection (shared/speakerEvents.ts);
 * - caps what the meeting page sends (#217): speaker events per second,
 *   audio blocks checked (counter, size), repeated participant lists
 *   dropped (shared/ratelimit.ts, shared/speakerEvents.ts);
 * - (Chrome) keeps `storage.local` away from content scripts where the
 *   browser allows it (Chrome ≥ 140; the pairing itself is not there);
 * - (Chrome) falls back to `tabCapture` through an offscreen document for
 *   the remote channel when the page shows no remote audio at all;
 * - keeps each tab's live transcript (the app's `segment` / `speaker` /
 *   `status` messages, `shared/live.ts`) for the side panel, which takes a
 *   snapshot and then follows the broadcast changes (#129);
 * - stays loaded from Start until the app's `done` (keep-alive, #137).
 *
 * Nothing is captured before an explicit Start. */
import browser, { type Runtime } from "webextension-polyfill";
import { CHANNEL, SeqCounter, encodeFrame, type ChannelByte } from "../shared/frame";
import { getPairing, liveAuth, liveUrl, onPairingChanged } from "../shared/pairing";
import { restrictToTrustedContexts } from "../shared/secureStore";
import { BACKGROUND_EVENTS, MAX_BLOCK_BYTES, RateLimiter, pageSeq } from "../shared/ratelimit";
import { testConnection } from "../shared/connection";
import { toAppCheck, type AppCheck } from "../shared/appcheck";
import { decodePayload, makeProbe, type TransportMode } from "../shared/transport";
import type { CaptureSnapshot, FromBackground, PageInfo, PanelBroadcast, PanelRequest, PanelState, ToBackground, ToOffscreen, ToPage } from "../shared/messages";
import { detectPlatform, type Platform } from "../shared/platform";
import { applyLive, initialTranscript, parseAppMessage, type LiveAction, type LiveTranscript } from "../shared/live";
import { canTakeStart, holdsBackground, initialSession, isCapturing, shouldTabCapture, step, type Effect, type SessionEvent, type Session } from "./session";
import { pickCaptureFrame } from "./frames";
import { SpeakerRelay, sanitizePageSpeaker } from "../shared/speakerEvents";
import { isLanguageCode } from "../shared/language";

// ---- storage.local: trusted contexts only (Chrome ≥ 140, #217) ------------------

// It holds no secret any more (the pairing is in the secure store), but no
// content script needs it either. Chrome forgets the setting when the
// browser restarts: every start of the background sets it again.
if (__BROWSER__ === "chrome") void restrictToTrustedContexts(chrome?.storage?.local);

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

/** One frame of a meeting tab running the content scripts (#287: Zoom's
 *  meeting iframe as well as the top frame). */
interface Frame {
  /** Its runtime port (while connected). */
  port: Runtime.Port;
  /** Its last capture snapshot (null: not reported yet). */
  capture: CaptureSnapshot | null;
}

interface Tab {
  tabId: number;
  session: Session;
  /** The tab's frames with an open port, by frame id (0 = top frame). */
  frames: Map<number, Frame>;
  /** The one frame that is armed and whose audio is sent (frames.ts);
   *  null before one is picked and after disarm. */
  captureFrame: number | null;
  /** When the current Start began arming (frames.ts waits a little). */
  armStartedAt: number;
  pickTimer?: ReturnType<typeof setTimeout>;
  page: { title: string; url: string; platform: Platform | null } | null;
  transport: TransportMode | null;
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
  /** Speaker events from the page per second (#217). */
  speakerRate: RateLimiter;
  /** What the side panel shows (#129). */
  transcript: LiveTranscript;
  /** The meeting's language chosen at Start (#288); null: none sent (the
   *  app uses its dictation language). */
  language: string | null;
}

/** Names this background's transcripts: after a restart (Chrome may stop an
 *  idle service worker) the panel sees another epoch and asks again. */
const EPOCH = Math.random().toString(36).slice(2, 10);

const tabs = new Map<number, Tab>();

function tabState(tabId: number): Tab {
  let t = tabs.get(tabId);
  if (!t) {
    t = {
      tabId,
      session: initialSession(),
      frames: new Map(),
      captureFrame: null,
      armStartedAt: 0,
      page: null,
      transport: null,
      ws: null,
      seq: new SeqCounter(),
      queue: [],
      lastSentAt: 0,
      armedAt: 0,
      tabCapture: "off",
      tabCaptureTried: false,
      removed: false,
      speakers: new SpeakerRelay(),
      speakerRate: new RateLimiter(BACKGROUND_EVENTS.burst, BACKGROUND_EVENTS.perSec),
      transcript: initialTranscript(EPOCH),
      language: null,
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
  // A new Start is a new meeting: the panel starts from a blank page.
  if (ev.type === "start" && s.phase === "checking" && before !== "checking") live(t, { kind: "reset" });
  for (const e of effects) run(t, e);
  if (s.phase !== before) {
    updateBadge(t);
    updateKeepAlive();
  }
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
      // Every frame: only the capture frame is armed, but each closes its
      // port on `disarm`, so an idle meeting tab doesn't keep us loaded.
      for (const f of t.frames.values()) post(f.port, { type: "disarm" });
      t.captureFrame = null;
      clearTimeout(t.pickTimer);
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

function post(port: Runtime.Port, m: FromBackground) {
  try {
    port.postMessage(m);
  } catch {
    /* the page went away: its onDisconnect reports it */
  }
}

/** To the capture frame only. */
function toPage(t: Tab, m: FromBackground) {
  const f = t.captureFrame !== null ? t.frames.get(t.captureFrame) : undefined;
  if (f) post(f.port, m);
}

/** The snapshot the panel and the tab-capture fallback look at: the
 *  capture frame's, else the top frame's. */
function captureOf(t: Tab): CaptureSnapshot | null {
  return t.frames.get(t.captureFrame ?? 0)?.capture ?? null;
}

async function arm(t: Tab) {
  t.armedAt = t.armStartedAt = Date.now();
  t.tabCaptureTried = false;
  t.tabCapture = "off";
  t.captureFrame = null;
  pickFrame(t);
  // Ask every frame of the tab running our scripts to open its port (a
  // frame already connected keeps it) and report what it sees: pickFrame
  // then arms the one with the call (#287).
  try {
    await browser.tabs.sendMessage(t.tabId, { type: "page:connect" } satisfies ToPage);
  } catch {
    if (t.session.phase !== "arming" || t.frames.size) return;
    dispatch(t, {
      type: "arm-failed",
      error: "This tab is not a supported meeting page, or it was open before the extension was installed: reload it.",
    });
  }
}

/** Arm the frame the call runs in (frames.ts), or look again once the
 *  frames had time to report. One capture frame per tab: a frame given up
 *  (it showed no call) is disarmed first, and only the capture frame's
 *  audio is sent. */
function pickFrame(t: Tab) {
  clearTimeout(t.pickTimer);
  t.pickTimer = undefined;
  if (!isCapturing(t.session)) return;
  const views = [...t.frames].map(([frameId, f]) => ({ frameId, capture: f.capture }));
  const pick = pickCaptureFrame(views, t.captureFrame, Date.now() - t.armStartedAt);
  if ("wait" in pick) {
    t.pickTimer = setTimeout(() => pickFrame(t), pick.wait);
    return;
  }
  if ("none" in pick || pick.frame === t.captureFrame) return;
  const old = t.captureFrame !== null ? t.frames.get(t.captureFrame) : undefined;
  if (old) post(old.port, { type: "disarm" });
  t.captureFrame = pick.frame;
  // The tab-capture fallback counts from when this frame was armed.
  t.armedAt = Date.now();
  toPage(t, { type: "arm", remote: t.tabCapture !== "on" });
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
  // An app that takes the token as the first message (#217) never sees it
  // in the URL, which Chrome logs when a connection fails.
  const firstMessage = lastCheck?.result.ok === true && lastCheck.result.liveAuth === "message";
  let ws: WebSocket;
  try {
    ws = new WebSocket(liveUrl(pairing, firstMessage));
  } catch {
    dispatch(t, { type: "ws-closed" });
    return;
  }
  ws.binaryType = "arraybuffer";
  t.ws = ws;
  const mine = () => t.ws === ws;
  ws.onopen = () => {
    if (!mine()) return;
    if (firstMessage) ws.send(liveAuth(pairing));
    dispatch(t, { type: "ws-open" });
  };
  ws.onmessage = (e) => {
    if (!mine() || typeof e.data !== "string") return;
    let msg: Record<string, unknown>;
    try {
      msg = JSON.parse(e.data);
    } catch {
      return;
    }
    const app = parseAppMessage(msg);
    if (app) live(t, { kind: "app", msg: app, at: Date.now() });
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
    // #288: an app older than it ignores the field.
    ...(t.language ? { language: t.language } : {}),
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
  clearTimeout(t.pickTimer);
  tabs.delete(t.tabId);
  updateKeepAlive();
}

// ---- staying loaded while a meeting is on ---------------------------------------------

/** Both browsers unload an idle background after ~30 s: Chrome its service
 *  worker, Firefox its event page (MV3 has no persistent background there).
 *  What counts as activity differs: Chrome ≥ 116 counts WebSocket traffic,
 *  Firefox only extension events and API calls — not the socket, so a
 *  quiet stretch (the app finishing a backlog after Stop, say) would unload
 *  the page and drop the socket and the panel's transcript. From Start until
 *  the app's `done`, a trivial API call every second resets the idle timer
 *  in both (the harness runs Firefox with a 2 s timeout); nothing is kept
 *  alive otherwise. */
const KEEP_ALIVE_MS = 1000;
let keepAliveTimer: ReturnType<typeof setInterval> | undefined;

function updateKeepAlive() {
  const hold = [...tabs.values()].some((t) => holdsBackground(t.session));
  if (hold && keepAliveTimer === undefined) {
    keepAliveTimer = setInterval(() => void browser.runtime.getPlatformInfo().catch(() => {}), KEEP_ALIVE_MS);
  } else if (!hold && keepAliveTimer !== undefined) {
    clearInterval(keepAliveTimer);
    keepAliveTimer = undefined;
  }
}

// ---- the page's port ------------------------------------------------------------------

function acceptsAudio(t: Tab): boolean {
  const p = t.session.phase;
  return p === "connecting" || p === "live" || p === "reconnecting";
}

function onPagePort(port: Runtime.Port) {
  const tab = port.sender?.tab;
  // Any frame the content scripts run in (Zoom's meeting iframe, #287);
  // the manifests inject them only into meeting pages.
  const frameId = port.sender?.frameId ?? 0;
  if (tab?.id === undefined) {
    port.disconnect();
    return;
  }
  const t = tabState(tab.id);
  const prev = t.frames.get(frameId);
  if (prev && prev.port !== port) prev.port.disconnect();
  const frame: Frame = { port, capture: null };
  t.frames.set(frameId, frame);
  const mine = () => t.frames.get(frameId) === frame;
  const capturing = () => mine() && t.captureFrame === frameId;
  post(port, { type: "hello", probe: makeProbe() });
  // The capture frame reconnected while arming: arm it again. Other frames
  // are picked from the snapshot they send right after connecting.
  if (frameId === t.captureFrame && t.session.phase === "arming") post(port, { type: "arm", remote: t.tabCapture !== "on" });

  port.onMessage.addListener((raw: unknown) => {
    if (!mine()) return;
    const m = raw as ToBackground;
    switch (m.type) {
      case "armed": {
        // A frame given up meanwhile was already told to disarm.
        if (!capturing() || !Number.isFinite(m.rate) || m.rate <= 0) return;
        // The meeting is the tab's page, whichever frame runs the call.
        const url = frameId === 0 ? m.url : (tab.url ?? m.url);
        t.page = { title: frameId === 0 ? m.title : (tab.title ?? m.title), url, platform: detectPlatform(url) ?? m.platform };
        t.transport = m.transport;
        if (t.session.phase === "arming") dispatch(t, { type: "armed", rate: m.rate });
        else if (t.session.rate !== undefined && m.rate !== t.session.rate) {
          // Picked after Start (frames.ts) with another audio rate than the
          // one the app was told: never seen in one tab, but say so.
          t.session = { ...t.session, message: `The meeting frame's audio rate changed (${t.session.rate} → ${m.rate} Hz): press Stop, then Start again.` };
          void broadcastState(t);
        }
        return;
      }
      case "arm-failed":
        if (capturing()) dispatch(t, { type: "arm-failed", error: m.error });
        return;
      case "state":
        frame.capture = m.state;
        pickFrame(t);
        if (frameId === (t.captureFrame ?? 0)) {
          maybeTabCapture(t);
          void broadcastState(t);
        }
        return;
      case "speaker": {
        // Meet names (#131): kept per tab, sent on the connection's clock.
        const msg = sanitizePageSpeaker(m.msg);
        if (!capturing() || !msg || !acceptsAudio(t) || !t.speakerRate.allow()) return;
        const live = t.session.phase === "live" && t.ws?.readyState === WebSocket.OPEN;
        for (const w of t.speakers.page(msg, live)) sendText(t, w);
        if (msg.type === "observer_health") void broadcastState(t);
        return;
      }
      case "pcm": {
        const pseq = pageSeq(m.seq);
        // One capturing frame per tab: never another frame's audio (#287).
        if (!capturing() || !acceptsAudio(t) || pseq === null) return;
        const block = (p: typeof m.mic) => {
          if (typeof p === "string" && p.length > MAX_BLOCK_BYTES * 2) return null;
          const b = p !== undefined ? decodePayload(p) : null;
          return b && b.byteLength <= MAX_BLOCK_BYTES && b.byteLength % 2 === 0 ? b : null;
        };
        const mic = block(m.mic);
        const remote = block(m.remote);
        if (mic) sendFrame(t, { ch: CHANNEL.mic, producer: "page", pseq, buf: mic });
        if (remote && t.tabCapture !== "on") sendFrame(t, { ch: CHANNEL.remote, producer: "page", pseq, buf: remote });
        return;
      }
    }
  });
  port.onDisconnect.addListener(() => {
    if (!mine()) return;
    t.frames.delete(frameId);
    if (frameId === t.captureFrame) {
      // The captured page unloaded (navigation, reload, tab closed, bfcache).
      t.captureFrame = null;
      t.speakers.clear();
      dispatch(t, { type: "page-gone" });
    } else if (t.captureFrame === null && !t.frames.size) {
      // The last frame went before one was picked (or after disarm).
      dispatch(t, { type: "page-gone" });
    } else {
      pickFrame(t);
    }
  });
}

// ---- Chrome: tab capture through an offscreen document ---------------------------------


/** The tab whose audio the offscreen document captures (one at a time). */
let offscreenTab: number | null = null;

function maybeTabCapture(t: Tab) {
  const go = shouldTabCapture({
    browser: __BROWSER__,
    capturing: t.session.phase === "live" || t.session.phase === "connecting" || t.session.phase === "reconnecting",
    remoteVia: captureOf(t)?.armed ? captureOf(t)?.remote.via : undefined,
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
  else if (__BROWSER__ === "chrome" && port.name === "offscreen" && port.sender?.url?.startsWith(browser.runtime.getURL(""))) onOffscreenPort(port);
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
    meetingPage: info === undefined ? t.frames.size > 0 || !!t.page : !!info,
    platform: info?.platform ?? t.page?.platform ?? null,
    paired,
    app: check === null ? null : check.ok ? { ok: true, version: check.app, subtitles: check.subtitles } : { ok: false, problem: check.reason },
    phase: s.phase,
    problem: s.problem,
    message: s.message,
    itemId: s.itemId,
    serverState: s.serverState,
    attempt: s.attempt,
    capture: captureOf(t) ?? info?.state ?? null,
    tabCapture: t.tabCapture,
    transport: t.transport,
    names: t.speakers.lastHealth,
    language: t.language,
  };
}

/** Apply a change to the tab's transcript and tell the open panels. */
function live(t: Tab, action: LiveAction) {
  t.transcript = applyLive(t.transcript, action);
  void broadcast({ type: "panel:live", tabId: t.tabId, epoch: t.transcript.epoch, rev: t.transcript.rev, action });
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
    case "panel:transcript":
      return Promise.resolve(tabs.get(m.tabId)?.transcript ?? initialTranscript(EPOCH));
    case "panel:start": {
      const t = tabState(m.tabId);
      // The language is fixed for the meeting: a Start while one runs
      // (ignored by the session) doesn't change it.
      if (canTakeStart(t.session)) t.language = isLanguageCode(m.language) ? m.language : null;
      dispatch(t, { type: "start" });
      return Promise.resolve(true);
    }
    case "panel:stop":
      dispatch(tabState(m.tabId), { type: "stop" });
      return Promise.resolve(true);
    default:
      return undefined;
  }
});

