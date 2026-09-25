import { describe, expect, it } from "vitest";
import { parseDictionaryFile, parseSnippetFile } from "../utils";
import {
  addDictionaryWords,
  dedupeDictionary,
  dictionaryDuplicateCount,
  dictionaryRows,
  duplicateCueCount,
  editDictionaryWord,
  exportDictionaryTxt,
  exportSnippetsCsv,
  pageOf,
  removeIndices,
  snippetDraftIssues,
  snippetRows,
} from "./personalization";

describe("dictionaryRows", () => {
  const words = ["Tauri", "Sussurro", "città", "tauri", "Zed", "whisper.cpp"];

  it("keeps the list order and each entry's index by default", () => {
    const rows = dictionaryRows(words, "", "added");
    expect(rows.map((r) => r.index)).toEqual([0, 1, 2, 3, 4, 5]);
    expect(rows.filter((r) => r.duplicate).map((r) => r.word)).toEqual(["tauri"]);
  });

  it("filters case- and accent-insensitively", () => {
    expect(dictionaryRows(words, "CITTA", "added").map((r) => r.word)).toEqual(["città"]);
    expect(dictionaryRows(words, " tau ", "added").map((r) => r.index)).toEqual([0, 3]);
    expect(dictionaryRows(words, "nothing", "added")).toEqual([]);
  });

  it("sorts A–Z and Z–A without losing indices", () => {
    const az = dictionaryRows(["b", "C", "a"], "", "az");
    expect(az.map((r) => [r.word, r.index])).toEqual([["a", 2], ["b", 0], ["C", 1]]);
    expect(dictionaryRows(["b", "C", "a"], "", "za").map((r) => r.word)).toEqual(["C", "b", "a"]);
  });

  it("stays fast with thousands of entries", () => {
    const big = Array.from({ length: 20_000 }, (_, i) => `word${i}`);
    const t = performance.now();
    const rows = dictionaryRows(big, "word19", "az");
    expect(rows.length).toBe(1111);
    expect(performance.now() - t).toBeLessThan(500);
  });
});

describe("dedupeDictionary", () => {
  it("keeps the first of each entry, trimmed, and drops blanks", () => {
    const r = dedupeDictionary(["Tauri", " tauri ", "", "Rust", "TAURI", "rust "]);
    expect(r).toEqual({ words: ["Tauri", "Rust"], removed: 4 });
    expect(dictionaryDuplicateCount(["Tauri", " tauri ", "", "Rust"])).toBe(2);
    expect(dictionaryDuplicateCount(["a", "b"])).toBe(0);
  });
});

describe("dictionary edits", () => {
  it("adds pasted lines, skipping what is already there", () => {
    const r = addDictionaryWords(["Tauri"], "tauri\n  Sussurro \n\nDarumaHQ");
    expect(r.merged).toEqual(["Tauri", "Sussurro", "DarumaHQ"]);
    expect(r.added).toBe(2);
    expect(r.present).toBe(1);
  });

  it("edits an entry in place, refusing blanks, line breaks and duplicates", () => {
    const words = ["Tauri", "Rust"];
    expect(editDictionaryWord(words, 1, "  Rustc ")).toEqual({ words: ["Tauri", "Rustc"] });
    expect(editDictionaryWord(words, 1, "rust")).toEqual({ words: ["Tauri", "rust"] }); // own entry
    expect(editDictionaryWord(words, 1, "TAURI")).toEqual({ error: "duplicate" });
    expect(editDictionaryWord(words, 1, "  ")).toEqual({ error: "empty" });
    expect(editDictionaryWord(words, 1, "a\nb")).toEqual({ error: "multiline" });
    expect(words).toEqual(["Tauri", "Rust"]); // never mutated
  });

  it("bulk-deletes by index", () => {
    expect(removeIndices(["a", "b", "c", "d"], [0, 2])).toEqual(["b", "d"]);
    expect(removeIndices(["a"], new Set<number>())).toEqual(["a"]);
  });
});

