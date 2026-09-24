import { describe, expect, it } from "vitest";
import {
  canAddSpeakerToPeople,
  identifyOffer,
  identifyVoicesArg,
  linkChoices,
  linkSuggestion,
  linkedPerson,
  isVoice,
  labelProblem,
  redetectBlocked,
  speakerOfLine,
  speakerShares,
  speakerSource,
  speakersEnabled,
} from "./speakers";
import type { DocSpeaker, Item, Person, Segment, VoiceSource } from "./types";

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
  const meeting = { meta: { type: "meeting" } } as Pick<Item, "meta">;
  const note = { meta: { type: "note" } } as Pick<Item, "meta">;
  const transcription = { meta: { type: "transcription" } } as Pick<Item, "meta">;

  it("is on for everything but notes", () => {
    // Notes never have speakers (P10).
    expect(speakersEnabled(note)).toBe(false);
    // Transcriptions always (P11, #134); meetings always (#138).
    expect(speakersEnabled(transcription)).toBe(true);
    expect(speakersEnabled(meeting)).toBe(true);
  });
});

describe("identifyVoicesArg", () => {
  it("is on only for a transcription with the box ticked", () => {
    expect(identifyVoicesArg("transcription", true)).toBe(true);
    expect(identifyVoicesArg("transcription", false)).toBe(false);
    // Ticked, then the type went back to Note: never sent.
    expect(identifyVoicesArg("note", true)).toBe(false);
    expect(identifyVoicesArg("meeting", true)).toBe(false);
  });
});

describe("identifyOffer", () => {
  const ok: VoiceSource = { available: true, reason: "", file_name: "a.wav" };
  const gone: VoiceSource = { available: false, reason: "The original file is gone.", file_name: "a.wav" };
  const t = (embedded = 0) => ({ meta: { type: "transcription" }, embedded_segments: embedded }) as Pick<Item, "meta" | "embedded_segments">;

  it("offers the button when the file is there, explains otherwise", () => {
    expect(identifyOffer(t(), ok)).toBe("identify");
    expect(identifyOffer(t(), gone)).toBe("explain");
  });

  it("offers nothing while unknown, with voice data, or off transcriptions", () => {
    expect(identifyOffer(t(), null)).toBe("none");
    expect(identifyOffer(t(3), ok)).toBe("none");
    const note = { meta: { type: "note" }, embedded_segments: 0 } as Pick<Item, "meta" | "embedded_segments">;
    const meeting = { meta: { type: "meeting" }, embedded_segments: 0 } as Pick<Item, "meta" | "embedded_segments">;
    expect(identifyOffer(note, ok)).toBe("none");
    expect(identifyOffer(meeting, ok)).toBe("none");
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

describe("people links", () => {
  const anna: Person = { id: "p-anna", name: "Anna Rossi", email: "anna@example.com", aliases: ["Anna R."] };
  const bob: Person = { id: "p-bob", name: "Bob", aliases: [] };
  const people = [bob, anna];

  it("suggests the person a Meet name or a rename matches, never for generic voices", () => {
    expect(linkSuggestion({ id: "meet:Anna R.", label: "Anna R.", color: "" }, people)).toEqual(anna);
    expect(linkSuggestion({ id: "voice:1", label: "  anna rossi ", color: "" }, people)).toEqual(anna);
    expect(linkSuggestion({ id: "voice:1", label: "Voice 1", color: "" }, people)).toBeNull();
    expect(linkSuggestion({ id: "voice:1", label: "Anna Rossi", color: "", person_id: "p-anna" }, people)).toBeNull();
  });

  it("finds the linked person and offers Add to People only for new names", () => {
    expect(linkedPerson({ id: "voice:1", label: "x", color: "", person_id: "p-anna" }, people)).toEqual(anna);
    expect(linkedPerson({ id: "voice:1", label: "x", color: "", person_id: "p-gone" }, people)).toBeNull();
    expect(canAddSpeakerToPeople({ id: "voice:1", label: "Luca", color: "" }, people)).toBe(true);
    expect(canAddSpeakerToPeople({ id: "voice:1", label: "Voice 1", color: "" }, people)).toBe(false);
    expect(canAddSpeakerToPeople({ id: "voice:1", label: "Anna R.", color: "" }, people)).toBe(false);
    expect(canAddSpeakerToPeople({ id: "voice:1", label: "Luca", color: "", person_id: "p-x" }, people)).toBe(false);
  });

  it("lists the suggested person first, then the rest by name", () => {
    expect(linkChoices({ id: "voice:1", label: "Anna R.", color: "" }, people).map((p) => p.id)).toEqual(["p-anna", "p-bob"]);
    expect(linkChoices({ id: "voice:1", label: "Voice 1", color: "" }, people).map((p) => p.id)).toEqual(["p-anna", "p-bob"]);
  });
});
