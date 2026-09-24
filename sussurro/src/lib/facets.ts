/* Library facets (0.9, #135): the selection state of the facet filters,
 * what goes to `archive_facets`, and the labels the UI shows.
 *
 * Semantics mirror archive/facets.rs: values within a facet are ORed,
 * facets are ANDed with each other and with the text query, and the counts
 * of a facet ignore its own selection (so its alternatives stay visible).
 * The date facet takes one cumulative bucket or one custom range. */

import { nameKey } from "./people";
import type { ItemSummary, ItemType } from "./types";

export type DateBucket = "today" | "week" | "month" | "year" | "older";

export const DATE_BUCKETS: { value: DateBucket; label: string }[] = [
  { value: "today", label: "Today" },
  { value: "week", label: "This week" },
  { value: "month", label: "This month" },
  { value: "year", label: "This year" },
  { value: "older", label: "Older" },
];

export interface FacetValue {
  key: string;
  label: string;
  count: number;
}

export interface Facets {
  total: number;
  types: FacetValue[];
  tags: FacetValue[];
  categories: FacetValue[];
  participants: FacetValue[];
  dates: FacetValue[];
}

/** `archive_facets` result. */
export interface FacetedSearch {
  items: ItemSummary[];
  facets: Facets;
}

/** The multi-select facets (the type chips and the date are separate). */
export type ListFacet = "tags" | "categories" | "participants";

export const LIST_FACETS: { value: ListFacet; label: string; noun: string }[] = [
  { value: "tags", label: "Tag", noun: "tag" },
  { value: "categories", label: "Category", noun: "category" },
  { value: "participants", label: "Participant", noun: "participant" },
];

export type DateSelection = { bucket: DateBucket } | { from: string; to: string } | null;

export interface FacetState {
  /** Item types, any of; empty = all. */
  types: ItemType[];
  tags: string[];
  categories: string[];
  /** Participant keys (`person:<id>` / `name:<name key>`). */
  participants: string[];
  /** Labels of the selected values, so a chip still reads right when the
   *  current counts no longer list the value. */
  labels: Record<string, string>;
  date: DateSelection;
}

export const EMPTY_FACETS: FacetState = {
  types: [],
  tags: [],
  categories: [],
  participants: [],
  labels: {},
  date: null,
};

/** Filters for `archive_facets` / `archive_search` (SearchFilters). */
export interface FacetFilters {
  types: ItemType[];
  tags: string[];
  categories: string[];
  participants: string[];
  date_bucket?: DateBucket;
  date_from?: string;
  date_to?: string;
  today: string;
}

const labelKey = (facet: ListFacet, key: string) => `${facet}:${key}`;

/** The viewer's local date as `YYYY-MM-DD` (never the UTC day). */
export function localToday(now: Date = new Date()): string {
  const p = (n: number) => String(n).padStart(2, "0");
  return `${now.getFullYear()}-${p(now.getMonth() + 1)}-${p(now.getDate())}`;
}

export function toggleType(s: FacetState, t: ItemType): FacetState {
  const types = s.types.includes(t) ? s.types.filter((x) => x !== t) : [...s.types, t];
  // Every type selected is the same as none.
  return { ...s, types: types.length === 3 ? [] : types };
}

export function clearTypes(s: FacetState): FacetState {
  return { ...s, types: [] };
}

export function toggleValue(s: FacetState, facet: ListFacet, key: string, label?: string): FacetState {
  const list = s[facet];
  const on = list.includes(key);
  const labels = { ...s.labels };
  if (on) delete labels[labelKey(facet, key)];
  else if (label) labels[labelKey(facet, key)] = label;
  return { ...s, [facet]: on ? list.filter((k) => k !== key) : [...list, key], labels };
}

export function clearFacet(s: FacetState, facet: ListFacet | "date"): FacetState {
  if (facet === "date") return { ...s, date: null };
  const labels = Object.fromEntries(Object.entries(s.labels).filter(([k]) => !k.startsWith(`${facet}:`)));
  return { ...s, [facet]: [], labels };
}

export function setDate(s: FacetState, date: DateSelection): FacetState {
  return { ...s, date };
}

/** A custom range is usable when its days parse and are in order (either
 *  end may be open). */
export function validRange(from: string, to: string): boolean {
  const ok = (d: string) => d === "" || (/^\d{4}-\d{2}-\d{2}$/.test(d) && !Number.isNaN(Date.parse(d)));
  if (!ok(from) || !ok(to) || (from === "" && to === "")) return false;
  return from === "" || to === "" || from <= to;
}

/** Selected values across every facet, types and date included. */
export function activeCount(s: FacetState): number {
  return s.types.length + s.tags.length + s.categories.length + s.participants.length + (s.date ? 1 : 0);
}

/** Any facet (besides the type chips) set — what the collapsed "Filters"
 *  button counts. */
export function facetCount(s: FacetState): number {
  return s.tags.length + s.categories.length + s.participants.length + (s.date ? 1 : 0);
}

export function clearAll(): FacetState {
  return { ...EMPTY_FACETS, labels: {} };
}

export function toFilters(s: FacetState, today: string): FacetFilters {
  const f: FacetFilters = {
    types: s.types,
    tags: s.tags,
    categories: s.categories,
    participants: s.participants,
    today,
  };
  if (s.date && "bucket" in s.date) f.date_bucket = s.date.bucket;
  else if (s.date && validRange(s.date.from, s.date.to)) {
    if (s.date.from) f.date_from = s.date.from;
    if (s.date.to) f.date_to = s.date.to;
  }
  return f;
}

