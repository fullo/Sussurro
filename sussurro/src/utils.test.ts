import { parseDictionaryFile, parseSnippetFile } from "./utils";

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
  });
});
