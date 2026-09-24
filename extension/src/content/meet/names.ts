/* The one guard every display name from the Meet page passes (#131): a
 * name goes to the app only if it looks like a person's name. Rejected:
 * empty or overlong strings, strings without a letter (timers, counters),
 * Meet's internal ids (`spaces/…`, `devices/…`), and the user's own
 * placeholder ("You", localized). Whitespace is collapsed; the display
 * string is otherwise kept as shown. Pure — unit tested. */

/** Longest name kept (the app's MAX_NAME_CHARS). */
export const MAX_NAME = 120;

/** "You" as Meet may show it for the user's own tile, in the UI languages
 *  Sussurro targets first. Compared case-insensitively. */
const SELF_WORDS = new Set(["you", "tu", "voi", "te", "du", "vous", "toi", "tú", "usted", "sie", "ich", "me", "io"]);

export function cleanName(raw: string | null | undefined): string | null {
  if (typeof raw !== "string") return null;
  const n = raw.replace(/\s+/g, " ").trim();
  if (!n || n.length > MAX_NAME) return null;
  if (!/\p{L}/u.test(n)) return null;
  if (/^(spaces|devices|conferences|participants)\//i.test(n)) return null;
  if (/^\d{1,2}:\d{2}/.test(n)) return null;
  const bare = n.replace(/[()]/g, "").trim().toLowerCase();
  if (SELF_WORDS.has(bare)) return null;
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
 *  (`tile:<hex>`): Meet's ids contain slashes and can be long. FNV-1a. */
export function tileKey(raw: string): string {
  let h = 0x811c9dc5;
  for (let i = 0; i < raw.length; i++) {
    h ^= raw.charCodeAt(i);
    h = Math.imul(h, 0x01000193) >>> 0;
  }
  return `tile:${h.toString(16).padStart(8, "0")}`;
}
