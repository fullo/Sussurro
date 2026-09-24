import { describe, expect, it } from "vitest";
import {
  isVoice,
  labelProblem,
  redetectBlocked,
  speakerOfLine,
  speakerShares,
  speakerSource,
  speakersEnabled,
} from "./speakers";
import type { DocSpeaker, Item, Segment } from "./types";

const v1: DocSpeaker = { id: "voice:1", label: "Voice 1", color: "#0f766e" };
const v2: DocSpeaker = { id: "voice:2", label: "Anna", color: "#7e22ce" };
const you: DocSpeaker = { id: "you", label: "You", color: "#1a1a1a" };

function seg(id: number, start: number, end: number, speaker?: string, text = "x"): Segment {
  return { id, start_ms: start, end_ms: end, raw: text, text, ...(speaker ? { speaker_id: speaker } : {}) };
}

function item(speakers: DocSpeaker[], segments: Segment[]): Pick<Item, "segments"> {
  return { segments: { version: 1, speakers, segments } };
}

describe("speakersEnabled", () => {
  it("needs the 0.9 preview and is never on for notes", () => {
    const meeting = { meta: { type: "meeting" } } as Pick<Item, "meta">;
    const note = { meta: { type: "note" } } as Pick<Item, "meta">;
    const transcription = { meta: { type: "transcription" } } as Pick<Item, "meta">;
    expect(speakersEnabled({ meetings_enabled: false }, meeting)).toBe(false);
    
    expect(speakersEnabled({ meetings_enabled: true }, meeting)).toBe(true);
    expect(speakersEnabled({ meetings_enabled: true }, transcription)).toBe(true);
    expect(speakersEnabled({ meetings_enabled: true }, note)).toBe(false);
  });
});

describe("speakerShares", () => {
  it("adds up each speaker's speech and rounds the percentages", () => {
    const shares = speakerShares(
      item(
        [you, v1, v2],
        [seg(0, 0, 3000, "you"), seg(1, 3000, 9000, "voice:1"), seg(2, 9000, 12000, "voice:2"), seg(3, 12000, 15000, "voice:1")],
      ),
    );
    expect(shares.map((s) => [s.speaker.id, s.ms, s.lines, s.percent])).toEqual([
      ["you", 3000, 1, 20],
      ["voice:1", 9000, 2, 60],
      ["voice:2", 3000, 1, 20],
    ]);
  });

  it("ignores empty lines and lines without a speaker, and lists silent speakers at 0 %", () => {
    const shares = speakerShares(item([v1, v2], [seg(0, 0, 4000, "voice:1"), seg(1, 4000, 8000, "voice:2", " "), seg(2, 8000, 9000)]));
    expect(shares.map((s) => s.percent)).toEqual([100, 0]);
    expect(speakerShares(item([v1], [])).map((s) => s.percent)).toEqual([0]);
  });
});

describe("speakerOfLine", () => {
  it("maps line ids to their listed speaker only", () => {
    const m = speakerOfLine(item([v1], [seg(0, 0, 1, "voice:1"), seg(1, 1, 2, "voice:9"), seg(2, 2, 3)]));
    expect([...m.entries()]).toEqual([[0, v1]]);
  });
});

describe("labels and sources", () => {
  it("names where a speaker comes from", () => {
    expect(speakerSource("you")).toBe("mic channel");
    expect(speakerSource("meet:Anna Rossi")).toBe("from Meet");
    expect(speakerSource("voice:3")).toBe("by voice");
    expect(speakerSource("other")).toBe("");
    expect(isVoice("voice:12")).toBe(true);
    expect(isVoice("voice:0")).toBe(false);
    expect(isVoice("voice:new")).toBe(false);
  });

  it("checks a new name the way the backend does", () => {
    expect(labelProblem("voice:1", "Anna Rossi")).toBe("");
    expect(labelProblem("voice:1", "   ")).toBe("");
    expect(labelProblem("you", " ")).toMatch(/needs a name/);
    expect(labelProblem("voice:1", "x".repeat(61))).toMatch(/60/);
    expect(labelProblem("voice:1", `  ${"x".repeat(60)}  `)).toBe("");
  });
});

describe("redetectBlocked", () => {
  it("needs voice data, a finished recording and an app-owned transcript", () => {
    expect(redetectBlocked({ embedded_segments: 4, edited_externally: false })).toBe("");
    expect(redetectBlocked({ embedded_segments: 0, edited_externally: false })).toMatch(/no voice data/);
    expect(redetectBlocked({ edited_externally: false })).toMatch(/no voice data/);
    expect(redetectBlocked({ embedded_segments: 4, edited_externally: true })).toMatch(/outside/);
    expect(redetectBlocked({ embedded_segments: 4, edited_externally: false, recording: true })).toMatch(/recording ends/);
  });
});
