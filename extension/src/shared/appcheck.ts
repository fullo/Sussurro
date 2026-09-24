/* "Is the app there?" for the capture session (#128), on top of #127's
 * `testConnection` (`GET /app/version`, connection.ts) and the stored
 * pairing (pairing.ts). Pure — unit tested. */
import type { ConnectionResult } from "./connection";

/** Why the app can't take a meeting right now. */
export type AppProblem = "not_paired" | Exclude<ConnectionResult["kind"], "ok">;

export type AppCheck = { ok: true; app: string } | { ok: false; reason: AppProblem; detail?: string };

/** `null` = no pairing stored. */
export function toAppCheck(r: ConnectionResult | null): AppCheck {
  if (!r) return { ok: false, reason: "not_paired" };
  switch (r.kind) {
    case "ok":
      return { ok: true, app: r.app };
    case "protocol_mismatch":
      return { ok: false, reason: r.kind, detail: `app ${r.app} speaks protocol ${r.protocol}` };
    case "unexpected":
      return { ok: false, reason: r.kind, detail: `HTTP ${r.status}` };
    default:
      return { ok: false, reason: r.kind };
  }
}

/** Problems a retry won't fix: stop reconnecting and tell the user. The app
 *  not answering (not started yet, restarting) is worth retrying. */
export function isFatal(p: AppProblem): boolean {
  return p !== "not_running" && p !== "timeout" && p !== "unexpected";
}

/** One line for the side panel (the options page has the long versions). */
export function problemText(p: AppProblem): string {
  switch (p) {
    case "not_paired":
      return "Not paired with the Sussurro app.";
    case "not_running":
      return "The Sussurro app is not running (or listens on another port).";
    case "timeout":
      return "The Sussurro app does not answer.";
    case "blocked":
      return "The browser blocked the connection to Sussurro: check that meetings are on in its settings.";
    case "meetings_disabled":
      return "Meetings are off in Sussurro → Settings → Browser extension.";
    case "bad_token":
      return "Sussurro refused the token. Pair the extension again.";
    case "forbidden":
      return "Sussurro refused this extension.";
    case "protocol_mismatch":
      return "This extension and the Sussurro app are different versions. Update both.";
    case "unexpected":
      return "Something unexpected answered on Sussurro's port.";
  }
}
