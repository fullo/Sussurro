/* Voice map (#144): the pure logic behind the Speakers panel's map card —
   which items show it, the backend's 2-D projection (archive_voice_map)
   mapped onto the SVG with the document's current speakers and lines,
   the legend, the caption, hit-testing and keyboard steps. */

import { lineTimestamp } from "@sussurro/transcript";
import type { Item, VoiceMap } from "./types";

/** The SVG's viewBox: width × height, and the margin kept free of points. */
export const VIEW_W = 320;
export const VIEW_H = 220;
export const VIEW_PAD = 10;
/** Colour of a line without a (known) speaker: a theme token. */
export const NO_SPEAKER_COLOR = "var(--c-text-light, #595959)";
/** Longest line text shown on hover, in characters. */
export const TEXT_MAX = 140;

/** Whether the Speakers panel shows the Voice map: not on notes (they
 *  have no speakers, P10), not while recording (the voice data is still
 *  growing), and only with voice data on at least two lines. */
export function voiceMapShown(item: Pick<Item, "meta" | "recording" | "embedded_segments">): boolean {
  return item.meta.type !== "note" && !item.recording && (item.embedded_segments ?? 0) >= 2;
}

export interface MapPoint {
  segmentId: number;
  /** The line's current speaker (null = none). */
  speakerId: string | null;
  label: string;
  color: string;
  /** Position in the viewBox (y grows downwards, as in SVG). */
  x: number;
  y: number;
  startMs: number;
  /** `HH:MM:SS`, as the transcript shows it. */
  time: string;
  /** The line's text, shortened for a tooltip. */
  text: string;
}

export interface LegendEntry {
  /** Speaker id, or null for lines without a speaker. */
  id: string | null;
  label: string;
  color: string;
  /** Points of this speaker on the map. */
  count: number;
}

export interface MapLayout {
  /** In time order (the keyboard walks them in this order). */
  points: MapPoint[];
  /** Speakers with at least one point, in the document's speaker order;
   *  lines without a speaker last. */
  legend: LegendEntry[];
}

/** `text` cut to `max` characters at a word boundary, with an ellipsis. */
export function shorten(text: string, max = TEXT_MAX): string {
  const t = text.trim().replace(/\s+/g, " ");
  if ([...t].length <= max) return t;
  const cut = [...t].slice(0, max).join("");
  const space = cut.lastIndexOf(" ");
  return `${(space > max * 0.6 ? cut.slice(0, space) : cut).trimEnd()}…`;
}

/** Place the backend's points in the viewBox, same scale on both axes (so
 *  distances keep their proportions), centred, +y up. Each point takes its
 *  line's current speaker, colour and text from the item — a line moved
 *  or renamed since the map was computed shows as it is now — and points
 *  of lines deleted since are dropped. */
