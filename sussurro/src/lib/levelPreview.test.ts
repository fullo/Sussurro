import { describe, expect, it } from "vitest";
import {
  SILENCE_HINT_MS,
  isSilent,
  nextSilenceSince,
  shouldShowSilenceHint,
  silenceHintText,
} from "./levelPreview";

describe("isSilent", () => {
  it("treats 0 and negatives as silence, anything positive as signal", () => {
    expect(isSilent(0)).toBe(true);
    expect(isSilent(-1)).toBe(true);
    expect(isSilent(0.001)).toBe(false);
  });
});

describe("nextSilenceSince", () => {
  it("clears the timestamp the moment signal returns", () => {
    expect(nextSilenceSince(0.5, 1000, 2000)).toBeNull();
  });

  it("starts the clock on the first silent reading", () => {
    expect(nextSilenceSince(0, null, 1000)).toBe(1000);
  });

  it("keeps the original start across further silent readings", () => {
    expect(nextSilenceSince(0, 1000, 2500)).toBe(1000);
  });
});

describe("shouldShowSilenceHint", () => {
  it("is false with no signal history yet", () => {
    expect(shouldShowSilenceHint(null, 10_000)).toBe(false);
  });

  it("is false just under the threshold", () => {
    expect(shouldShowSilenceHint(0, SILENCE_HINT_MS - 1)).toBe(false);
  });

  it("is true at and after the threshold", () => {
    expect(shouldShowSilenceHint(0, SILENCE_HINT_MS)).toBe(true);
    expect(shouldShowSilenceHint(0, SILENCE_HINT_MS + 5000)).toBe(true);
  });

  it("composes with nextSilenceSince over a tick sequence", () => {
    let since: number | null = null;
    // Signal for the first 2s, then silence.
    since = nextSilenceSince(0.3, since, 0);
    since = nextSilenceSince(0.2, since, 1000);
    expect(shouldShowSilenceHint(since, 2000)).toBe(false);
    since = nextSilenceSince(0, since, 2000);
    expect(shouldShowSilenceHint(since, 2000)).toBe(false);
    since = nextSilenceSince(0, since, 4999);
    expect(shouldShowSilenceHint(since, 4999)).toBe(false);
    since = nextSilenceSince(0, since, 5000);
    expect(shouldShowSilenceHint(since, 5000)).toBe(true);
    // Signal resumes: the hint clears and the clock resets on the next silence.
    since = nextSilenceSince(0.4, since, 5100);
    expect(shouldShowSilenceHint(since, 5100)).toBe(false);
  });
});

describe("silenceHintText", () => {
  it("suggests playing something only for system audio", () => {
    expect(silenceHintText("mic")).not.toMatch(/play/i);
    expect(silenceHintText("system")).toMatch(/play something/i);
  });
});
