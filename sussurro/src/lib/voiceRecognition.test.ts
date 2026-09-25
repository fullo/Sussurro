import { describe, expect, it } from "vitest";
import { forgetAllPrompt, formatSpeech, statusesById, toggleAction, voiceBadge, voiceStatusLabel } from "./voiceRecognition";
import type { VoiceStatus } from "./types";

const status = (over: Partial<VoiceStatus> = {}): VoiceStatus => ({
  person_id: "p-anna",
  enabled: true,
  speech_ms: 0,
  documents: 0,
  ready: false,
  min_speech_ms: 60_000,
  min_documents: 2,
  updated: "2026-09-25T10:00:00Z",
  ...over,
});

describe("formatSpeech", () => {
  it("reads seconds and minutes", () => {
    expect(formatSpeech(0)).toBe("0 s");
    expect(formatSpeech(45_000)).toBe("45 s");
    expect(formatSpeech(60_000)).toBe("1 min");
    expect(formatSpeech(95_400)).toBe("1 min 35 s");
    expect(formatSpeech(184_000)).toBe("3 min 4 s");
    expect(formatSpeech(-5)).toBe("0 s");
  });
});

describe("voiceStatusLabel / voiceBadge", () => {
  it("off when there is no profile", () => {
    expect(voiceStatusLabel(null)).toMatch(/^Off/);
    expect(voiceStatusLabel(status({ enabled: false }))).toMatch(/^Off/);
    expect(voiceBadge(undefined)).toBe("");
    expect(voiceBadge(status({ enabled: false }))).toBe("");
  });

  it("learning, with what is still missing", () => {
    expect(voiceStatusLabel(status())).toMatch(/no confirmed speech yet.*1 min from 2 documents/);
    const s = status({ speech_ms: 45_000, documents: 1 });
    expect(voiceStatusLabel(s)).toBe(
      "Learning: 45 s of 1 min of confirmed speech, from 1 of 2 documents. No suggestions until then.",
    );
    expect(voiceBadge(s)).toBe("learning voice");
  });

  it("ready, with seconds and documents", () => {
    const s = status({ speech_ms: 184_000, documents: 3, ready: true });
    expect(voiceStatusLabel(s)).toBe("Ready: 3 min 4 s of confirmed speech from 3 documents. Suggestions are on.");
    expect(voiceBadge(s)).toBe("voice recognised");
  });
});

describe("toggleAction", () => {
  it("asks before turning on, deletes when turned off, ignores no-ops", () => {
    expect(toggleAction(false, true)).toBe("confirm");
    expect(toggleAction(true, false)).toBe("off");
    expect(toggleAction(true, true)).toBe("none");
    expect(toggleAction(false, false)).toBe("none");
  });
});

describe("statusesById / forgetAllPrompt", () => {
  it("indexes statuses by person", () => {
    const by = statusesById([status(), status({ person_id: "p-bruno", ready: true })]);
    expect(Object.keys(by).sort()).toEqual(["p-anna", "p-bruno"]);
    expect(by["p-bruno"].ready).toBe(true);
  });

  it("says how many profiles go", () => {
    expect(forgetAllPrompt(0)).toMatch(/no voice profiles/);
    expect(forgetAllPrompt(1)).toMatch(/^Delete 1 person's voice profile from/);
    expect(forgetAllPrompt(3)).toMatch(/^Delete 3 people's voice profiles from/);
    // Your own voice (#243) goes with the rest.
    expect(forgetAllPrompt(0, true)).toMatch(/^Delete your own voice from/);
    expect(forgetAllPrompt(2, true)).toMatch(/^Delete 2 people's voice profiles and your own voice/);
  });
});
