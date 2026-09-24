import { describe, expect, it } from "vitest";
import {
  describeDictionaryMerge,
  describeSnippetMerge,
  mergeDictionary,
  mergeSnippets,
  normalizeCue,
  parseDictionaryFile,
  parseSnippetFile,
} from "./utils";

describe("File Parsing Helpers", () => {
  describe("parseDictionaryFile", () => {
    it("should parse a standard dictionary file", () => {
      const content = "Sussurro\nTauri\nDarumaHQ\n";
      const result = parseDictionaryFile(content);
      expect(result).toEqual(["Sussurro", "Tauri", "DarumaHQ"]);
    });

    it("should ignore empty lines and leading/trailing whitespace", () => {
      const content = "\n  Sussurro  \n\n  Tauri  \n";
      const result = parseDictionaryFile(content);
      expect(result).toEqual(["Sussurro", "Tauri"]);
    });

    it("should return an empty array for empty input", () => {
      const content = "";
      const result = parseDictionaryFile(content);
      expect(result).toEqual([]);
    });
  });

  describe("parseSnippetFile", () => {
    it("should parse a standard CSV snippet file", () => {
      const content = "firma email,Send the official signature\nnuovo paragrafo,New paragraph\n";
      const result = parseSnippetFile(content);
      expect(result).toEqual([
        { cue: "firma email", text: "Send the official signature" },
        { cue: "nuovo paragrafo", text: "New paragraph" },
      ]);
    });

    it("should handle different line endings", () => {
      const content = "cue1,text1\r\ncue2,text2\r\n";
      const result = parseSnippetFile(content);
      expect(result).toEqual([
        { cue: "cue1", text: "text1" },
        { cue: "cue2", text: "text2" },
      ]);
    });

    it("should ignore empty lines", () => {
      const content = "cue1,text1\n\ncue2,text2\n";
      const result = parseSnippetFile(content);
      expect(result).toEqual([
        { cue: "cue1", text: "text1" },
        { cue: "cue2", text: "text2" },
      ]);
    });

    it("should return an empty array for empty input", () => {
      const content = "";
      const result = parseSnippetFile(content);
      expect(result).toEqual([]);
    });

    it("should skip a cue,text header row", () => {
      expect(parseSnippetFile("Cue,Text\nsig,Best regards\n")).toEqual([
        { cue: "sig", text: "Best regards" },
      ]);
    });

    it("should keep quoted text with commas, escaped quotes and line breaks", () => {
      const content = 'firma email,"Best regards,\nFrancesco ""fullo"" Fullone"\r\nciao,hello\n';
      expect(parseSnippetFile(content)).toEqual([
        { cue: "firma email", text: 'Best regards,\nFrancesco "fullo" Fullone' },
        { cue: "ciao", text: "hello" },
      ]);
    });

    it("should keep unquoted extra commas as part of the text", () => {
      expect(parseSnippetFile("addr,Via Roma 1, Milano\n")).toEqual([
        { cue: "addr", text: "Via Roma 1, Milano" },
      ]);
    });

    it("should skip lines without both a cue and a text", () => {
      expect(parseSnippetFile("just a line\n,no cue\nno text,\n")).toEqual([]);
    });
  });

  describe("normalizeCue", () => {
    it("should ignore case, punctuation and extra spaces like the Rust matcher", () => {
      expect(normalizeCue("  Firma, Email! ")).toBe("firma email");
      expect(normalizeCue("firma   email")).toBe("firma email");
    });
  });

  describe("mergeDictionary", () => {
    it("should append only new words, deduping trimmed and case-insensitively", () => {
      const r = mergeDictionary(["Tauri", "Sussurro"], ["tauri", " SUSSURRO ", "DarumaHQ", "darumahq"]);
      expect(r.merged).toEqual(["Tauri", "Sussurro", "DarumaHQ"]);
      expect(r.added).toBe(1);
      expect(r.present).toBe(3);
      expect(describeDictionaryMerge(r)).toBe("Added 1 word, 3 already present");
    });

    it("should never modify or reorder existing entries", () => {
      const existing = ["b", "a"];
      const r = mergeDictionary(existing, ["c"]);
      expect(r.merged).toEqual(["b", "a", "c"]);
      expect(existing).toEqual(["b", "a"]);
    });

    it("should report nothing added when every word is already present", () => {
      const r = mergeDictionary(["Tauri"], ["TAURI"]);
      expect(r.merged).toEqual(["Tauri"]);
      expect(describeDictionaryMerge(r)).toBe("Added 0 words, 1 already present");
    });
  });

  describe("mergeSnippets", () => {
    const existing = [{ cue: "firma email", text: "Best regards" }];

    it("should add snippets with new cues", () => {
      const r = mergeSnippets(existing, [{ cue: "ciao", text: "hello" }]);
      expect(r.merged).toEqual([...existing, { cue: "ciao", text: "hello" }]);
      expect([r.added, r.present, r.conflicts]).toEqual([1, 0, 0]);
      expect(describeSnippetMerge(r)).toBe("Added 1 snippet");
    });

    it("should count an identical cue+text as already present", () => {
      const r = mergeSnippets(existing, [{ cue: "Firma Email!", text: " Best regards " }]);
      expect(r.merged).toEqual(existing);
      expect([r.added, r.present, r.conflicts]).toEqual([0, 1, 0]);
    });

    it("should keep the existing snippet on a cue conflict and count it", () => {
      const r = mergeSnippets(existing, [
        { cue: "firma email", text: "Cheers" },
        { cue: "new", text: "x" },
        { cue: "NEW", text: "y" },
      ]);
      expect(r.merged).toEqual([...existing, { cue: "new", text: "x" }]);
      expect([r.added, r.present, r.conflicts]).toEqual([1, 0, 2]);
      expect(describeSnippetMerge(r)).toBe(
        "Added 1 snippet, 2 conflicts skipped (cue already used with a different text — kept yours)",
      );
    });

    it("should not mutate the existing list", () => {
      const before = existing.slice();
      mergeSnippets(existing, [{ cue: "other", text: "t" }]);
      expect(existing).toEqual(before);
    });
  });
});
