import { describe, expect, it } from "vitest";
import { differsFromDictation, effectiveRun, runArgs } from "./runOptions";
import type { Settings } from "./types";

const settings = (over: Partial<Settings> = {}) =>
  ({ engine: "whisper", language: "it", cleanup_level: "light", ...over }) as Settings;

describe("per-run options (#157)", () => {
  it("default to the dictation settings", () => {
    expect(effectiveRun(settings(), {})).toEqual({ language: "it", cleanupLevel: "light" });
    expect(effectiveRun(settings(), { language: null, cleanupLevel: null })).toEqual({ language: "it", cleanupLevel: "light" });
    expect(runArgs(settings(), {})).toEqual({ language: "it", cleanupLevel: "light" });
    expect(differsFromDictation(settings(), {})).toBe(false);
  });

  it("use the choice made in New", () => {
    const choice = { language: "en", cleanupLevel: "high" as const };
    expect(effectiveRun(settings(), choice)).toEqual({ language: "en", cleanupLevel: "high" });
    expect(runArgs(settings(), choice)).toEqual({ language: "en", cleanupLevel: "high" });
    expect(differsFromDictation(settings(), choice)).toBe(true);
    // Choosing the dictation's own value is not a difference.
    expect(differsFromDictation(settings(), { language: "it" })).toBe(false);
  });

  it("leave the language to Parakeet, which detects it", () => {
    const p = settings({ engine: "parakeet" });
    expect(effectiveRun(p, { language: "en" }).language).toBe("auto");
    expect(runArgs(p, { language: "en", cleanupLevel: "none" })).toEqual({ language: null, cleanupLevel: "none" });
    expect(differsFromDictation(p, { language: "en" })).toBe(false);
  });
});
