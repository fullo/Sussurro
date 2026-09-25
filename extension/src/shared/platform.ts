/* Which meeting platform a page belongs to. Must agree with the
   content_scripts `matches` of both manifests (checked in the tests). */

export type Platform = "meet" | "teams" | "zoom";

/** Meeting pages whose call runs in the top frame: the content scripts run
 *  there only. Teams keeps both of its old hosts; organisational tenants
 *  move to `teams.cloud.microsoft` (Microsoft's deadline 2026-09-30, #287). */
export const TOP_FRAME_MATCHES: readonly string[] = [
  "https://meet.google.com/*",
  "https://teams.microsoft.com/*",
  "https://teams.live.com/*",
  "https://teams.cloud.microsoft/*",
];

/** Meeting pages whose call runs in a same-origin iframe (Zoom's web client
 *  under `/wc/`): the content scripts also run in the page's frames that
 *  match (`all_frames`), and the background captures one frame per tab
 *  (#287). */
export const ALL_FRAMES_MATCHES: readonly string[] = ["https://*.zoom.us/wc/*"];

/** The meeting pages the content scripts run on (manifest match patterns,
 *  host permissions). */
export const MEETING_MATCHES: readonly string[] = [...TOP_FRAME_MATCHES, ...ALL_FRAMES_MATCHES];

/** The local Sussurro app (loopback only, any port). */
export const APP_MATCH = "http://127.0.0.1/*";

/** Platform of a meeting page URL, or null for anything else (including
 *  look-alike hosts and non-https pages). */
export function detectPlatform(url: string): Platform | null {
  let u: URL;
  try {
    u = new URL(url);
  } catch {
    return null;
  }
  if (u.protocol !== "https:") return null;
  const host = u.hostname.toLowerCase();
  if (host === "meet.google.com") return "meet";
  if (host === "teams.microsoft.com" || host === "teams.live.com" || host === "teams.cloud.microsoft") return "teams";
  if ((host === "zoom.us" || host.endsWith(".zoom.us")) && (u.pathname === "/wc" || u.pathname.startsWith("/wc/"))) {
    return "zoom";
  }
  return null;
}
