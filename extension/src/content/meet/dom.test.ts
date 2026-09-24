// @vitest-environment happy-dom
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import { fits, pickSet, probe, watchedAttributes } from "./dom";
import { assess } from "./health";
import { tileKey } from "./names";
import { MEET_SETS } from "./selectors";
import type { SelectorSet } from "./selectors/types";

const HERE = dirname(fileURLToPath(import.meta.url));
const fixture = (name: string): Document =>
  new DOMParser().parseFromString(readFileSync(join(HERE, "fixtures", name), "utf8"), "text/html");

const [SET] = MEET_SETS;

describe("selector sets", () => {
  it("are data only, versioned and never match on classes other than the untranslated-name leaf", () => {
    const ids = new Set<string>();
    for (const s of MEET_SETS) {
      expect(s.id).toMatch(/^meet-\d{4}-\d{2}[a-z]$/);
      expect(ids.has(s.id)).toBe(false);
      ids.add(s.id);
      for (const list of Object.values(s.hooks)) {
        for (const strat of list as { name: string; css?: string }[]) {
          expect(strat.name).toBeTruthy();
          // No obfuscated class tokens: at most the semantic `notranslate`.
          const classes = (strat.css ?? "").match(/\.[A-Za-z_-][\w-]*/g) ?? [];
          expect(classes.every((c) => c === ".notranslate")).toBe(true);
          // No text or label matching (localized).
          expect(strat.css ?? "").not.toMatch(/aria-label|:contains|title=/);
        }
      }
    }
  });

  it("read tiles, names, the user's own tile and the speaking one", () => {
    const p = probe(fixture("grid-3.html"), SET);
    expect(p.set).toBe(SET.id);
    expect(p.tiles).toEqual([
      { key: tileKey("spaces/abc/devices/1"), name: "Ada Test", self: true, speaking: false },
      { key: tileKey("spaces/abc/devices/2"), name: "Bruno Esempio", self: false, speaking: true },
      { key: tileKey("spaces/abc/devices/3"), name: "Chiara Prova", self: false, speaking: false },
    ]);
    expect(p.selfName).toBe("Ada Test");
    expect(p.matched).toEqual({
      tile: "participant-id attribute",
      tileId: "participant-id attribute",
      tileName: "untranslated leaf",
      selfMarker: "self-name attribute",
      speaking: "audio-level attribute",
    });
    expect(fits(p, SET)).toBe(true);
    expect(p.tiles.every((t) => /^tile:[0-9a-f]{8}$/.test(t.key))).toBe(true);
  });

  it("find nothing on a page without tiles, and the health check says so after the grace time", () => {
    const doc = fixture("lobby.html");
    const p = probe(doc, SET);
    expect(p.tiles).toEqual([]);
    expect(p.matched.tile).toBeNull();
    expect(fits(p, SET)).toBe(false);
    // No set fits: the first one is used, and health reports it.
    expect(pickSet(doc, MEET_SETS).set.id).toBe(SET.id);
    const input = { set: SET.id, tiles: 0, named: 0, rejected: 0, transitions: 0, speechMs: 0, csrcSeen: false, bound: 0 };
    expect(assess(0, { ...input, now: 5_000 }).state).toBe("ok");
    const late = assess(0, { ...input, now: 15_000 });
    expect(late.state).toBe("names_unavailable");
    expect(late.hooks.tile).toBe("missing");
  });

  it("report broken names instead of guessing when the name leaf changes", () => {
    const p = probe(fixture("names-changed.html"), SET);
    expect(p.tiles.map((t) => t.name)).toEqual([null, null]);
    expect(p.rejectedNames).toBe(1);
    const h = assess(0, { now: 1_000, set: SET.id, tiles: 2, named: 0, rejected: p.rejectedNames, transitions: 0, speechMs: 0, csrcSeen: true, bound: 0 });
    expect(h.hooks.tileName).toBe("broken");
    expect(h.state).toBe("names_unavailable");
  });

  it("pick the first set that fits, newest first", () => {
    const broken: SelectorSet = {
      ...SET,
      id: "meet-2099-01a",
      hooks: { ...SET.hooks, tile: [{ name: "gone", css: "[data-no-such-thing]" }] },
    };
    const doc = fixture("grid-3.html");
    expect(pickSet(doc, [broken, SET]).set.id).toBe(SET.id);
    expect(pickSet(doc, [SET, broken]).set.id).toBe(SET.id);
    expect(watchedAttributes([SET])).toEqual(expect.arrayContaining(["data-participant-id", "data-self-name", "data-audio-level"]));
  });

  it("tolerate a selector the engine can't parse", () => {
    const odd: SelectorSet = { ...SET, hooks: { ...SET.hooks, tile: [{ name: "bad", css: "[[" }, ...SET.hooks.tile] } };
    expect(probe(fixture("grid-3.html"), odd).tiles).toHaveLength(3);
  });
});
