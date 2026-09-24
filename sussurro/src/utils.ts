export interface SnippetEntry {
  cue: string;
  text: string;
}

/**
 * Parses a dictionary file (.txt) where each line is a dictionary entry.
 * Removes leading/trailing whitespace and ignores empty lines.
 */
export function parseDictionaryFile(content: string): string[] {
  return content
    .split(/\r?\n/)
    .map((line) => line.trim())
    .filter((line) => line.length > 0);
}

/**
 * Splits CSV text into records of fields (RFC 4180 style): fields may be
 * wrapped in double quotes, which lets them contain commas and line breaks
 * (e.g. a multi-line email signature); `""` inside quotes is a literal `"`.
 */
function parseCsv(content: string): string[][] {
  const records: string[][] = [];
  let record: string[] = [];
  let field = "";
  let inQuotes = false;
  let atFieldStart = true;

  for (let i = 0; i < content.length; i++) {
    const c = content[i];
    if (inQuotes) {
      if (c === '"') {
        if (content[i + 1] === '"') {
          field += '"';
          i++;
        } else {
          inQuotes = false;
        }
      } else {
        field += c;
      }
    } else if (c === '"' && atFieldStart) {
      inQuotes = true;
      atFieldStart = false;
    } else if (c === ",") {
      record.push(field);
      field = "";
      atFieldStart = true;
    } else if (c === "\n" || c === "\r") {
      if (c === "\r" && content[i + 1] === "\n") i++;
      record.push(field);
      records.push(record);
      record = [];
      field = "";
      atFieldStart = true;
    } else {
      // Leading spaces before an opening quote (`cue, "text"`) are allowed.
      if (!(atFieldStart && c.trim() === "")) atFieldStart = false;
      field += c;
    }
  }
  if (field !== "" || record.length > 0) {
    record.push(field);
    records.push(record);
  }
  return records;
}

/**
 * Parses a snippet file (.csv) where each record is `cue,text`.
 * Wrap the text in double quotes when it contains commas or line breaks.
 * Unquoted extra commas are kept as part of the text. An optional
 * `cue,text` header row and records without both a cue and a text are skipped.
 */
export function parseSnippetFile(content: string): SnippetEntry[] {
  const snippets: SnippetEntry[] = [];
  let first = true;

  for (const fields of parseCsv(content)) {
    if (fields.every((f) => f.trim() === "")) continue;
    const cue = fields[0].trim();
    const text = fields.slice(1).join(",").trim();
    if (first) {
      first = false;
      if (cue.toLowerCase() === "cue" && text.toLowerCase() === "text") continue;
    }
    if (cue && text) snippets.push({ cue, text });
  }

  return snippets;
}

/**
 * Cue identity as the snippet matcher sees it (mirrors `normalize` in
 * src-tauri/src/snippets.rs): case and punctuation are ignored.
 */
export function normalizeCue(cue: string): string {
  const n = cue
    .toLowerCase()
    .replace(/[^\p{L}\p{N}]+/gu, " ")
    .trim();
  return n || cue.trim().toLowerCase();
}

export interface DictionaryMerge {
  merged: string[];
  added: number;
  present: number;
}

/**
 * Appends imported words that aren't already in the dictionary. Comparison is
 * trimmed and case-insensitive; existing entries are never changed or removed.
 */
export function mergeDictionary(existing: string[], imported: string[]): DictionaryMerge {
  const merged = existing.slice();
  const seen = new Set(existing.map((w) => w.trim().toLowerCase()));
  let added = 0;
  let present = 0;
  for (const raw of imported) {
    const word = raw.trim();
    if (!word) continue;
    const key = word.toLowerCase();
    if (seen.has(key)) {
      present++;
    } else {
      seen.add(key);
      merged.push(word);
      added++;
    }
  }
  return { merged, added, present };
}

export interface SnippetMerge {
  merged: SnippetEntry[];
  added: number;
  /** Same cue and same text already present. */
  present: number;
  /** Same cue but different text: the existing snippet was kept. */
  conflicts: number;
}

/**
 * Appends imported snippets whose cue isn't already used. On a cue clash the
 * existing snippet always wins; a clash with a different text is counted as a
 * skipped conflict so the user can be told.
 */
export function mergeSnippets(existing: SnippetEntry[], imported: SnippetEntry[]): SnippetMerge {
  const merged = existing.slice();
  const byCue = new Map<string, string>();
  for (const s of existing) {
    const key = normalizeCue(s.cue);
    if (!byCue.has(key)) byCue.set(key, s.text);
  }
  let added = 0;
  let present = 0;
  let conflicts = 0;
  for (const s of imported) {
    const key = normalizeCue(s.cue);
    const current = byCue.get(key);
    if (current === undefined) {
      byCue.set(key, s.text);
      merged.push({ cue: s.cue, text: s.text });
      added++;
    } else if (current.trim() === s.text.trim()) {
      present++;
    } else {
      conflicts++;
    }
  }
  return { merged, added, present, conflicts };
}

const plural = (n: number, one: string, many: string) => `${n} ${n === 1 ? one : many}`;

export function describeDictionaryMerge(r: DictionaryMerge): string {
  let msg = `Added ${plural(r.added, "word", "words")}`;
  if (r.present) msg += `, ${r.present} already present`;
  return msg;
}

export function describeSnippetMerge(r: SnippetMerge): string {
  let msg = `Added ${plural(r.added, "snippet", "snippets")}`;
  if (r.present) msg += `, ${r.present} already present`;
  if (r.conflicts) {
    msg += `, ${plural(r.conflicts, "conflict", "conflicts")} skipped (cue already used with a different text — kept yours)`;
  }
  return msg;
}
