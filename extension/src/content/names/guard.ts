/* The one guard every display name from a meeting page passes (#131;
 * Teams/Zoom #245/#246): a name goes to the app only if it looks like a
 * person's name. Rejected: empty or overlong strings, strings without a
 * letter (timers, counters), Meet's internal ids (`spaces/…`, `devices/…`),
 * the user's own placeholder ("You", localized), "Unknown user" (Teams,
 * localized) and names cut with an ellipsis (Zoom truncates long names:
 * two people sharing the start of a name would become one). Whitespace is
 * collapsed.
 *
 * Qualifiers the platforms append in parentheses — "(Guest)",
 * "(Unverified)", "(External)" on Teams, "(Host)", "(Co-host)", "(me)" on
 * Zoom, in the UI languages Sussurro targets first — are removed: they
 * are not part of the person's name, and the app matches names against
 * People (#132). Two people who differ only by a qualifier become
 * namesakes, which the binder never binds. A parenthesised group that is
 * not only qualifiers (an organisation, a nickname) stays. Pure — unit
 * tested. */

/** Longest name kept (the app's MAX_NAME_CHARS). */
export const MAX_NAME = 120;

/** "You" as a page may show it for the user's own tile, in the UI
 *  languages Sussurro targets first. Compared case-insensitively. */
const SELF_WORDS = new Set(["you", "tu", "voi", "te", "du", "vous", "toi", "tú", "usted", "sie", "ich", "me", "io"]);

/** Qualifiers stripped from the end of a name (lower case). */
const QUALIFIERS = new Set([
  // guest
  "guest", "ospite", "gast", "invité", "invitée", "invitado", "invitada", "convidado",
  // unverified
  "unverified", "non verificato", "non verificata", "nicht verifiziert", "non vérifié", "non vérifiée", "no verificado", "no verificada",
  // external
  "external", "esterno", "esterna", "extern", "externe", "externo", "externa",
  // host roles (Zoom)
  "host", "co-host", "cohost", "organizer", "organizzatore", "organizzatrice", "moderatore", "moderatrice", "gastgeber", "co-gastgeber", "hôte", "co-hôte", "anfitrión", "coanfitrión",
  // the user's own tile (Zoom)
  "me", "io", "ich", "moi", "yo", "you", "tu", "du",
]);

/** A whole name meaning "not known" (Teams' anonymous callers). */
const UNKNOWN = new Set(["unknown", "unknown user", "utente sconosciuto", "sconosciuto", "unbekannter benutzer", "unbekannt", "utilisateur inconnu", "inconnu", "usuario desconocido", "desconocido"]);

/** Remove trailing "(Guest)", "(Host, me)", … groups made only of known
 *  qualifiers. */
function stripQualifiers(n: string): string {
  for (;;) {
    const m = /^(.*\S)\s*\(([^()]*)\)$/u.exec(n);
    if (!m) return n;
    const parts = m[2].split(",").map((p) => p.trim().toLowerCase());
    if (!parts.length || !parts.every((p) => QUALIFIERS.has(p))) return n;
    n = m[1];
  }
}

export function cleanName(raw: string | null | undefined): string | null {
  if (typeof raw !== "string") return null;
  let n = raw.replace(/\s+/g, " ").trim();
  if (!n || n.length > MAX_NAME) return null;
  if (/(…|\.\.\.)$/u.test(n)) return null;
  n = stripQualifiers(n);
  if (!/\p{L}/u.test(n)) return null;
  if (/^(spaces|devices|conferences|participants)\//i.test(n)) return null;
  if (/^\d{1,2}:\d{2}/.test(n)) return null;
  const bare = n.replace(/[()]/g, "").trim().toLowerCase();
  if (SELF_WORDS.has(bare) || UNKNOWN.has(bare)) return null;
  if (bare.split(",").every((p) => QUALIFIERS.has(p.trim()))) return null;
  return n;
}

/** Identity of a name (case, accents and spacing ignored), for duplicates. */
export function nameKey(n: string): string {
  return n
    .normalize("NFKD")
    .replace(/\p{M}/gu, "")
    .toLowerCase()
    .replace(/\s+/g, " ")
    .trim();
}

/** A short stable id for a tile's participant id, safe for the protocol
 *  (`tile:<hex>`): page ids contain slashes and can be long. FNV-1a. */
export function tileKey(raw: string): string {
  let h = 0x811c9dc5;
  for (let i = 0; i < raw.length; i++) {
    h ^= raw.charCodeAt(i);
    h = Math.imul(h, 0x01000193) >>> 0;
  }
  return `tile:${h.toString(16).padStart(8, "0")}`;
}
