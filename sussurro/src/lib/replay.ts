/* Per-speaker replay (0.10, #142): the pure logic behind the Audio tab.
   The player (lib/replayPlayer.ts) and the component only wire these to
   <audio> elements.

   Timelines. Every saved file starts at the session's t = 0 (#141), so a
   segment's start_ms is also its position in its channel's file: the
   "real" time. A playlist is a list of spans of real time to play back to
   back; its "virtual" timeline is those spans laid end to end — what the
   seek bar shows. With "All speakers" the playlist is one span covering the
   whole recording, so virtual = real.

   Two-channel items (mic + remote, mic + system). "All speakers" plays both
   files together, kept in sync, so the browser mixes them (a conversation
   sounds like the call did). "Play only: X" plays each of X's segments from
   its own channel's file only — the other side's crosstalk is not heard.
   Mixing through Web Audio was not chosen: a MediaElementSource on audio
   from another origin (the sussurro-audio: scheme) is muted unless served
   with CORS, and two elements in sync need neither. */

import type { Segment, Word } from "./types";

/** Playback speeds offered (0.75–2×). */
export const REPLAY_RATES = [0.75, 1, 1.25, 1.5, 1.75, 2] as const;
/** ← / → and the skip buttons. */
export const SKIP_MS = 10_000;
/** A speaker's segments closer than this are played as one span (no seek
 *  for a breath; longer gaps — someone else talking — are skipped). */
export const MERGE_GAP_MS = 250;

/** Segment fields the replay needs. */
export type ReplaySegment = Pick<Segment, "id" | "start_ms" | "end_ms" | "channel" | "speaker_id" | "text" | "stt_error" | "words">;

/** The file a segment's audio is in: `audio.wav` for a single-channel item
 *  (whatever the segment's channel), else `audio-<channel>.wav`. A lone
 *  file of another name (a channel file not renamed, #141) serves every
 *  segment. null = that channel was not saved. Pure. */
export function channelFile(channel: Segment["channel"], files: string[]): string | null {
  if (files.length === 0) return null;
  if (files.includes("audio.wav")) return "audio.wav";
  const own = `audio-${channel ?? "mic"}.wav`;
  if (files.includes(own)) return own;
  return files.length === 1 ? files[0] : null;
}

/** The path handed to `convertFileSrc(…, "sussurro-audio")`: the backend
 *  splits it at the last "/" into item id and file name. */
export function audioSrcPath(itemId: string, file: string): string {
  return `${itemId}/${file}`;
}

/** Whether a segment is a transcript line (the same rule as toLines). */
export function isLine(s: Pick<Segment, "text" | "stt_error">): boolean {
  return !!(s.text.trim() || s.stt_error);
}

export interface PlayEntry {
  /** Real time span, [start_ms, end_ms). */
  start_ms: number;
  end_ms: number;
  /** Files to play during the span (sorted). */
  files: string[];
  /** Where the span starts on the virtual timeline. */
  offset_ms: number;
}

export interface Playlist {
  /** null = all speakers (the whole recording). */
  speakerId: string | null;
  entries: PlayEntry[];
  /** Length of the virtual timeline. */
  duration_ms: number;
}

/** The playlist for "All speakers" (`speakerId` null) or "Play only:
 *  <speaker>". `totalMs`: the recording's length as far as known (the
 *  frontmatter duration, the files' durations); the last segment's end
 *  counts too. A speaker's lines become spans in time order, each played
 *  from its channel's file; spans that overlap or are closer than
 *  `mergeGapMs` are joined (a join across channels plays both files).
 *  Lines whose channel was not saved are left out. Pure. */
