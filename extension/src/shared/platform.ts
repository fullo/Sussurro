/* Which meeting platform a page belongs to. Must agree with the
   content_scripts `matches` of both manifests (checked in the tests). */

export type Platform = "meet" | "teams" | "zoom";

/** The meeting pages the content scripts run on (manifest match patterns). */
export const MEETING_MATCHES: readonly string[] = [
  "https://meet.google.com/*",
  "https://teams.microsoft.com/*",
  "https://teams.live.com/*",
  "https://*.zoom.us/wc/*",
];

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
  if (host === "teams.microsoft.com" || host === "teams.live.com") return "teams";
  if ((host === "zoom.us" || host.endsWith(".zoom.us")) && (u.pathname === "/wc" || u.pathname.startsWith("/wc/"))) {
    return "zoom";
  }
  return null;
}