/** The values to list in a facet: the selected ones first (with a zero
 *  count when the current query no longer finds them), then the rest in
 *  the server's order (most items first). */
export function withSelected(values: FacetValue[], selected: string[], labels: Record<string, string>, facet: ListFacet): FacetValue[] {
  const byKey = new Map(values.map((v) => [v.key, v]));
  const first = selected.map((k) => byKey.get(k) ?? { key: k, label: labels[labelKey(facet, k)] ?? fallbackLabel(k), count: 0 });
  return [...first, ...values.filter((v) => !selected.includes(v.key))];
}

/** A readable label for a key with no remembered label. */
function fallbackLabel(key: string): string {
  return key.replace(/^(person|name):/, "");
}

/** Values whose label contains the typed text (case and accents ignored). */
export function filterValues(values: FacetValue[], query: string): FacetValue[] {
  const q = nameKey(query);
  return q ? values.filter((v) => nameKey(v.label).includes(q)) : values;
}

export function valueLabel(s: FacetState, facet: ListFacet, key: string, values: FacetValue[] = []): string {
  return values.find((v) => v.key === key)?.label ?? s.labels[labelKey(facet, key)] ?? fallbackLabel(key);
}

/** "12 Mar 2026" from `YYYY-MM-DD` (local, no time-zone shift). */
function shortDay(day: string): string {
  const [y, m, d] = day.split("-").map(Number);
  return new Date(y, m - 1, d).toLocaleDateString(undefined, { day: "numeric", month: "short", year: "numeric" });
}

export function dateLabel(date: DateSelection): string {
  if (!date) return "";
  if ("bucket" in date) return DATE_BUCKETS.find((b) => b.value === date.bucket)?.label ?? date.bucket;
  if (date.from && date.to) return date.from === date.to ? shortDay(date.from) : `${shortDay(date.from)} – ${shortDay(date.to)}`;
  if (date.from) return `from ${shortDay(date.from)}`;
  if (date.to) return `until ${shortDay(date.to)}`;
  return "";
}

/** Chip text: "Tag", "Tag: release", "Tag: release +2". */
export function facetSummary(s: FacetState, facet: ListFacet | "date", values: FacetValue[] = []): string {
  if (facet === "date") {
    const d = dateLabel(s.date);
    return d ? `Date: ${d}` : "Date";
  }
  const name = LIST_FACETS.find((f) => f.value === facet)?.label ?? facet;
  const sel = s[facet];
  if (!sel.length) return name;
  const first = valueLabel(s, facet, sel[0], values);
  return sel.length > 1 ? `${name}: ${first} +${sel.length - 1}` : `${name}: ${first}`;
}

/* ---------- persistence (localStorage, like the other UI choices) ---------- */

export const FACETS_KEY = "libraryFacets";

const TYPES: ItemType[] = ["note", "meeting", "transcription"];
const BUCKETS = DATE_BUCKETS.map((b) => b.value);
/** Bound on each remembered list (the backend caps a facet at 200). */
const MAX_REMEMBERED = 100;

function strings(v: unknown): string[] {
  if (!Array.isArray(v)) return [];
  return [...new Set(v.filter((x): x is string => typeof x === "string" && x.trim() !== ""))].slice(0, MAX_REMEMBERED);
}

/** Parse a remembered selection, dropping anything malformed. */
export function parseFacetState(raw: string | null): FacetState {
  if (!raw) return clearAll();
  let v: unknown;
  try {
    v = JSON.parse(raw);
  } catch {
    return clearAll();
  }
  if (!v || typeof v !== "object") return clearAll();
  const o = v as Record<string, unknown>;
  const types = strings(o.types).filter((t): t is ItemType => TYPES.includes(t as ItemType));
  const labels: Record<string, string> = {};
  if (o.labels && typeof o.labels === "object") {
    for (const [k, l] of Object.entries(o.labels as Record<string, unknown>)) if (typeof l === "string") labels[k] = l;
  }
  let date: DateSelection = null;
  const d = o.date as Record<string, unknown> | null | undefined;
  if (d && typeof d === "object") {
    if (typeof d.bucket === "string" && BUCKETS.includes(d.bucket as DateBucket)) date = { bucket: d.bucket as DateBucket };
    else if (typeof d.from === "string" && typeof d.to === "string" && validRange(d.from, d.to)) date = { from: d.from, to: d.to };
  }
  const s: FacetState = {
    types: types.length === 3 ? [] : types,
    tags: strings(o.tags),
    categories: strings(o.categories),
    participants: strings(o.participants),
    labels: {},
    date,
  };
  // Keep only the labels of selected values.
  for (const f of LIST_FACETS)
    for (const k of s[f.value]) {
      const l = labels[labelKey(f.value, k)];
      if (l) s.labels[labelKey(f.value, k)] = l;
    }
  return s;
}

export function loadFacetState(): FacetState {
  try {
    return parseFacetState(localStorage.getItem(FACETS_KEY));
  } catch {
    return clearAll();
  }
}

export function saveFacetState(s: FacetState): void {
  try {
    if (activeCount(s) === 0) localStorage.removeItem(FACETS_KEY);
    else localStorage.setItem(FACETS_KEY, JSON.stringify(s));
  } catch {
    /* storage unavailable: the selection just isn't kept */
  }
}
