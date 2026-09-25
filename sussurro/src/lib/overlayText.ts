/* Live preview in the overlay (#98), pure helpers.

   The preview loop re-transcribes the whole recording every ~1.2 s and emits
   the text as `partial-transcript`. Whisper keeps revising the last words it
   heard, so the overlay tells the settled part (unchanged across two passes,
   drawn solid) from the provisional tail (drawn as ghost text), and shows only
   the end of a long dictation. */

/** Mic RMS (0..1) → VU percentage on a −60…0 dBFS scale; 0 when silent. */
export function levelToPercent(rms: number): number {
  if (!(rms > 0)) return 0;
  return Math.max(0, Math.min(100, ((20 * Math.log10(rms) + 60) / 60) * 100));
}

/** Byte offsets where each word of `text` starts (a word = a run of non-space). */
function wordStarts(text: string): number[] {
  const starts: number[] = [];
  const re = /\S+/g;
  let m: RegExpExecArray | null;
  while ((m = re.exec(text)) !== null) starts.push(m.index);
  return starts;
}

/** `text` split in two at a word boundary: the settled head and the ghost tail. */
export interface PreviewSplit {
  settled: string;
  ghost: string;
}

/**
 * Split the new partial `next` against the previous one `prev`: the leading
 * words both passes agree on are settled; the rest — plus the last
 * `holdBack` words, which the next pass may still revise even when they
 * matched — is ghost. `settled + ghost === next`.
 */
export function splitSettled(prev: string, next: string, holdBack = 2): PreviewSplit {
  const a = prev.match(/\S+/g) ?? [];
  const bStarts = wordStarts(next);
  const b = next.match(/\S+/g) ?? [];
  let common = 0;
  while (common < a.length && common < b.length && a[common] === b[common]) common++;
  const settledWords = Math.max(0, Math.min(common, b.length - holdBack));
  if (settledWords === 0) return { settled: "", ghost: next };
  const cut = settledWords < b.length ? bStarts[settledWords] : next.length;
  return { settled: next.slice(0, cut), ghost: next.slice(cut) };
}

/** What the overlay draws: the tail of the text, split into settled and ghost. */
export interface PreviewView extends PreviewSplit {
  /** Earlier text was cut off: draw a leading ellipsis. */
  clipped: boolean;
}

/**
 * Keep only the last ~`maxChars` of a long preview (the overlay has room for
 * two or three lines, and the newest words are the ones worth seeing),
 * cutting at a word boundary. A single word longer than `maxChars` is kept
 * whole rather than chopped.
 */
export function tailView(split: PreviewSplit, maxChars = 160): PreviewView {
  const full = split.settled + split.ghost;
  if (full.length <= maxChars) return { ...split, clipped: false };
  let cut = full.length - maxChars;
  // Move forward to the start of the next word, so no word is cut in half.
  if (cut > 0 && /\S/.test(full[cut - 1]) && /\S/.test(full[cut])) {
    const next = full.slice(cut).search(/\s/);
    cut = next === -1 ? full.lastIndexOf(" ", cut) + 1 : cut + next;
  }
  while (cut < full.length && /\s/.test(full[cut])) cut++;
  if (cut <= 0 || cut >= full.length) return { ...split, clipped: false };
  const s = split.settled.length;
  return {
    settled: cut < s ? split.settled.slice(cut) : "",
    ghost: cut < s ? split.ghost : split.ghost.slice(cut - s),
    clipped: true,
  };
}
