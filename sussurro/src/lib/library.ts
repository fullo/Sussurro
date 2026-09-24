/* Pure helpers for the Library list and the document header. */

import { formatDurationLabel, formatItemDate, parseDuration } from "./format";
import type { ItemMeta, ItemSummary, ItemType } from "./types";

export type TypeFilter = ItemType | "all";

export const TYPE_FILTERS: { value: TypeFilter; label: string }[] = [
  { value: "all", label: "All" },
  { value: "note", label: "Notes" },
  { value: "meeting", label: "Meetings" },
  { value: "transcription", label: "Transcriptions" },
];

export const TYPE_LABEL: Record<ItemType, string> = {
  note: "Note",
  meeting: "Meeting",
  transcription: "Transcription",
};

/** Items of one type (client-side fallback when the search index is down). */
export function filterByType(items: ItemSummary[], type: TypeFilter): ItemSummary[] {
  return type === "all" ? items : items.filter((i) => i.meta.type === type);
}

/** Case-insensitive substring match on title, tags, categories and
 *  participant names and emails — the client-side fallback for the
 *  full-text index. */
export function matchesQuery(item: ItemSummary, query: string): boolean {
  const q = query.trim().toLowerCase();
  if (!q) return true;
  const people = (item.meta.participants ?? []).flatMap((p) => [p.name, p.email ?? ""]);
  const hay = [item.meta.title, ...item.meta.tags, ...item.meta.categories, ...people].join("\n").toLowerCase();
  return q.split(/\s+/).every((w) => hay.includes(w));
}

/** "Today 08:40 · 3 min" */
export function itemSubtitle(meta: ItemMeta, now: Date = new Date()): string {
  const parts = [formatItemDate(meta.date, now)];
  const d = formatDurationLabel(parseDuration(meta.duration));
  if (d) parts.push(d);
  return parts.filter(Boolean).join(" · ");
}

/** Split an FTS snippet with `**match**` markers into plain and highlighted
 *  runs, so the UI renders <mark> without injecting HTML. */
export function splitHighlights(snippet: string): { text: string; hit: boolean }[] {
  const out: { text: string; hit: boolean }[] = [];
  const parts = snippet.split("**");
  parts.forEach((text, i) => {
    if (!text) return;
    // An unbalanced trailing marker leaves the last run unhighlighted.
    const hit = i % 2 === 1 && i < parts.length - 1;
    const last = out[out.length - 1];
    if (last && last.hit === hit) last.text += text;
    else out.push({ text, hit });
  });
  return out;
}

/** Values typed into a chip field: split on commas, trimmed, no empties. */
export function parseChipInput(input: string): string[] {
  return input.split(",").map((s) => s.trim()).filter(Boolean);
}

/** Add values to a tag/category list, skipping case-insensitive duplicates. */
export function addChips(list: string[], values: string[]): string[] {
  const out = list.slice();
  const seen = new Set(out.map((v) => v.toLowerCase()));
  for (const v of values) {
    const t = v.trim();
    if (t && !seen.has(t.toLowerCase())) {
      out.push(t);
      seen.add(t.toLowerCase());
    }
  }
  return out;
}

export function removeChip(list: string[], value: string): string[] {
  return list.filter((v) => v !== value);
}

/** Metadata with the New screen's default tags and category merged in;
 *  null when there is nothing to add (no write needed). */
export function withDefaults(meta: ItemMeta, tags: string[], categories: string[]): ItemMeta | null {
  const nextTags = addChips(meta.tags, tags);
  const nextCats = addChips(meta.categories, categories);
  if (nextTags.length === meta.tags.length && nextCats.length === meta.categories.length) return null;
  return { ...meta, tags: nextTags, categories: nextCats };
}
