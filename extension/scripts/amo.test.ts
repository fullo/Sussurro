import { describe, expect, it } from "vitest";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { APPROVAL_NOTES_MAX, amoMetadata } from "./amo";

const notes = readFileSync(fileURLToPath(new URL("../AMO-REVIEWER-NOTES.md", import.meta.url)), "utf8");

describe("amoMetadata", () => {
  it("sends AMO-REVIEWER-NOTES.md as approval notes, within AMO's limit", () => {
    const m = amoMetadata(notes);
    expect(m.version.approval_notes).toBe(notes.trim());
    expect(m.version.approval_notes.length).toBeLessThanOrEqual(APPROVAL_NOTES_MAX);
  });

  it("declares Firefox desktop only (never Android)", () => {
    expect(amoMetadata(notes).version.compatibility).toEqual(["firefox"]);
  });

  it("explains every warning the lint gate accepts", () => {
    for (const code of ["UNSAFE_VAR_ASSIGNMENT", "KEY_FIREFOX_ANDROID_UNSUPPORTED_BY_MIN_VERSION", "setProp", "dangerouslySetInnerHTML"]) {
      expect(notes).toContain(code);
    }
  });

  it("refuses empty or over-long notes", () => {
    expect(() => amoMetadata(" \n")).toThrow(/empty/);
    expect(() => amoMetadata("x".repeat(APPROVAL_NOTES_MAX + 1))).toThrow(/at most 3000/);
    expect(amoMetadata("a\r\nb").version.approval_notes).toBe("a\nb");
  });
});