export function layoutVoiceMap(
  map: VoiceMap,
  item: Pick<Item, "segments">,
  w = VIEW_W,
  h = VIEW_H,
  pad = VIEW_PAD,
): MapLayout {
  const segs = new Map(item.segments.segments.map((s) => [s.id, s]));
  const speakers = new Map(item.segments.speakers.map((s) => [s.id, s]));
  const kept = map.points.filter((p) => segs.has(p.segment_id) && Number.isFinite(p.x) && Number.isFinite(p.y));
  const xs = kept.map((p) => p.x);
  const ys = kept.map((p) => p.y);
  const [minX, maxX] = [Math.min(...xs), Math.max(...xs)];
  const [minY, maxY] = [Math.min(...ys), Math.max(...ys)];
  const midX = (minX + maxX) / 2;
  const midY = (minY + maxY) / 2;
  const span = Math.max((maxX - minX) / (w - 2 * pad), (maxY - minY) / (h - 2 * pad));
  const scale = span > 0 ? 1 / span : 0;
  const points: MapPoint[] = kept.map((p) => {
    const seg = segs.get(p.segment_id)!;
    const speakerId = seg.speaker_id ?? null;
    const sp = speakerId ? speakers.get(speakerId) : undefined;
    return {
      segmentId: p.segment_id,
      speakerId,
      label: sp?.label ?? (speakerId ? speakerId : "No speaker"),
      color: sp?.color || NO_SPEAKER_COLOR,
      x: w / 2 + (p.x - midX) * scale,
      y: h / 2 - (p.y - midY) * scale,
      startMs: seg.start_ms,
      time: lineTimestamp(seg.start_ms),
      text: seg.text.trim() ? shorten(seg.text) : "[not transcribed]",
    };
  });
  points.sort((a, b) => a.startMs - b.startMs || a.segmentId - b.segmentId);

  const counts = new Map<string | null, number>();
  for (const p of points) counts.set(p.speakerId, (counts.get(p.speakerId) ?? 0) + 1);
  const legend: LegendEntry[] = [];
  for (const sp of item.segments.speakers) {
    const n = counts.get(sp.id);
    if (n) legend.push({ id: sp.id, label: sp.label, color: sp.color || NO_SPEAKER_COLOR, count: n });
  }
  // Speakers the list doesn't know (shouldn't happen), then no speaker.
  for (const [id, n] of counts) {
    if (id !== null && !speakers.has(id)) legend.push({ id, label: id, color: NO_SPEAKER_COLOR, count: n });
  }
  const none = counts.get(null);
  if (none) legend.push({ id: null, label: "No speaker", color: NO_SPEAKER_COLOR, count: none });
  return { points, legend };
}

/** Dot radius in viewBox units: smaller as the map fills up. */
export function pointRadius(n: number): number {
  if (n > 2000) return 1.6;
  if (n > 500) return 2.2;
  if (n > 100) return 3;
  return 4;
}

/** Index of the point nearest (x, y) within `maxDist`, or -1. */
export function nearestPoint(points: Pick<MapPoint, "x" | "y">[], x: number, y: number, maxDist: number): number {
  let best = -1;
  let bestD = maxDist * maxDist;
  points.forEach((p, i) => {
    const d = (p.x - x) ** 2 + (p.y - y) ** 2;
    if (d <= bestD) {
      best = i;
      bestD = d;
    }
  });
  return best;
}

/** The point a key moves to from `current` (-1 = none yet), in time
 *  order: ← ↑ previous, → ↓ next, Home/End first/last, PageUp/PageDown
 *  ten lines. `null` when the key is not the map's. */
export function stepPoint(count: number, current: number, key: string): number | null {
  if (count === 0) return null;
  const at = (i: number) => Math.min(count - 1, Math.max(0, i));
  switch (key) {
    case "ArrowRight":
    case "ArrowDown":
      return current < 0 ? 0 : at(current + 1);
    case "ArrowLeft":
    case "ArrowUp":
      return current < 0 ? count - 1 : at(current - 1);
    case "PageDown":
      return at(current + 10);
    case "PageUp":
      return at(current - 10);
    case "Home":
      return 0;
    case "End":
      return count - 1;
    default:
      return null;
  }
}

/** What the map shows and how far to trust it, under the map. */
export function mapCaption(map: Pick<VoiceMap, "points" | "total" | "explained">): string {
  const pct = Math.round(Math.max(0, Math.min(1, map.explained)) * 100);
  const base =
    "Each dot is a line, placed by how the voice sounds: dots close together sound alike, so a speaker's lines " +
    "usually gather in one cloud. Closeness is approximate";
  const keep = pct > 0 ? ` — a flat map keeps about ${pct}% of the differences between voice prints.` : ".";
  const sampled =
    map.total > map.points.length
      ? ` Showing ${map.points.length.toLocaleString("en")} of ${map.total.toLocaleString("en")} lines.`
      : "";
  return base + keep + sampled;
}

/** Screen-reader name of a point. */
export function pointLabel(p: Pick<MapPoint, "label" | "time" | "text">): string {
  return `${p.label}, ${p.time}: ${p.text}`;
}
