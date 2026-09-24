/* The side panel's item actions (#129) on the app's token routes (#126):
 *
 * - `POST /items/{id}/open` — "Open in Sussurro": the app comes to the front
 *   on the item;
 * - `GET /items/{id}/export?format=txt|srt` — "Copy as text" and "Create
 *   .srt" (the same exports as the app's Export menu).
 *
 * Item ids contain `/` (`2026/09/2026-09-24-weekly-sync`); the app takes
 * them as they are between `/items/` and the action. The token travels
 * only in the `Authorization` header, never in an error. */
import { appUrl, authHeaders, type Pairing } from "./pairing";

export type ExportFormat = "txt" | "srt";

export type ItemResult<T> = { ok: true; value: T } | { ok: false; error: string };

type Fetch = (input: string, init?: RequestInit) => Promise<Response>;

const TIMEOUT_MS = 8000;

/** `/items/<id>/<action>`, each path segment escaped. Pure. */
export function itemPath(id: string, action: "open" | "export"): string {
  return `/items/${id.split("/").map(encodeURIComponent).join("/")}/${action}`;
}

/** The download name: the item's folder name plus the extension. Pure. */
export function exportFilename(id: string, format: ExportFormat): string {
  const folder = id.split("/").filter(Boolean).pop() ?? "transcript";
  return `${folder.replace(/[\\/:*?"<>|]/g, "_") || "transcript"}.${format}`;
}

/** An answer that is not 2xx → words for the user. Pure. */
export function describeFailure(status: number, body: unknown): string {
  const appSays = body && typeof body === "object" && typeof (body as { error?: unknown }).error === "string" ? (body as { error: string }).error : "";
  switch (status) {
    case 401:
      return "Sussurro refused the token. Pair the extension again.";
    case 403:
      return "Sussurro refused this extension.";
    case 404:
      return appSays && appSays !== "unknown endpoint"
        ? "Sussurro can't find this item: it was deleted or moved."
        : "This Sussurro can't do that: update the app.";
    case 422:
      return appSays ? `Sussurro can't export it: ${appSays}.` : "Sussurro can't export this item.";
    default:
      return `Sussurro answered with HTTP ${status}.`;
  }
}

async function call(p: Pairing, path: string, method: "GET" | "POST", fetchImpl: Fetch): Promise<ItemResult<Response>> {
  let res: Response;
  try {
    res = await fetchImpl(appUrl(p.port, path), {
      method,
      headers: authHeaders(p),
      cache: "no-store",
      credentials: "omit",
      signal: AbortSignal.timeout(TIMEOUT_MS),
    });
  } catch {
    return { ok: false, error: "The Sussurro app is not reachable." };
  }
  if (res.ok) return { ok: true, value: res };
  let body: unknown = null;
  try {
    body = await res.json();
  } catch {
    /* not JSON */
  }
  return { ok: false, error: describeFailure(res.status, body) };
}

/** "Open in Sussurro". */
export async function openItem(p: Pairing, id: string, fetchImpl: Fetch = fetch): Promise<ItemResult<true>> {
  const r = await call(p, itemPath(id, "open"), "POST", fetchImpl);
  return r.ok ? { ok: true, value: true } : r;
}

/** The item exported as `format` (text). */
export async function exportItem(p: Pairing, id: string, format: ExportFormat, fetchImpl: Fetch = fetch): Promise<ItemResult<string>> {
  const r = await call(p, `${itemPath(id, "export")}?format=${format}`, "GET", fetchImpl);
  if (!r.ok) return r;
  try {
    return { ok: true, value: await r.value.text() };
  } catch {
    return { ok: false, error: "The export from Sussurro was cut short." };
  }
}
