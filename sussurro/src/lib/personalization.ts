/* Dictionary & snippets manager (#99), pure helpers: search, sort, dedupe,
   edits, paging and export in the same formats the bulk import reads
   (../utils: parseDictionaryFile for .txt, parseSnippetFile for .csv).

   The dictionary stays a plain list, one word or phrase per entry (= one
   per line in the file); entries are compared trimmed and
   case-insensitively, exactly like `mergeDictionary`. Snippets are
   identified by their cue as the matcher sees it (`normalizeCue`). */

import { mergeDictionary, normalizeCue, parseDictionaryFile, type DictionaryMerge, type SnippetEntry } from "../utils";

export type ListSort = "added" | "az" | "za";

/** Search key: lowercase, accents stripped ("Città" matches "citta"). */
export function searchKey(s: string): string {
  return s.normalize("NFD").replace(/\p{M}+/gu, "").toLowerCase();
}

/** Identity of a dictionary entry (same rule as `mergeDictionary`). */
export const wordKey = (w: string) => w.trim().toLowerCase();

const collator = new Intl.Collator(undefined, { sensitivity: "base", numeric: true });

function sortRows<T>(rows: T[], sort: ListSort, key: (r: T) => string): T[] {
  if (sort === "added") return rows;
  const dir = sort === "az" ? 1 : -1;
  return rows.slice().sort((a, b) => dir * collator.compare(key(a), key(b)));
}

/* ---------- Dictionary ---------- */

export interface WordRow {
  word: string;
  /** Position in `settings.dictionary`: what edits and deletes refer to. */
  index: number;
  /** Repeats an earlier entry (trimmed, case-insensitive). */
  duplicate: boolean;
}

/** The dictionary's rows matching `query`, in `sort` order. */
export function dictionaryRows(words: string[], query: string, sort: ListSort): WordRow[] {
  const q = searchKey(query.trim());
  const seen = new Set<string>();
  const rows: WordRow[] = [];
  words.forEach((word, index) => {
    const k = wordKey(word);
    const duplicate = seen.has(k);
    seen.add(k);
    if (!q || searchKey(word).includes(q)) rows.push({ word, index, duplicate });
  });
  return sortRows(rows, sort, (r) => r.word.trim());
}

/** How many entries repeat an earlier one or are blank (what dedupe removes). */
export function dictionaryDuplicateCount(words: string[]): number {
  return words.length - dedupeDictionary(words).words.length;
}

/** Keep the first of each entry (trimmed, case-insensitive), drop blanks. */
export function dedupeDictionary(words: string[]): { words: string[]; removed: number } {
  const seen = new Set<string>();
  const out: string[] = [];
  for (const w of words) {
    const k = wordKey(w);
    if (!k || seen.has(k)) continue;
    seen.add(k);
    out.push(w.trim());
  }
  return { words: out, removed: words.length - out.length };
}

/** Add what the user typed or pasted: one entry per line, existing kept. */
export function addDictionaryWords(words: string[], input: string): DictionaryMerge {
  return mergeDictionary(words, parseDictionaryFile(input));
}

export type WordEditError = "empty" | "multiline" | "duplicate";

/** Replace entry `index` with `value`, or say why not. */
export function editDictionaryWord(
  words: string[],
  index: number,
  value: string,
): { words: string[] } | { error: WordEditError } {
  if (/[\r\n]/.test(value.trim())) return { error: "multiline" };
  const word = value.trim();
  if (!word) return { error: "empty" };
  const k = wordKey(word);
  if (words.some((w, i) => i !== index && wordKey(w) === k)) return { error: "duplicate" };
  const next = words.slice();
  next[index] = word;
  return { words: next };
}

/** The dictionary without the entries at `indices`. */
export function removeIndices<T>(list: T[], indices: Iterable<number>): T[] {
  const drop = new Set(indices);
  return list.filter((_, i) => !drop.has(i));
}

/** The .txt export: one entry per line, what `parseDictionaryFile` reads. */
export function exportDictionaryTxt(words: string[]): string {
  const lines = words.map((w) => w.trim()).filter(Boolean);
  return lines.length ? lines.join("\n") + "\n" : "";
}

/* ---------- Snippets ---------- */

