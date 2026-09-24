import { describe, expect, it } from "vitest";
import { exportChoices, exportFileName, hasSubtitles, subtitlesInfo } from "./export";
import type { SubtitlesStatus } from "./types";

const status = (over: Partial<SubtitlesStatus> = {}): SubtitlesStatus => ({
  file: "transcript.srt",
  applicable: true,
  exists: false,
  edited_externally: false,
  ...over,
});

describe("exportChoices", () => {
  it("offers subtitles for meetings and transcriptions only", () => {
    expect(exportChoices("note").map((c) => c.format)).toEqual(["md", "txt"]);
    expect(exportChoices("meeting").map((c) => c.format)).toEqual(["md", "txt", "srt", "vtt"]);
    expect(exportChoices("transcription").map((c) => c.label)).toEqual([".md", ".txt", ".srt", ".vtt"]);
    expect(hasSubtitles("note")).toBe(false);
    expect(hasSubtitles("transcription")).toBe(true);
  });
});

describe("exportFileName", () => {
  it("uses the item folder name", () => {
    expect(exportFileName("2026/09/2026-09-24-weekly-sync", "srt")).toBe("2026-09-24-weekly-sync.srt");
    expect(exportFileName("", "md")).toBe("sussurro.md");
  });
});

describe("subtitlesInfo", () => {
  it("is null for notes", () => {
    expect(subtitlesInfo("note", "always", status(), false)).toBeNull();
  });

  it("offers Create, then Update, on request", () => {
    expect(subtitlesInfo("transcription", "on_request", status(), false)?.action).toBe("Create .srt");
    expect(subtitlesInfo("meeting", "on_request", status({ exists: true }), false)?.action).toBe("Update .srt");
    // Status not loaded yet: still offered.
    expect(subtitlesInfo("meeting", "on_request", null, false)?.action).toBe("Create .srt");
  });

  it("with Always, only offers creating a missing file", () => {
    const missing = subtitlesInfo("meeting", "always", status(), false);
    expect(missing?.action).toBe("Create .srt");
    const there = subtitlesInfo("meeting", "always", status({ exists: true }), false);
    expect(there?.action).toBeNull();
    expect(there?.note).toMatch(/updated every time/);
  });

  it("never offers overwriting a file edited outside, nor a live item", () => {
    const edited = subtitlesInfo("transcription", "on_request", status({ exists: true, edited_externally: true }), false);
    expect(edited?.action).toBeNull();
    expect(edited?.note).toMatch(/edited outside Sussurro/);
    expect(subtitlesInfo("meeting", "on_request", status(), true)?.action).toBeNull();
  });
});
