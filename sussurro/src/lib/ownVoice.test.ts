import { describe, expect, it } from "vitest";
import {
  ENROL_PARAGRAPHS,
  canFinishEnrolment,
  enrolLanguage,
  enrolmentClock,
  enrolmentFull,
  enrolmentPercent,
  ownVoiceOffer,
  ownVoiceSummary,
  singleChannel,
} from "./ownVoice";
import type { DocSpeaker, Item, OwnVoiceStatus, Segment } from "./types";

const status = (enrolled: boolean): OwnVoiceStatus => ({
  enrolled,
  speech_ms: enrolled ? 29_400 : 0,
  label_as_you: enrolled,
  updated: enrolled ? "2026-09-25T10:00:00Z" : "",
  min_speech_ms: 20_000,
  target_ms: 30_000,
  max_ms: 90_000,
});

function item(
  source: string,
  speakers: DocSpeaker[],
  segments: Partial<Segment>[],
  extra: Partial<Item> = {},
): Item {
  return {
    id: "x",
    meta: { type: "meeting", title: "", source } as Item["meta"],
    segments: {
      version: 1,
      speakers,
      segments: segments.map((s, i) => ({ id: i, start_ms: 0, end_ms: 1000, raw: "", text: "a", ...s }) as Segment),
    },
    body: "",
    edited_externally: false,
    embedded_segments: 3,
    ...extra,
  };
}

const v = (n: number, extra: Partial<DocSpeaker> = {}): DocSpeaker => ({
  id: `voice:${n}`,
  label: `Voice ${n}`,
  color: "#000",
  ...extra,
});

describe("enrolment paragraph", () => {
  it("follows the dictation language, then the system's", () => {
    expect(enrolLanguage("it", "en-US")).toBe("it");
    expect(enrolLanguage("en", "it-IT")).toBe("en");
    expect(enrolLanguage("auto", "it-IT")).toBe("it");
    expect(enrolLanguage("auto", "de-DE")).toBe("en");
    expect(enrolLanguage("fr", "it")).toBe("it");
  });

  it("is about 30 s of reading in both languages", () => {
    for (const p of Object.values(ENROL_PARAGRAPHS)) {
      const words = p.split(/\s+/).length;
      // ~2.5 words/s at a meeting's pace.
      expect(words).toBeGreaterThan(65);
      expect(words).toBeLessThan(100);
    }
  });
});

describe("single-channel recordings", () => {
  it("are rooms, files and system audio without a separate mic", () => {
    expect(singleChannel(item("mic", [v(1)], [{ speaker_id: "voice:1", channel: "mic" }]))).toBe(true);
    expect(singleChannel(item("file:a.mp3", [v(1)], [{ speaker_id: "voice:1", channel: "file" }]))).toBe(true);
    expect(singleChannel(item("system", [v(1)], [{ speaker_id: "voice:1", channel: "system" }]))).toBe(true);
    expect(singleChannel(item("system", [v(1)], [{ channel: "mic" }]))).toBe(false);
    expect(singleChannel(item("browser:meet.google.com", [v(1)], []))).toBe(false);
    expect(singleChannel(item("mic", [v(1)], [{ speaker_id: "you" }]))).toBe(false);
  });
});

describe("the speaker panel's offer", () => {
  const room = item("mic", [v(1), v(2)], [{ speaker_id: "voice:1" }, { speaker_id: "voice:2" }]);

  it("asks to record the voice, then to find it", () => {
    expect(ownVoiceOffer(room, status(false))).toBe("enrol");
    expect(ownVoiceOffer(room, status(true))).toBe("find");
    expect(ownVoiceOffer(room, null)).toBe("none");
  });

  it("says when a voice was matched", () => {
    const matched = item("mic", [v(1), v(2, { label: "You", own_voice: true })], []);
    expect(ownVoiceOffer(matched, status(true))).toBe("matched");
    // Taken off by the user: offered again only as "find".
    const off = item("mic", [v(1), v(2, { own_voice: false })], []);
    expect(ownVoiceOffer(off, status(true))).toBe("find");
  });

  it("offers nothing where it can't work", () => {
    expect(ownVoiceOffer({ ...room, meta: { ...room.meta, type: "note" } }, status(true))).toBe("none");
    expect(ownVoiceOffer({ ...room, recording: true }, status(true))).toBe("none");
    expect(ownVoiceOffer({ ...room, edited_externally: true }, status(true))).toBe("none");
    expect(ownVoiceOffer({ ...room, embedded_segments: 0 }, status(true))).toBe("none");
    expect(ownVoiceOffer(item("browser:meet.google.com", [v(1)], []), status(false))).toBe("none");
  });
});

describe("enrolment progress", () => {
  const s = status(false);
  it("can stop after the minimum and saves at the maximum", () => {
    expect(canFinishEnrolment(20_000, s)).toBe(false);
    expect(canFinishEnrolment(24_000, s)).toBe(true);
    expect(canFinishEnrolment(20_000, { min_speech_ms: 20_000, target_ms: 10_000 })).toBe(true);
    expect(enrolmentFull(89_999, s)).toBe(false);
    expect(enrolmentFull(90_000, s)).toBe(true);
  });

  it("shows the share of the target and a clock", () => {
    expect(enrolmentPercent(0, s)).toBe(0);
    expect(enrolmentPercent(15_000, s)).toBe(50);
    expect(enrolmentPercent(60_000, s)).toBe(100);
    expect(enrolmentClock(12_400, s)).toBe("0:12 / 0:30");
    expect(enrolmentClock(75_000, s)).toBe("1:15 / 0:30");
  });

  it("summarises the status", () => {
    expect(ownVoiceSummary(null)).toBe("");
    expect(ownVoiceSummary(status(false))).toMatch(/Not recorded/);
    expect(ownVoiceSummary(status(true))).toBe("Recorded on 2026-09-25 · 29 s of speech");
  });
});
