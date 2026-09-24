/* One tab's capture session as a state machine. Pure — unit tested; the
 * background glue (index.ts) runs the effects (fetch, WebSocket, messages
 * to the page) and feeds the results back as events.
 *
 *   idle ─start→ checking ─ok→ arming ─armed→ connecting ─open→ live ─stop→ stopping ─done→ done
 *                   │not ok        │failed         │closed          │closed
 *                   ↓              ↓               ↓                ↓
 *                 error          error        reconnecting ←────────┘  (backoff, then check:
 *                                                                   not running → retry,
 *                                                                   fatal → error)
 *
 * Capture only starts on an explicit `start` (the side panel's Start); the
 * page's audio graph exists from `armed` to the `disarm` effect. A socket
 * that closes during a meeting means the app ended that meeting (it keeps
 * what it got); the reconnect sends a new `start`, i.e. a new item. */
import { backoffDelay } from "../shared/backoff";
import { isFatal, type AppCheck, type AppProblem } from "../shared/appcheck";

export type Phase = "idle" | "checking" | "arming" | "connecting" | "live" | "reconnecting" | "stopping" | "done" | "error";

export interface Session {
  phase: Phase;
  /** Reconnect attempts since the socket was last open. */
  attempt: number;
  /** The page's AudioContext rate, from `armed`. */
  rate?: number;
  problem?: AppProblem;
  /** Human-readable error or last warning. */
  message?: string;
  /** The app's item for the current (or last) meeting. */
  itemId?: string;
  /** Last `status.state` from the app. */
  serverState?: string;
}

export type SessionEvent =
  | { type: "start" }
  | { type: "check"; result: AppCheck }
  | { type: "armed"; rate: number }
  | { type: "arm-failed"; error: string }
  | { type: "ws-open" }
  | { type: "ws-status"; state: string; itemId?: string; message?: string }
  | { type: "ws-closed" }
  | { type: "retry" }
  | { type: "stop" }
  /** The tab closed or navigated away (the content script is gone). */
  | { type: "page-gone" };

export type Effect =
  | { type: "check" }
  | { type: "arm" }
  | { type: "disarm" }
  | { type: "connect" }
  | { type: "send-start" }
  | { type: "send-stop" }
  | { type: "close" }
  | { type: "schedule-retry"; ms: number }
  | { type: "cancel-retry" };

export const initialSession = (): Session => ({ phase: "idle", attempt: 0 });

/** Is the page's audio being captured (for the badge)? */
export function isCapturing(s: Session): boolean {
  return s.phase === "arming" || s.phase === "connecting" || s.phase === "live" || s.phase === "reconnecting";
}

/** Must the background stay loaded for this session? From Start until the
 *  app's `done` (including `stopping`, while the app finishes and the
 *  socket may carry nothing for a while): unloading it would drop the
 *  WebSocket it owns and the panel's transcript. */
export function holdsBackground(s: Session): boolean {
  return isCapturing(s) || s.phase === "checking" || s.phase === "stopping";
}

export function step(s: Session, ev: SessionEvent, random: () => number = Math.random): { s: Session; effects: Effect[] } {
  const same = { s, effects: [] as Effect[] };
  const to = (patch: Partial<Session>, ...effects: Effect[]) => ({ s: { ...s, ...patch }, effects });
  const retry = (patch: Partial<Session> = {}) => {
    const attempt = s.attempt + 1;
    return to({ ...patch, phase: "reconnecting", attempt }, { type: "schedule-retry", ms: backoffDelay(attempt, undefined, random) });
  };

  switch (ev.type) {
    case "start":
      if (s.phase !== "idle" && s.phase !== "done" && s.phase !== "error") return same;
      return { s: { phase: "checking", attempt: 0 }, effects: [{ type: "check" }] };

    case "check":
      if (s.phase === "checking") {
        return ev.result.ok
          ? to({ phase: "arming" }, { type: "arm" })
          : to({ phase: "error", problem: ev.result.reason, message: ev.result.detail });
      }
      if (s.phase === "reconnecting") {
        if (ev.result.ok) return to({ phase: "connecting", problem: undefined }, { type: "connect" });
        if (isFatal(ev.result.reason)) {
          return to({ phase: "error", problem: ev.result.reason, message: ev.result.detail }, { type: "disarm" });
        }
        return retry({ problem: ev.result.reason });
      }
      return same;

    case "armed":
      if (s.phase !== "arming") return { s, effects: [{ type: "disarm" }] }; // stopped meanwhile
      return to({ phase: "connecting", rate: ev.rate }, { type: "connect" });

    case "arm-failed":
      if (s.phase !== "arming") return same;
      return to({ phase: "error", message: ev.error });

    case "ws-open":
      if (s.phase !== "connecting") return { s, effects: [{ type: "close" }] }; // stale socket
      return to({ phase: "live", attempt: 0, problem: undefined, itemId: undefined, serverState: undefined }, { type: "send-start" });

    case "ws-status": {
      const patch: Partial<Session> = { serverState: ev.state };
      if (ev.itemId) patch.itemId = ev.itemId;
      if (ev.state === "warning" && ev.message) patch.message = ev.message;
      if (ev.state === "done") return to({ ...patch, phase: "done" }, { type: "close" });
      if (ev.state === "error") {
        const stopping = s.phase === "stopping";
        return to({ ...patch, phase: "error", message: ev.message }, ...(stopping ? [] : [{ type: "disarm" } as Effect]), { type: "close" });
      }
      return to(patch);
    }

    case "ws-closed":
      if (s.phase === "live" || s.phase === "connecting") return retry();
      if (s.phase === "stopping") return to({ phase: "done" });
      return same;

    case "retry":
      return s.phase === "reconnecting" ? { s, effects: [{ type: "check" }] } : same;

    case "stop":
      switch (s.phase) {
        case "checking":
          return to({ phase: "idle" });
        case "arming":
          return to({ phase: "idle" }, { type: "disarm" });
        case "connecting":
        case "reconnecting":
          return to({ phase: "idle" }, { type: "disarm" }, { type: "close" }, { type: "cancel-retry" });
        case "live":
          return to({ phase: "stopping" }, { type: "disarm" }, { type: "send-stop" });
        default:
          return same;
      }

    case "page-gone":
      switch (s.phase) {
        case "live":
          return to({ phase: "stopping" }, { type: "send-stop" });
        case "checking":
        case "arming":
        case "connecting":
        case "reconnecting":
          return to({ phase: "idle" }, { type: "close" }, { type: "cancel-retry" });
        default:
          return same;
      }
  }
}

/** Chrome's last fallback: capture the tab's audio for the remote channel
 *  when, some seconds into a capture, the page shows no remote audio at all
 *  (no peer-connection track, no media element) — e.g. a client that
 *  decodes audio in WebAssembly and plays it through WebAudio. Firefox has
 *  no `tabCapture`. */
export const TAB_CAPTURE_AFTER_MS = 4000;

export function shouldTabCapture(o: {
  browser: "chrome" | "firefox";
  capturing: boolean;
  remoteVia: string | undefined;
  armedForMs: number;
  alreadyTried: boolean;
}): boolean {
  return o.browser === "chrome" && o.capturing && !o.alreadyTried && o.remoteVia === "none" && o.armedForMs >= TAB_CAPTURE_AFTER_MS;
}
