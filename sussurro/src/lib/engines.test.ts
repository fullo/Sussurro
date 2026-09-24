import { describe, expect, it } from "vitest";
import { ENGINES, detectsLanguage, engineDetail, engineLabel, engineSelectable, QWEN3_ASR_NOTE } from "./engines";

describe("engines", () => {
  it("list the three engines, Whisper first", () => {
    expect(ENGINES.map((e) => e.value)).toEqual(["whisper", "parakeet", "qwen3_asr"]);
    expect(engineLabel("qwen3_asr")).toBe("Qwen3-ASR");
    expect(engineDetail("qwen3_asr")).toContain("optional");
  });

  it("only Whisper takes a language hint", () => {
    expect(detectsLanguage("whisper")).toBe(false);
    expect(detectsLanguage("parakeet")).toBe(true);
    expect(detectsLanguage("qwen3_asr")).toBe(true);
  });

  it("offer Qwen3-ASR only when the sidecar is bundled, unless it is in use", () => {
    expect(engineSelectable("qwen3_asr", "whisper", false)).toBe(false);
    expect(engineSelectable("qwen3_asr", "whisper", true)).toBe(true);
    expect(engineSelectable("qwen3_asr", "qwen3_asr", false)).toBe(true);
    expect(engineSelectable("parakeet", "whisper", false)).toBe(true);
  });

  it("say Qwen3-ASR is optional and that turbo is better on Italian", () => {
    expect(QWEN3_ASR_NOTE).toMatch(/optional/i);
    expect(QWEN3_ASR_NOTE).toMatch(/large-v3-turbo is more accurate on Italian/);
  });
});