describe("snippetRows", () => {
  const snippets = [
    { cue: "firma", text: "Francesco Fullone" },
    { cue: "indirizzo", text: "Via Roma 1, Città" },
    { cue: "Firma!", text: "Another signature" },
    { cue: "", text: "orphan text" },
  ];

  it("flags cues shadowed by an earlier snippet and incomplete snippets", () => {
    const rows = snippetRows(snippets, "", "added");
    expect(rows.map((r) => r.shadowedBy)).toEqual([null, null, 0, null]);
    expect(rows.map((r) => r.sameCue)).toEqual([1, 0, 1, 0]);
    expect(rows.map((r) => r.incomplete)).toEqual([false, false, false, true]);
    expect(duplicateCueCount(snippets)).toBe(1);
  });

  it("searches cue and text, accent-insensitively", () => {
    expect(snippetRows(snippets, "citta", "added").map((r) => r.index)).toEqual([1]);
    expect(snippetRows(snippets, "FIRMA", "added").map((r) => r.index)).toEqual([0, 2]);
    expect(snippetRows(snippets, "signature", "added").map((r) => r.index)).toEqual([2]);
  });

  it("sorts by cue", () => {
    expect(snippetRows(snippets.slice(0, 3), "", "az").map((r) => r.index)).toEqual([0, 2, 1]);
    expect(snippetRows(snippets.slice(0, 3), "", "za").map((r) => r.index)).toEqual([1, 2, 0]); // "Firma!" sorts after "firma"
  });

  it("reports a draft whose cue another snippet already uses", () => {
    expect(snippetDraftIssues(snippets, { cue: "FIRMA", text: "x" }, -1)).toEqual({
      emptyCue: false,
      emptyText: false,
      cueUsedBy: 0,
    });
    // Editing snippet 0 itself: the only other "firma" is 2.
    expect(snippetDraftIssues(snippets, { cue: "firma", text: "x" }, 0).cueUsedBy).toBe(2);
    expect(snippetDraftIssues(snippets, { cue: " ", text: "" }, -1)).toEqual({
      emptyCue: true,
      emptyText: true,
      cueUsedBy: null,
    });
  });
});

describe("export round-trip through the import parsers", () => {
  it("dictionary .txt", () => {
    const words = ["Sussurro", "  whisper.cpp ", "", "New York Times", "DarumaHQ"];
    const txt = exportDictionaryTxt(words);
    expect(txt).toBe("Sussurro\nwhisper.cpp\nNew York Times\nDarumaHQ\n");
    expect(parseDictionaryFile(txt)).toEqual(["Sussurro", "whisper.cpp", "New York Times", "DarumaHQ"]);
    expect(exportDictionaryTxt([])).toBe("");
  });

  it("snippets .csv with commas, quotes, line breaks and a 'cue,text' lookalike", () => {
    const snippets = [
      { cue: "cue", text: "text" }, // would look like a header if it came first
      { cue: "firma", text: "Un saluto,\nFrancesco" },
      { cue: 'say "hi"', text: 'He said "hello", then left' },
      { cue: "plain", text: "no quoting needed" },
      { cue: "crlf", text: "line one\r\nline two" },
      { cue: "a, b", text: "cue with a comma" },
      { cue: "incomplete", text: "  " },
    ];
    const csv = exportSnippetsCsv(snippets);
    expect(csv.startsWith("cue,text\n")).toBe(true);
    expect(parseSnippetFile(csv)).toEqual(snippets.slice(0, 6));
  });

  it("snippets .csv is empty when there is nothing to export", () => {
    expect(exportSnippetsCsv([{ cue: "", text: "x" }])).toBe("");
  });

  it("round-trips thousands of entries", () => {
    const words = Array.from({ length: 5000 }, (_, i) => `term ${i}`);
    expect(parseDictionaryFile(exportDictionaryTxt(words))).toEqual(words);
    const snippets = Array.from({ length: 3000 }, (_, i) => ({ cue: `cue ${i}`, text: `text, ${i}\nsecond "line"` }));
    expect(parseSnippetFile(exportSnippetsCsv(snippets))).toEqual(snippets);
  });
});

describe("pageOf", () => {
  const rows = Array.from({ length: 250 }, (_, i) => i);
  it("slices pages and clamps out-of-range pages", () => {
    expect(pageOf(rows, 0).rows).toHaveLength(100);
    expect(pageOf(rows, 2)).toMatchObject({ page: 2, pages: 3 });
    expect(pageOf(rows, 2).rows).toEqual(rows.slice(200));
    expect(pageOf(rows, 9).page).toBe(2);
    expect(pageOf(rows, -1).page).toBe(0);
    expect(pageOf([], 3)).toEqual({ rows: [], page: 0, pages: 1 });
  });
});
