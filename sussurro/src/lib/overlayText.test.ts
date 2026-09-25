import { describe, expect, it } from "vitest";
import { levelToPercent, splitSettled, tailView } from "./overlayText";

describe("levelToPercent", () => {
  it("maps silence, full scale and out-of-range values", () => {
    expect(levelToPercent(0)).toBe(0);
    expect(levelToPercent(Number.NaN)).toBe(0);
    expect(levelToPercent(1)).toBe(100);
    expect(levelToPercent(2)).toBe(100);
    expect(levelToPercent(0.0001)).toBe(0); // −80 dBFS, below the scale
    expect(levelToPercent(0.001)).toBeCloseTo(0); // −60 dBFS
    expect(levelToPercent(0.0316)).toBeCloseTo(50, 0); // ≈ −30 dBFS
  });
});

describe("splitSettled", () => {
  it("treats the first pass as all ghost", () => {
    expect(splitSettled("", "hello there world")).toEqual({ settled: "", ghost: "hello there world" });
  });

  it("settles the agreed words but holds back the last ones", () => {
    const r = splitSettled("hello there my frend", "hello there my friend how are");
    expect(r.settled).toBe("hello there my ");
    expect(r.ghost).toBe("friend how are");
  });

  it("holds back the last words even when both passes agree on everything", () => {
    const r = splitSettled("one two three four", "one two three four");
    expect(r).toEqual({ settled: "one two ", ghost: "three four" });
  });

  it("always recomposes the original text", () => {
    const cases: [string, string][] = [
      ["a b c", "a  b\nc d"],
      ["x", ""],
      ["same text here", "different text entirely now"],
      ["  lead", "  lead space words"],
    ];
    for (const [prev, next] of cases) {
      const r = splitSettled(prev, next, 1);
      expect(r.settled + r.ghost).toBe(next);
    }
  });

  it("settles nothing when whisper revised the first word", () => {
    expect(splitSettled("Hello world again", "Yellow world again ok").settled).toBe("");
  });
});

describe("tailView", () => {
  it("leaves short text alone", () => {
    expect(tailView({ settled: "a b ", ghost: "c" }, 50)).toEqual({ settled: "a b ", ghost: "c", clipped: false });
  });

  it("keeps only the last words of a long text, cutting at a word boundary", () => {
    const words = Array.from({ length: 60 }, (_, i) => `word${i}`);
    const text = words.join(" ");
    const cut = text.length - 20;
    const v = tailView({ settled: text.slice(0, cut), ghost: text.slice(cut) }, 80);
    const shown = v.settled + v.ghost;
    expect(v.clipped).toBe(true);
    expect(shown.length).toBeLessThanOrEqual(80);
    expect(text.endsWith(shown)).toBe(true);
    expect(shown.startsWith("word")).toBe(true); // no half word
    expect(v.ghost).toBe(text.slice(cut)); // the ghost tail survives the cut
    expect(v.settled.length).toBeGreaterThan(0);
  });

  it("drops the settled part entirely when the ghost alone is long", () => {
    const ghost = " " + "ghost ".repeat(40).trim();
    const v = tailView({ settled: "settled words", ghost }, 30);
    expect(v.settled).toBe("");
    expect(v.clipped).toBe(true);
    expect(ghost.endsWith(v.ghost)).toBe(true);
    expect(v.ghost.startsWith("ghost")).toBe(true);
  });

  it("keeps a single overlong word whole instead of chopping it", () => {
    const v = tailView({ settled: "", ghost: "x".repeat(200) }, 50);
    expect(v).toEqual({ settled: "", ghost: "x".repeat(200), clipped: false });
  });
});
