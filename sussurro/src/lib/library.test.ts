import { describe, expect, it } from "vitest";
import {
  addChips,
  filterByType,
  itemSubtitle,
  matchesQuery,
  parseChipInput,
  removeChip,
  splitHighlights,
  withDefaults,
} from "./library";
import type { ItemMeta, ItemSummary } from "./types";

const meta = (over: Partial<ItemMeta> = {}): ItemMeta => ({
  type: "note",
  title: "Idee per l'onboarding",
  date: new Date(2026, 8, 24, 8, 40).toISOString(),
  duration: "00:03:12",
  source: "mic",
  language: "it",
  engine: "whisper-small",
  tags: ["idee"],
  categories: [],
  participants: [],
  ...over,
});

const item = (id: string, over: Partial<ItemMeta> = {}): ItemSummary => ({
  id,
  meta: meta(over),
  edited_externally: false,
});

describe("filterByType / matchesQuery", () => {
  const items = [item("a"), item("b", { type: "transcription", title: "Podcast", tags: ["audio"] })];

  it("filters by type", () => {
    expect(filterByType(items, "all")).toHaveLength(2);
    expect(filterByType(items, "transcription").map((i) => i.id)).toEqual(["b"]);
    expect(filterByType(items, "meeting")).toEqual([]);
  });

  it("matches every word in title, tags or categories, case-insensitively", () => {
    expect(matchesQuery(items[1], "podcast AUDIO")).toBe(true);
    expect(matchesQuery(items[1], "podcast idee")).toBe(false);
    expect(matchesQuery(items[0], "  ")).toBe(true);
  });
});

describe("itemSubtitle", () => {
  it("joins date and length", () => {
    const now = new Date(2026, 8, 24, 12);
    expect(itemSubtitle(meta(), now)).toBe("Today 08:40 · 3 min");
    expect(itemSubtitle(meta({ duration: undefined }), now)).toBe("Today 08:40");
  });
});

describe("splitHighlights", () => {
  it("splits FTS markers into runs", () => {
    expect(splitHighlights("…the **archive** works on **all** systems")).toEqual([
      { text: "…the ", hit: false },
      { text: "archive", hit: true },
      { text: " works on ", hit: false },
      { text: "all", hit: true },
      { text: " systems", hit: false },
    ]);
  });

  it("does not highlight an unbalanced tail", () => {
    expect(splitHighlights("a **b")).toEqual([{ text: "a b", hit: false }]);
    expect(splitHighlights("")).toEqual([]);
  });
});

describe("chips", () => {
  it("parses comma lists", () => {
    expect(parseChipInput(" release, roadmap ,,")).toEqual(["release", "roadmap"]);
  });

  it("adds without case-insensitive duplicates and removes exact values", () => {
    expect(addChips(["Release"], ["release", "roadmap", " roadmap "])).toEqual(["Release", "roadmap"]);
    expect(removeChip(["a", "b"], "a")).toEqual(["b"]);
  });

  it("merges New's default tags and category only when something is new", () => {
    expect(withDefaults(meta(), ["idee"], [])).toBeNull();
    const m = withDefaults(meta({ extra_key: "kept" }), ["prodotto"], ["lavoro"])!;
    expect(m.tags).toEqual(["idee", "prodotto"]);
    expect(m.categories).toEqual(["lavoro"]);
    expect(m.extra_key).toBe("kept");
  });
});
