/* Rules every shipped selector set follows (#131, #245, #246): versioned
 * per platform, data only, attributes and structure only — never
 * obfuscated classes (Meet's semantic `notranslate` leaf is the one class
 * allowed) and never visible text or labels — and unverified until a live
 * check (#184) records a date. */
import { describe, expect, it } from "vitest";
import { watchedAttributes } from "./names/dom";
import { PROFILES, profileFor } from "./profiles";

const ALLOWED_CLASSES: Record<string, string[]> = { meet: [".notranslate"], teams: [], zoom: [] };

describe("platform profiles", () => {
  it("cover every platform, and nothing else", () => {
    expect(Object.keys(PROFILES).sort()).toEqual(["meet", "teams", "zoom"]);
    for (const [platform, p] of Object.entries(PROFILES)) expect(p.platform).toBe(platform);
    expect(profileFor(null)).toBeNull();
    expect(profileFor("zoom")?.sources.kind).toBe("ssrc");
    expect(profileFor("teams")?.sources).toMatchObject({ kind: "csrc", mirrorPolls: 0 });
    expect(profileFor("meet")?.sources).toMatchObject({ kind: "csrc", mirrorPolls: 3 });
  });

  it("ship versioned, data-only selector sets without classes or label matching", () => {
    const ids = new Set<string>();
    for (const [platform, p] of Object.entries(PROFILES)) {
      expect(p.sets.length).toBeGreaterThan(0);
      for (const s of p.sets) {
        expect(s.id).toMatch(new RegExp(`^${platform}-\\d{4}-\\d{2}[a-z]$`));
        expect(ids.has(s.id)).toBe(false);
        ids.add(s.id);
        // Unverified until #184 inspects a live page (then a date).
        expect(s.verifiedOn === null || /^\d{4}-\d{2}-\d{2}$/.test(s.verifiedOn)).toBe(true);
        for (const list of Object.values(s.hooks)) {
          for (const strat of list as { name: string; css?: string; attr?: string }[]) {
            expect(strat.name).toBeTruthy();
            const css = strat.css ?? "";
            const classes = css.match(/\.[A-Za-z_-][\w-]*/g) ?? [];
            expect(classes.every((c) => ALLOWED_CLASSES[platform].includes(c))).toBe(true);
            expect(css).not.toMatch(/aria-label|:contains|title=|\[class/);
            expect(strat.attr ?? "").not.toMatch(/^(aria-label|title|class)$/);
          }
        }
      }
      expect(watchedAttributes(p.sets).length).toBeGreaterThan(0);
    }
  });

  it("are still provisional: no set was checked on a live page yet (#184)", () => {
    // When #184 verifies a set, it records `verifiedOn` and updates this test.
    for (const p of Object.values(PROFILES)) for (const s of p.sets) expect(s.verifiedOn).toBeNull();
  });
});
