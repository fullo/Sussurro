/* Pairing with the local app and the "is the app there?" check. Pure —
 * unit tested; the I/O (storage, fetch) lives in the background.
 *
 * The pairing flow (#127: token generated in the app, pasted into the
 * options page) stores `{port, token}` under `storage.local["pairing"]`.
 * Until then the side panel shows "not paired". */

import { PROTOCOL_VERSION } from "./frame";

/** `storage.local` key holding the {@link Pairing}. */
export const PAIRING_KEY = "pairing";
/** The app's default local API port (`Settings.api_port`). */
export const DEFAULT_PORT = 4525;

export interface Pairing {
  port: number;
  /** The extension token (64 hex characters from the app). */
  token: string;
}

/** A stored value → a usable pairing, or null (not paired, or garbage). */
export function parsePairing(raw: unknown): Pairing | null {
  if (!raw || typeof raw !== "object") return null;
  const r = raw as Record<string, unknown>;
  const token = typeof r.token === "string" ? r.token.trim() : "";
  if (!token || /\s/.test(token)) return null;
  const port = r.port === undefined ? DEFAULT_PORT : Number(r.port);
  if (!Number.isInteger(port) || port < 1 || port > 65535) return null;
  return { port, token };
}

/** `GET /app/version`: the handshake (needs `Authorization: Bearer`). */
export function versionUrl(p: Pairing): string {
  return `http://127.0.0.1:${p.port}/app/version`;
}

/** `WS /live`: browsers can't set headers on a WebSocket, hence `?token=`. */
export function liveUrl(p: Pairing): string {
  return `ws://127.0.0.1:${p.port}/live?token=${encodeURIComponent(p.token)}`;
}

/** Outcome of the app check, as the side panel words it. */
export type AppCheck =
  | { ok: true; app: string }
  | { ok: false; reason: AppProblem; detail?: string };

export type AppProblem =
  /** No pairing stored yet. */
  | "not-paired"
  /** Nothing answers on the port (connection refused / timeout). */
  | "not-running"
  /** 404: the app runs but meetings are off (`meetings_enabled`), or it is too old. */
  | "meetings-disabled"
  /** 401: wrong or regenerated token. */
  | "bad-token"
  /** 403: the app refused this extension's origin. */
  | "forbidden"
  /** Another `/live` protocol version. */
  | "protocol-mismatch"
  /** Anything else (5xx, garbage). */
  | "error";

/** Problems a retry won't fix: stop reconnecting and tell the user. */
export function isFatal(p: AppProblem): boolean {
  return p !== "not-running" && p !== "error";
}

/** Classify a `GET /app/version` result. `status` null = the fetch itself
 *  failed (network error: the app is not running). */
export function classifyVersion(status: number | null, body: unknown): AppCheck {
  if (status === null) return { ok: false, reason: "not-running" };
  if (status === 401) return { ok: false, reason: "bad-token" };
  if (status === 403) return { ok: false, reason: "forbidden" };
  if (status === 404) return { ok: false, reason: "meetings-disabled" };
  if (status !== 200) return { ok: false, reason: "error", detail: `HTTP ${status}` };
  const b = (body && typeof body === "object" ? body : {}) as Record<string, unknown>;
  if (b.protocol !== PROTOCOL_VERSION) {
    return { ok: false, reason: "protocol-mismatch", detail: `app speaks protocol ${String(b.protocol)}, extension ${PROTOCOL_VERSION}` };
  }
  return { ok: true, app: typeof b.app === "string" ? b.app : "?" };
}

/** One line for the side panel. */
export function problemText(p: AppProblem): string {
  switch (p) {
    case "not-paired":
      return "Not paired with the Sussurro app. Pair it in the extension options.";
    case "not-running":
      return "The Sussurro app is not running (or listens on another port).";
    case "meetings-disabled":
      return "Meeting capture is turned off in the Sussurro app.";
    case "bad-token":
      return "The app refused the extension token. Pair the extension again.";
    case "forbidden":
      return "The app refused this browser extension.";
    case "protocol-mismatch":
      return "This extension and the Sussurro app are different versions. Update both.";
    case "error":
      return "The Sussurro app answered with an error.";
  }
}
