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
});
