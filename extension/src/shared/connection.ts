/* "Test connection" on the options page (#127): `GET /app/version` with the
   token, and what the answer means for the user. The request and the
   mapping are separate so the mapping can be tested without a network.

   What the app answers (sussurro/src-tauri/src/api):
   - 200 `{app, protocol}`  → paired; compatible when `protocol` matches ours
   - 401                     → wrong or missing token (or never paired in the app)
   - 403                     → origin refused (should not happen from the extension)
   - 404                     → meetings are off (E12: the route doesn't exist), or
                               a Sussurro older than 0.9
   - connection refused      → the app isn't running, or its local API is off,
                               or it listens on another port */
import { PROTOCOL_VERSION, appUrl, authHeaders, type Pairing } from "./pairing";

export type ConnectionResult =
  | { kind: "ok"; app: string; protocol: number }
  | { kind: "protocol_mismatch"; app: string; protocol: number }
  | { kind: "not_running" }
  | { kind: "timeout" }
  /** Something answers on the port but the browser blocked the reply
   *  (no host permission for 127.0.0.1, or a CORS preflight refused — as
   *  when meetings are off and the extension lacks host access). */
  | { kind: "blocked" }
  | { kind: "bad_token" }
  | { kind: "forbidden" }
  | { kind: "meetings_disabled" }
  | { kind: "unexpected"; status: number };

/** Map an HTTP answer of `GET /app/version`. Pure. */
export function classifyResponse(status: number, body: unknown): ConnectionResult {
  if (status === 401) return { kind: "bad_token" };
  if (status === 403) return { kind: "forbidden" };
  if (status === 404) return { kind: "meetings_disabled" };
  if (status === 200 && body && typeof body === "object") {
    const { app, protocol } = body as { app?: unknown; protocol?: unknown };
    if (typeof app === "string" && typeof protocol === "number") {
      return protocol === PROTOCOL_VERSION ? { kind: "ok", app, protocol } : { kind: "protocol_mismatch", app, protocol };
    }
  }
  return { kind: "unexpected", status };
}

type Fetch = (input: string, init?: RequestInit) => Promise<Response>;

/** Call `GET /app/version`. Never throws; the token is sent only in the
 *  `Authorization` header and never appears in the result. */
export async function testConnection(
  p: Pairing,
  { fetchImpl = fetch, timeoutMs = 4000 }: { fetchImpl?: Fetch; timeoutMs?: number } = {},
): Promise<ConnectionResult> {
  const url = appUrl(p.port, "/app/version");
  let res: Response;
  try {
    res = await fetchImpl(url, {
      headers: authHeaders(p),
      cache: "no-store",
      credentials: "omit",
      signal: AbortSignal.timeout(timeoutMs),
    });
  } catch (e) {
    if (e instanceof DOMException && (e.name === "TimeoutError" || e.name === "AbortError")) {
      return { kind: "timeout" };
    }
    // A network error: nothing listening, or the browser blocked the reply.
    // An opaque probe (no headers, so no CORS preflight) tells them apart.
    try {
      await fetchImpl(url, { mode: "no-cors", cache: "no-store", credentials: "omit", signal: AbortSignal.timeout(timeoutMs) });
      return { kind: "blocked" };
    } catch {
      return { kind: "not_running" };
    }
  }
  let body: unknown = null;
  try {
    body = await res.json();
  } catch {
    // Not JSON: something other than Sussurro, or an unexpected error page.
  }
  return classifyResponse(res.status, body);
}

export interface ResultMessage {
  ok: boolean;
  title: string;
  detail: string;
}

/** What to tell the user. Pure; never includes the token. */
export function describeResult(r: ConnectionResult, port: number): ResultMessage {
  switch (r.kind) {
    case "ok":
      return {
        ok: true,
        title: `Connected to Sussurro ${r.app}`,
        detail: `Protocol ${r.protocol}, compatible with this extension. Meetings can be recorded.`,
      };
    case "protocol_mismatch":
      return {
        ok: false,
        title: `Sussurro ${r.app} speaks another protocol`,
        detail:
          r.protocol > PROTOCOL_VERSION
            ? `Sussurro uses protocol ${r.protocol}, this extension ${PROTOCOL_VERSION}: update the extension to the version that matches the app.`
            : `Sussurro uses protocol ${r.protocol}, this extension ${PROTOCOL_VERSION}: update Sussurro to the version that matches the extension.`,
      };
    case "not_running":
      return {
        ok: false,
        title: "Sussurro is not reachable",
        detail: `Nothing answers on 127.0.0.1:${port}. Start Sussurro and check Settings → Browser extension: the local API must be on (it starts with the app) and use port ${port}.`,
      };
    case "timeout":
      return {
        ok: false,
        title: "No answer from Sussurro",
        detail: `Something is listening on 127.0.0.1:${port} but did not answer in time. Is it Sussurro? Check the port in Settings → Browser extension.`,
      };
    case "blocked":
      return {
        ok: false,
        title: "The browser blocked the connection",
        detail:
          "Sussurro seems to be running, but the reply was blocked. Check that meetings are enabled in Sussurro → Settings → Browser extension, and that this extension may access 127.0.0.1 (its site access / permissions).",
      };
    case "bad_token":
      return {
        ok: false,
        title: "Wrong token",
        detail:
          "Sussurro refused the token: it was regenerated, or the code comes from another computer. Copy the pairing code again from Sussurro → Settings → Browser extension.",
      };
    case "forbidden":
      return {
        ok: false,
        title: "Sussurro refused this extension",
        detail: "The request's origin was not accepted. Reload the extension and try again.",
      };
    case "meetings_disabled":
      return {
        ok: false,
        title: "Meetings are off in Sussurro",
        detail:
          "Turn on Meetings in Sussurro → Settings → Browser extension. (If there is no such setting, update Sussurro: meetings need version 0.9 or later.)",
      };
    case "unexpected":
      return {
        ok: false,
        title: "Unexpected answer",
        detail: `Port ${port} answered with HTTP ${r.status}, not like Sussurro. Check the port in Sussurro → Settings → Browser extension.`,
      };
  }
}
