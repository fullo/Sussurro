/* Settings → Scripting (#249, E14): archive API tokens. The backend keeps
   only a SHA-256 of each token (`api/tokens.rs`); the UI lists them by id,
   name, scopes and times, and sees a token's plaintext once, right after
   `archive_token_create`. Pure. */

/** What an archive token may do (`api::tokens::Scope`). */
export type ArchiveScope = "read" | "people" | "write";

/** A token as listed (`archive_tokens_list`): never the token, never its hash. */
export interface ArchiveTokenInfo {
  id: string;
  name: string;
  scopes: ArchiveScope[];
  /** RFC 3339, UTC. */
  created: string;
  last_used: string | null;
}

/** `archive_token_create`'s answer: the only time the token is shown. */
export interface NewArchiveToken {
  token: string;
  info: ArchiveTokenInfo;
}

export const SCOPES: { id: ArchiveScope; label: string; hint: string }[] = [
  { id: "read", label: "Read", hint: "list, search, read and export items and documents; people's names" },
  { id: "people", label: "People's emails", hint: "adds participants' and People's emails to what Read returns" },
  { id: "write", label: "Write", hint: "create a note from text" },
];

/** Same limits as the backend (`MAX_NAME_CHARS`, `MAX_TOKENS`). */
export const MAX_NAME_CHARS = 64;
export const MAX_TOKENS = 32;

/** Why `name` can't name a new token, or null. Mirrors
 *  `tokens::validate_name` so the button can say it before the round trip. */
export function tokenNameError(name: string, existing: ArchiveTokenInfo[]): string | null {
  const n = name.trim();
  if (!n) return "Give the token a name (what script uses it).";
  if ([...n].length > MAX_NAME_CHARS) return `At most ${MAX_NAME_CHARS} characters.`;
  // eslint-disable-next-line no-control-regex
  if (/[\u0000-\u001f\u007f-\u009f]/.test(n)) return "No control characters.";
  if (existing.some((t) => t.name.toLowerCase() === n.toLowerCase())) return `A token named “${n}” already exists.`;
  if (existing.length >= MAX_TOKENS) return `At most ${MAX_TOKENS} tokens: revoke one you no longer use.`;
  return null;
}

/** Scopes in a fixed order, without duplicates. */
export function normalizeScopes(scopes: ArchiveScope[]): ArchiveScope[] {
  return SCOPES.map((s) => s.id).filter((id) => scopes.includes(id));
}

/** "read · write" for the list. */
export function scopeSummary(scopes: ArchiveScope[]): string {
  return normalizeScopes(scopes).join(" · ");
}

/** A tick in the create form, toggled. `people` needs `read` to be useful,
 *  so ticking it ticks `read` too (unticking `read` leaves it — the backend
 *  accepts any non-empty set). */
export function toggleScope(scopes: ArchiveScope[], scope: ArchiveScope, on: boolean): ArchiveScope[] {
  const next = on ? [...scopes, scope] : scopes.filter((s) => s !== scope);
  if (on && scope === "people" && !next.includes("read")) next.push("read");
  return normalizeScopes(next);
}

/** A first request to try, with the token in an environment variable (so
 *  it doesn't end up in the shell history). */
export function curlExample(port: number): string {
  return `curl -H "Authorization: Bearer $SUSSURRO_TOKEN" http://127.0.0.1:${port}/archive/items`;
}

/** What the Archive API row says under its switch. */
export function archiveApiStatus(apiEnabled: boolean, apiArchive: boolean, tokens: number): string {
  if (!apiArchive) return "off: every /archive request is refused";
  if (!apiEnabled) return "on, but the local API is off (Behavior → Advanced)";
  if (tokens === 0) return "on: create a token below for your script";
  return `on · ${tokens} token${tokens === 1 ? "" : "s"}`;
}