export function buildPlaylist(
  segments: ReplaySegment[],
  files: string[],
  totalMs: number,
  speakerId: string | null,
  mergeGapMs = MERGE_GAP_MS,
): Playlist {
  const sorted = [...files].sort();
  if (sorted.length === 0) return { speakerId, entries: [], duration_ms: 0 };
  if (speakerId === null) {
    const end = Math.max(0, totalMs || 0, ...segments.map((s) => s.end_ms));
    const entries = end > 0 ? [{ start_ms: 0, end_ms: end, files: sorted, offset_ms: 0 }] : [];
    return { speakerId, entries, duration_ms: end };
  }
  const spans = segments
    .filter((s) => s.speaker_id === speakerId && isLine(s) && s.end_ms > s.start_ms)
    .map((s) => ({ start: s.start_ms, end: s.end_ms, file: channelFile(s.channel, sorted) }))
    .filter((s): s is { start: number; end: number; file: string } => s.file !== null)
    .sort((a, b) => a.start - b.start || a.end - b.end);
  const entries: PlayEntry[] = [];
  for (const s of spans) {
    const last = entries[entries.length - 1];
    if (last && s.start <= last.end_ms + mergeGapMs) {
      last.end_ms = Math.max(last.end_ms, s.end);
      if (!last.files.includes(s.file)) last.files = [...last.files, s.file].sort();
    } else {
      entries.push({ start_ms: s.start, end_ms: s.end, files: [s.file], offset_ms: 0 });
    }
  }
  let offset = 0;
  for (const e of entries) {
    e.offset_ms = offset;
    offset += e.end_ms - e.start_ms;
  }
  return { speakerId, entries, duration_ms: offset };
}

/** Index of the last entry starting at or before `realMs` (-1 = none). */
function lastStartingBy(entries: PlayEntry[], realMs: number): number {
  let lo = 0;
  let hi = entries.length - 1;
  let found = -1;
  while (lo <= hi) {
    const mid = (lo + hi) >> 1;
    if (entries[mid].start_ms <= realMs) {
      found = mid;
      lo = mid + 1;
    } else hi = mid - 1;
  }
  return found;
}

/** Real time → position on the playlist's timeline. A time in a skipped
 *  gap maps to where the next span starts. Pure. */
export function toVirtual(pl: Playlist, realMs: number): number {
  const i = lastStartingBy(pl.entries, realMs);
  if (i < 0) return 0;
  const e = pl.entries[i];
  return e.offset_ms + Math.min(realMs, e.end_ms) - e.start_ms;
}

/** Where to play for real time `realMs`: the span containing it, else the
 *  next span (the gap is skipped). null = past the last span. Pure. */
export function locate(pl: Playlist, realMs: number): { index: number; ms: number } | null {
  const i = lastStartingBy(pl.entries, realMs);
  if (i >= 0 && realMs < pl.entries[i].end_ms) return { index: i, ms: Math.max(realMs, pl.entries[i].start_ms) };
  const next = i + 1;
  if (next < pl.entries.length) return { index: next, ms: pl.entries[next].start_ms };
  return null;
}

/** Position on the playlist's timeline → span and real time (clamped to
 *  the playlist; the very end maps to the end of the last span). Pure. */
export function fromVirtual(pl: Playlist, virtualMs: number): { index: number; ms: number } | null {
  const n = pl.entries.length;
  if (n === 0) return null;
  const v = Math.min(Math.max(0, virtualMs), pl.duration_ms);
  let lo = 0;
  let hi = n - 1;
  let i = 0;
  while (lo <= hi) {
    const mid = (lo + hi) >> 1;
    if (pl.entries[mid].offset_ms <= v) {
      i = mid;
      lo = mid + 1;
    } else hi = mid - 1;
  }
  const e = pl.entries[i];
  return { index: i, ms: Math.min(e.end_ms, e.start_ms + v - e.offset_ms) };
}

/* ---------- Highlighting ---------- */

/** Lines in time order, for {@link activeSegment}. */
export function sortLines<T extends ReplaySegment>(segments: T[]): T[] {
  return segments.filter(isLine).sort((a, b) => a.start_ms - b.start_ms || a.id - b.id);
}

/** How far back {@link activeSegment} looks for a line that started before
 *  a later one and still covers `ms` (overlapping lines across channels).
 *  Segments are VAD chunks of at most ~30 s, so this is generous. */
const LOOKBACK_MS = 120_000;

/** The line being played at real time `ms` in `sorted` (from
 *  {@link sortLines}): of the lines covering `ms`, the one that started
 *  last; with `speakerId`, only that speaker's lines. null in a gap. */
