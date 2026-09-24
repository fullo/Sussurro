/* The pairing code of the browser extension (#127, E6): one string the user
   copies from Settings → Browser extension and pastes into the extension's
   options page. It carries the local API port and the extension token:

       sussurro:<port>:<token>

   Shared with the extension, which imports it as `@sussurro/pairing` (Vite
   alias + tsconfig path, like `@sussurro/transcript`): one definition of
   the format on both sides. Pure — no imports. */

export interface Pairing {
  /** Local API port on 127.0.0.1. */
  port: number;
  /** Extension token: 64 lowercase hex characters (32 random bytes). */
  token: string;
}

export const PAIRING_PREFIX = "sussurro";

/** What the app generates: `api::auth::generate_token` (32 bytes, hex). */
const TOKEN_RE = /^[0-9a-f]{64}$/;

export type Parsed<T> = { ok: true; value: T } | { ok: false; error: string };

/** A TCP port (1–65535) from a number or its decimal text; null otherwise. */
export function parsePort(v: string | number): number | null {
  const s = String(v).trim();
  if (!/^\d{1,5}$/.test(s)) return null;
  const n = Number(s);
  return n >= 1 && n <= 65535 ? n : null;
}

/** The token in canonical (lowercase) form, or null when it is not one the
 *  app could have generated — typically a truncated copy. */
export function normalizeToken(v: string): string | null {
  const t = v.trim().toLowerCase();
  return TOKEN_RE.test(t) ? t : null;
}

export function encodePairingCode(p: Pairing): string {
  return `${PAIRING_PREFIX}:${p.port}:${p.token}`;
}

/** Parse a pasted pairing code. Tolerates surrounding whitespace, quotes and
 *  line breaks (a copy from a chat or an e-mail), and any case. The error
 *  messages never repeat the token. */
export function parsePairingCode(text: string): Parsed<Pairing> {
  const s = text.replace(/\s+/g, "").replace(/^["'`]+|["'`]+$/g, "");
  if (!s) return { ok: false, error: "Paste the pairing code from Sussurro." };
  const parts = s.split(":");
  if (parts[0].toLowerCase() !== PAIRING_PREFIX || parts.length !== 3) {
    return {
      ok: false,
      error: "This is not a Sussurro pairing code: it starts with “sussurro:”. Copy it again from Sussurro → Settings → Browser extension.",
    };
  }
  const port = parsePort(parts[1]);
  if (port === null) return { ok: false, error: "The port in the pairing code is not valid (1–65535)." };
  const token = normalizeToken(parts[2]);
  if (token === null) {
    return { ok: false, error: "The token in the pairing code is incomplete or damaged. Copy the code again." };
  }
  return { ok: true, value: { port, token } };
}

/** Validate a port and a token entered separately. */
export function validatePairing(port: string | number, token: string): Parsed<Pairing> {
  const p = parsePort(port);
  if (p === null) return { ok: false, error: "The port must be a number between 1 and 65535." };
  const t = normalizeToken(token);
  if (t === null) return { ok: false, error: "The token must be the 64 characters shown by Sussurro." };
  return { ok: true, value: { port: p, token: t } };
}

/** For display: only the last four characters, never the whole token. */
export function maskToken(token: string): string {
  const t = token.trim();
  if (!t) return "";
  return `••••••••${t.length > 8 ? t.slice(-4) : ""}`;
}

/** The pairing code with the token masked, for display next to the Copy button. */
export function maskedPairingCode(port: number): string {
  return `${PAIRING_PREFIX}:${port}:••••••••`;
}
