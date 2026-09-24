/* ISOLATED-world content script: the extension side of a meeting page.
 *
 * - Handshake with the MAIN-world hook (shared/handshake.ts) for a private
 *   MessagePort.
 * - Answers the background's `page:info` (platform, title, capture state)
 *   and, on `page:connect` (the user pressed Start), opens the runtime port
 *   `capture` and relays: commands background → MAIN, audio and state
 *   MAIN → background.
 * - Audio crosses the runtime port binary-safe when the browser allows it
 *   (negotiated with the background's probe, shared/transport.ts), else as
 *   base64. Buffers from the page are copied into this world first
 *   (Firefox Xray wrappers).
 * - The runtime port closing (tab navigating, extension reloaded, service
 *   worker gone) disarms the hook: no capture without a listener. */
import browser, { type Runtime } from "webextension-polyfill";
import { offerPort } from "../shared/handshake";
import { detectPlatform } from "../shared/platform";
import { detectMode, encodePayload, ownBuffer, type TransportMode } from "../shared/transport";
import type { CaptureSnapshot, FromBackground, FromMain, PageInfo, ToBackground, ToMain, ToPage } from "../shared/messages";

const platform = detectPlatform(location.href);
const targetOrigin = location.origin === "null" ? "*" : location.origin;
const mainPort = offerPort(window, targetOrigin);

let state: CaptureSnapshot | null = null;
let bg: Runtime.Port | null = null;
let mode: TransportMode = "base64";
let keepalive: ReturnType<typeof setInterval> | undefined;

const toMain = (m: ToMain) => {
  void mainPort.then((p) => p.postMessage(m));
};

const toBg = (m: ToBackground) => {
  try {
    bg?.postMessage(m);
  } catch {
    // The port died between two messages: onDisconnect cleans up.
  }
};

function field(o: unknown, k: string): unknown {
  try {
    return (o as Record<string, unknown>)[k];
  } catch {
    return undefined;
  }
}

void mainPort.then((p) => {
  p.onmessage = (e: MessageEvent) => {
    const m = e.data as FromMain;
    switch (field(m, "t")) {
      case "pcm": {
        if (!bg) return;
        const mic = ownBuffer(field(m, "mic"));
        const rem = field(m, "remote");
        const remote = rem ? ownBuffer(rem) : null;
        const out: ToBackground = { type: "pcm", seq: Number(field(m, "seq")) };
        if (mic) out.mic = encodePayload(mode, mic);
        if (remote) out.remote = encodePayload(mode, remote);
        toBg(out);
        return;
      }
      case "state":
        // Plain data: a clone makes it this world's own (Firefox).
        state = structuredClone(field(m, "state")) as CaptureSnapshot;
        toBg({ type: "state", state });
        return;
      case "armed":
        toBg({ type: "armed", rate: Number(field(m, "rate")), title: document.title, url: location.href, platform });
        return;
      case "arm-failed":
        toBg({ type: "arm-failed", error: String(field(m, "error")) });
        return;
    }
  };
  toMain({ t: "state?" });
});

function drop(port: Runtime.Port) {
  if (bg !== port) return;
  bg = null;
  clearInterval(keepalive);
}

function connect() {
  if (bg) return;
  const port = browser.runtime.connect({ name: "capture" });
  bg = port;
  port.onMessage.addListener((raw: unknown) => {
    const m = raw as FromBackground;
    switch (m.type) {
      case "hello":
        mode = detectMode(m.probe);
        return;
      case "arm":
        toMain({ t: "arm", remote: m.remote });
        return;
      case "disarm":
        // Capture is over: close the port too, so an idle meeting tab does
        // not keep the background awake. Start reconnects it.
        toMain({ t: "disarm" });
        drop(port);
        port.disconnect();
        return;
      case "remote":
        toMain({ t: "remote", on: m.on });
        return;
    }
  });
  port.onDisconnect.addListener(() => {
    if (bg !== port) return;
    drop(port);
    toMain({ t: "disarm" });
  });
  // The hook's watchdog stops capturing if we go quiet.
  keepalive = setInterval(() => toMain({ t: "state?" }), 2000);
}

browser.runtime.onMessage.addListener((raw: unknown) => {
  const m = raw as ToPage;
  if (!m || typeof m !== "object") return undefined;
  if (m.type === "page:info") {
    const info: PageInfo = { platform, title: document.title, url: location.href, state };
    return Promise.resolve(info);
  }
  if (m.type === "page:connect") {
    connect();
    return Promise.resolve(true);
  }
  return undefined;
});