export interface SnippetRow {
  snippet: SnippetEntry;
  index: number;
  /** Another snippet earlier in the list has the same cue (as the matcher
   *  compares them): it is the one used, this one never fires. */
  shadowedBy: number | null;
  /** How many other snippets share this cue (0 = unique). */
  sameCue: number;
  incomplete: boolean;
}

/** index → index of the first snippet with the same cue, for every snippet
 *  that a previous one shadows; plus the size of each cue group. */
export function cueGroups(snippets: SnippetEntry[]): { first: Map<number, number>; size: Map<string, number> } {
  const firstByCue = new Map<string, number>();
  const size = new Map<string, number>();
  const first = new Map<number, number>();
  snippets.forEach((s, i) => {
    if (!s.cue.trim()) return;
    const k = normalizeCue(s.cue);
    size.set(k, (size.get(k) ?? 0) + 1);
    const f = firstByCue.get(k);
    if (f === undefined) firstByCue.set(k, i);
    else first.set(i, f);
  });
  return { first, size };
}

/** The snippets matching `query` (in the cue or the text), in `sort` order. */
export function snippetRows(snippets: SnippetEntry[], query: string, sort: ListSort): SnippetRow[] {
  const q = searchKey(query.trim());
  const { first, size } = cueGroups(snippets);
  const rows: SnippetRow[] = [];
  snippets.forEach((snippet, index) => {
    if (q && !searchKey(snippet.cue).includes(q) && !searchKey(snippet.text).includes(q)) return;
    const group = snippet.cue.trim() ? (size.get(normalizeCue(snippet.cue)) ?? 1) : 1;
    rows.push({
      snippet,
      index,
      shadowedBy: first.get(index) ?? null,
      sameCue: group - 1,
      incomplete: !snippet.cue.trim() || !snippet.text.trim(),
    });
  });
  return sortRows(rows, sort, (r) => r.snippet.cue.trim());
}

/** Snippets whose cue repeats an earlier one (they never fire). */
export function duplicateCueCount(snippets: SnippetEntry[]): number {
  return cueGroups(snippets).first.size;
}

/** Problems with a snippet being added or edited (`index` = the one being
 *  edited, -1 for a new one): the other snippet already using its cue. */
export function snippetDraftIssues(
  snippets: SnippetEntry[],
  draft: SnippetEntry,
  index: number,
): { emptyCue: boolean; emptyText: boolean; cueUsedBy: number | null } {
  const k = normalizeCue(draft.cue);
  const other = draft.cue.trim()
    ? snippets.findIndex((s, i) => i !== index && s.cue.trim() !== "" && normalizeCue(s.cue) === k)
    : -1;
  return {
    emptyCue: !draft.cue.trim(),
    emptyText: !draft.text.trim(),
    cueUsedBy: other === -1 ? null : other,
  };
}

/** One CSV field, quoted when the import would otherwise split or trim it. */
function csvField(s: string): string {
  return /[",\r\n]|^\s|\s$/.test(s) ? `"${s.replace(/"/g, '""')}"` : s;
}

/** The .csv export: a `cue,text` header, then one snippet per record, text
 *  quoted when it holds commas, quotes or line breaks — what
 *  `parseSnippetFile` reads. Snippets without a cue or a text are left out
 *  (the import would skip them, and they never fire). */
export function exportSnippetsCsv(snippets: SnippetEntry[]): string {
  const rows = snippets
    .map((s) => ({ cue: s.cue.trim(), text: s.text.trim() }))
    .filter((s) => s.cue && s.text)
    .map((s) => `${csvField(s.cue)},${csvField(s.text)}`);
  return rows.length ? ["cue,text", ...rows].join("\n") + "\n" : "";
}

/* ---------- Paging ---------- */

export const PAGE_SIZE = 100;

/** One page of `rows` (0-based, clamped), and how many pages there are. */
export function pageOf<T>(rows: T[], page: number, size = PAGE_SIZE): { rows: T[]; page: number; pages: number } {
  const pages = Math.max(1, Math.ceil(rows.length / size));
  const p = Math.min(Math.max(0, Math.floor(page)), pages - 1);
  return { rows: rows.slice(p * size, (p + 1) * size), page: p, pages };
}