export function activeSegment<T extends ReplaySegment>(sorted: T[], ms: number, speakerId: string | null = null): T | null {
  let lo = 0;
  let hi = sorted.length - 1;
  let i = -1;
  while (lo <= hi) {
    const mid = (lo + hi) >> 1;
    if (sorted[mid].start_ms <= ms) {
      i = mid;
      lo = mid + 1;
    } else hi = mid - 1;
  }
  for (let j = i; j >= 0 && ms - sorted[j].start_ms <= LOOKBACK_MS; j--) {
    const s = sorted[j];
    if (speakerId !== null && s.speaker_id !== speakerId) continue;
    if (ms < s.end_ms) return s;
  }
  return null;
}

/** The line to go to from real time `ms`: the next one starting after it
 *  (`dir` 1), or (`dir` -1) the start of the current line when more than
 *  1.5 s into it, else the previous one — like a player's "previous
 *  track". With `speakerId`, only that speaker's lines. Pure. */
export function adjacentLine<T extends ReplaySegment>(sorted: T[], ms: number, dir: 1 | -1, speakerId: string | null = null): T | null {
  const own = speakerId === null ? sorted : sorted.filter((s) => s.speaker_id === speakerId);
  if (dir === 1) return own.find((s) => s.start_ms > ms + 1) ?? null;
  let prev: T | null = null;
  for (const s of own) {
    if (s.start_ms < ms - 1500) prev = s;
    else break;
  }
  return prev;
}

/** One displayed word with its timing. */
export interface TimedToken {
  text: string;
  start_ms: number;
  end_ms: number;
}

/** The line's text split into its words with timings, when the words of
 *  `segments.json` (timed on the raw transcript) line up one to one with
 *  the displayed text — cleanup that only fixes punctuation or case keeps
 *  them aligned. null when they don't (a rewritten or edited line): the
 *  line is then highlighted as a whole. Pure. */
export function wordSpans(text: string, words: Word[] | undefined): TimedToken[] | null {
  if (!words || words.length === 0) return null;
  const tokens = text.split(/\s+/).filter(Boolean);
  if (tokens.length !== words.length) return null;
  return tokens.map((t, i) => ({ text: t, start_ms: words[i].start_ms, end_ms: words[i].end_ms }));
}

/** Index of the word being said at `ms`: the last one started by then
 *  (it stays lit through the pause before the next). -1 before the first. */
export function activeWord(tokens: { start_ms: number }[], ms: number): number {
  let lo = 0;
  let hi = tokens.length - 1;
  let i = -1;
  while (lo <= hi) {
    const mid = (lo + hi) >> 1;
    if (tokens[mid].start_ms <= ms) {
      i = mid;
      lo = mid + 1;
    } else hi = mid - 1;
  }
  return i;
}

/** Talk time per speaker, for the "Play only" menu: speakers with at least
 *  one line whose channel was saved, in the document's order. Pure. */
export function speakerTalkTime(
  segments: ReplaySegment[],
  files: string[],
  speakers: { id: string; label: string }[],
): { id: string; label: string; ms: number }[] {
  const ms = new Map<string, number>();
  for (const s of segments) {
    if (!s.speaker_id || !isLine(s) || s.end_ms <= s.start_ms || !channelFile(s.channel, files)) continue;
    ms.set(s.speaker_id, (ms.get(s.speaker_id) ?? 0) + s.end_ms - s.start_ms);
  }
  return speakers.filter((sp) => ms.has(sp.id)).map((sp) => ({ id: sp.id, label: sp.label, ms: ms.get(sp.id)! }));
}

/** `M:SS` / `H:MM:SS` for the player's clock. */
export function formatPlayerTime(ms: number): string {
  const s = Math.max(0, Math.floor(ms / 1000));
  const h = Math.floor(s / 3600);
  const m = Math.floor(s / 60) % 60;
  const ss = String(s % 60).padStart(2, "0");
  return h > 0 ? `${h}:${String(m).padStart(2, "0")}:${ss}` : `${m}:${ss}`;
}
