/* ---------- Cleanup endpoint (privacy) ---------- */

/** Host + transport of a user-entered cleanup endpoint URL (scheme optional).
 *  Mirrors `is_local_endpoint()` in the Rust backend (settings.rs). */
export function parseEndpoint(url: string): { host: string; secure: boolean } | null {
  const s = url.trim();
  if (!s) return null;
  // Drop an optional "scheme://" prefix so bare hosts parse too.
  const sep = s.indexOf("://");
  const rest = sep >= 0 ? s.slice(sep + 3) : s;
  // Authority only: up to the first path/query/fragment character.
  const authorityRaw = rest.split(/[/?#]/)[0] ?? "";
  if (!authorityRaw) return null;
  // Strip userinfo ("user@host"), then the port — bracketed IPv6 literals
  // keep their address inside the brackets.
  const at = authorityRaw.lastIndexOf("@");
  const authority = at >= 0 ? authorityRaw.slice(at + 1) : authorityRaw;
  if (!authority) return null;
  let host: string;
  if (authority.startsWith("[")) {
    const close = authority.indexOf("]");
    if (close < 0) return null;
    host = authority.slice(1, close);
  } else {
    const colon = authority.lastIndexOf(":");
    host = colon >= 0 ? authority.slice(0, colon) : authority;
  }
  host = host.toLowerCase();
  if (!host) return null;
  return { host, secure: /^https:/i.test(s) };
}

const LOCAL_ENDPOINT_HOSTS = new Set(["localhost", "127.0.0.1", "::1"]);

/** Whether the cleanup endpoint stays on this machine (mirrors is_local_endpoint in Rust). */
export function isLocalEndpoint(url: string): boolean {
  const e = parseEndpoint(url);
  return !!e && (LOCAL_ENDPOINT_HOSTS.has(e.host) || e.host.endsWith(".local"));
}
