import { describe, expect, it } from "vitest";
import { alsoSpeakingText, lineTimestamp, toLines } from "@sussurro/transcript";

describe("transcript helpers (via the @sussurro/transcript alias)", () => {
  it("formats line timestamps like transcript.md", () => {
    expect(lineTimestamp(0)).toBe("00:00:00");
    expect(lineTimestamp(27_400)).toBe("00:00:27");
    expect(lineTimestamp(3_723_000)).toBe("01:02:03");
  });

  it("drops blank segments but keeps failed ones as [not transcribed] rows", () => {
    const lines = toLines([
      { id: 0, start_ms: 0, text: "Ciao.", edited: true },
      { id: 1, start_ms: 1000, text: "  " },
      { id: 2, start_ms: 2000, text: "", stt_error: "model crashed" },
      { id: 3, start_ms: 3000, text: "Scritto a mano.", stt_error: "old error" },
    ]);
    expect(lines).toEqual([
      { id: 0, start_ms: 0, text: "Ciao.", edited: true },
      { id: 2, start_ms: 2000, text: "", sttError: "model crashed" },
      { id: 3, start_ms: 3000, text: "Scritto a mano." },
    ]);
  });

  it("attaches the listed speaker to each line", () => {
    const anna = { id: "voice:1", label: "Anna", color: "#0f766e" };
    const lines = toLines(
      [
        { id: 0, start_ms: 0, text: "Ciao.", speaker_id: "voice:1" },
        { id: 1, start_ms: 1000, text: "Chi parla?", speaker_id: "voice:9" },
        { id: 2, start_ms: 2000, text: "Nessuno." },
      ],
      [anna],
    );
    expect(lines.map((l) => l.speaker)).toEqual([anna, undefined, undefined]);
    // Without a speaker list nothing is attached (notes).
    expect(toLines([{ id: 0, start_ms: 0, text: "x", speaker_id: "voice:1" }])[0].speaker).toBeUndefined();
  });

  it("lists the other speakers of an overlapped line once (#244)", () => {
    const v1 = { id: "voice:1", label: "Voice 1", color: "#0f766e" };
    const v2 = { id: "voice:2", label: "Anna", color: "#7e22ce" };
    const v3 = { id: "voice:3", label: "Voice 3", color: "#1f6feb" };
    const lines = toLines(
      [
        {
          id: 0,
          start_ms: 0,
          text: "Sì, però",
          speaker_id: "voice:1",
          overlap: [
            { start_ms: 100, end_ms: 400, speaker_id: "voice:2" },
            { start_ms: 900, end_ms: 1200, speaker_id: "voice:2" },
            { start_ms: 1500, end_ms: 1800, speaker_id: "voice:3" },
            // Its own speaker or an unknown one never shows.
            { start_ms: 2000, end_ms: 2100, speaker_id: "voice:1" },
            { start_ms: 2200, end_ms: 2300, speaker_id: "voice:9" },
          ],
        },
        { id: 1, start_ms: 3000, text: "Chi?", speaker_id: "voice:2", overlap: [{ start_ms: 3000, end_ms: 3500 }] },
        { id: 2, start_ms: 4000, text: "Nessuno.", speaker_id: "voice:3" },
      ],
      [v1, v2, v3],
    );
    expect(lines[0].alsoSpeaking).toEqual([v2, v3]);
    expect(lines[1].alsoSpeaking).toEqual([]);
    expect(lines[2].alsoSpeaking).toBeUndefined();
    expect(alsoSpeakingText(lines[0].alsoSpeaking!)).toBe("+ Anna and Voice 3 also speaking");
    expect(alsoSpeakingText([v1])).toBe("+ Voice 1 also speaking");
    expect(alsoSpeakingText([v1, v2, v3])).toBe("+ Voice 1, Anna and Voice 3 also speaking");
    expect(alsoSpeakingText([])).toBe("overlapping speech");
    // Without a speaker list (notes, live runs) nothing is shown.
    expect(toLines([{ id: 0, start_ms: 0, text: "x", overlap: [{ start_ms: 0, end_ms: 500 }] }])[0].alsoSpeaking).toBeUndefined();
  });
});
