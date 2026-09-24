import { describe, expect, it } from "vitest";
import { lineTimestamp, toLines } from "@sussurro/transcript";

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
    // Without a speaker list nothing is attached (notes, the classic UI).
    expect(toLines([{ id: 0, start_ms: 0, text: "x", speaker_id: "voice:1" }])[0].speaker).toBeUndefined();
  });
});
