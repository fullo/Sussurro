import { describe, expect, it } from "vitest";
import {
  activeCount,
  clearAll,
  clearFacet,
  clearTypes,
  dateLabel,
  EMPTY_FACETS,
  facetCount,
  facetSummary,
  filterValues,
  localToday,
  parseFacetState,
  setDate,
  toFilters,
  toggleType,
  toggleValue,
  validRange,
  withSelected,
  type FacetState,
  type FacetValue,
} from "./facets";

const v = (key: string, label: string, count: number): FacetValue => ({ key, label, count });

describe("facet selection", () => {
  it("toggles values within a facet (OR) and keeps their labels", () => {
    let s = toggleValue(EMPTY_FACETS, "tags", "release", "Release");
    s = toggleValue(s, "tags", "roadmap", "roadmap");
    s = toggleValue(s, "participants", "person:p-anna", "Anna Rossi");
    expect(s.tags).toEqual(["release", "roadmap"]);
    expect(s.participants).toEqual(["person:p-anna"]);
    expect(s.labels["participants:person:p-anna"]).toBe("Anna Rossi");
    s = toggleValue(s, "participants", "person:p-anna");
    expect(s.participants).toEqual([]);
    expect(s.labels["participants:person:p-anna"]).toBeUndefined();
    // The input state is never mutated.
    expect(EMPTY_FACETS.tags).toEqual([]);
  });

  it("type chips: toggling every type is the same as All", () => {
    let s = toggleType(EMPTY_FACETS, "note");
    s = toggleType(s, "meeting");
    expect(s.types).toEqual(["note", "meeting"]);
    expect(toggleType(s, "transcription").types).toEqual([]);
    expect(toggleType(s, "note").types).toEqual(["meeting"]);
    expect(clearTypes(s).types).toEqual([]);
  });

  it("clears one facet, or everything", () => {
    let s = toggleValue(EMPTY_FACETS, "tags", "a", "A");
    s = toggleValue(s, "categories", "team", "Team");
    s = setDate(toggleType(s, "note"), { bucket: "week" });
    expect(activeCount(s)).toBe(4);
    expect(facetCount(s)).toBe(3);
    const noTags = clearFacet(s, "tags");
    expect(noTags.tags).toEqual([]);
    expect(noTags.categories).toEqual(["team"]);
    expect(noTags.labels).toEqual({ "categories:team": "Team" });
    expect(clearFacet(s, "date").date).toBeNull();
    expect(activeCount(clearAll())).toBe(0);
  });
});

describe("filters sent to archive_facets", () => {
  it("maps the selection, the bucket and the viewer's today", () => {
    let s: FacetState = toggleValue(EMPTY_FACETS, "tags", "release");
    s = setDate(toggleType(s, "meeting"), { bucket: "month" });
    expect(toFilters(s, "2026-09-24")).toEqual({
      types: ["meeting"],
      tags: ["release"],
      categories: [],
      participants: [],
      date_bucket: "month",
      today: "2026-09-24",
    });
  });

  it("sends a custom range only when it is valid, open ends allowed", () => {
    const f = (from: string, to: string) => toFilters(setDate(EMPTY_FACETS, { from, to }), "2026-09-24");
    expect(f("2026-03-01", "2026-09-01")).toMatchObject({ date_from: "2026-03-01", date_to: "2026-09-01" });
    expect(f("2026-03-01", "")).toMatchObject({ date_from: "2026-03-01" });
    expect(f("2026-03-01", "")).not.toHaveProperty("date_to");
    expect(f("2026-09-01", "2026-03-01")).not.toHaveProperty("date_from");
    expect(validRange("", "")).toBe(false);
    expect(validRange("2026-01-01", "2026-01-01")).toBe(true);
    expect(validRange("2026-1-1", "")).toBe(false);
  });

  it("today is the local calendar day, not the UTC one", () => {
    // 00:30 local on 24 Sep: whatever the zone, the local day is the 24th.
    expect(localToday(new Date(2026, 8, 24, 0, 30))).toBe("2026-09-24");
    expect(localToday(new Date(2026, 0, 1, 23, 59))).toBe("2026-01-01");
  });
});

describe("facet values and labels", () => {
  const tags = [v("release", "Release", 12), v("roadmap", "roadmap", 3), v("caffè", "Caffè", 1)];

  it("lists selected values first, keeping ones the query no longer finds", () => {
    let s = toggleValue(EMPTY_FACETS, "tags", "roadmap", "roadmap");
    s = toggleValue(s, "tags", "gone", "Gone");
    const list = withSelected(tags, s.tags, s.labels, "tags");
    expect(list.map((x) => [x.key, x.count])).toEqual([
      ["roadmap", 3],
      ["gone", 0],
      ["release", 12],
      ["caffè", 1],
    ]);
    expect(list[1].label).toBe("Gone");
  });

  it("finds values ignoring case and accents", () => {
    expect(filterValues(tags, "CAFFE").map((x) => x.key)).toEqual(["caffè"]);
    expect(filterValues(tags, "  ")).toHaveLength(3);
  });

  it("summarises a facet for its chip", () => {
    let s = toggleValue(EMPTY_FACETS, "participants", "person:p-anna", "Anna Rossi");
    expect(facetSummary(s, "participants")).toBe("Participant: Anna Rossi");
    s = toggleValue(s, "participants", "name:voice 1");
    expect(facetSummary(s, "participants")).toBe("Participant: Anna Rossi +1");
    expect(facetSummary(s, "tags")).toBe("Tag");
    expect(facetSummary(EMPTY_FACETS, "date")).toBe("Date");
    expect(facetSummary(setDate(EMPTY_FACETS, { bucket: "week" }), "date")).toBe("Date: This week");
    expect(dateLabel({ from: "2026-03-01", to: "" })).toMatch(/^from /);
    expect(dateLabel({ from: "", to: "2026-03-01" })).toMatch(/^until /);
    expect(dateLabel({ from: "2026-03-01", to: "2026-03-01" })).not.toContain("–");
  });
});

describe("persistence", () => {
  it("round-trips a selection", () => {
    let s = toggleValue(EMPTY_FACETS, "categories", "team", "Team");
    s = setDate(toggleType(s, "transcription"), { from: "2026-01-01", to: "2026-02-01" });
    expect(parseFacetState(JSON.stringify(s))).toEqual(s);
  });

  it("drops anything malformed instead of failing", () => {
    expect(parseFacetState(null)).toEqual(EMPTY_FACETS);
    expect(parseFacetState("{not json")).toEqual(EMPTY_FACETS);
    expect(parseFacetState("[]")).toEqual(EMPTY_FACETS);
    const s = parseFacetState(
      JSON.stringify({
        types: ["note", "podcast", 3],
        tags: ["a", "a", "", 7, "b"],
        participants: "person:x",
        labels: { "tags:a": "A", "tags:zzz": "stale", "tags:b": 5 },
        date: { bucket: "decade" },
      }),
    );
    expect(s.types).toEqual(["note"]);
    expect(s.tags).toEqual(["a", "b"]);
    expect(s.participants).toEqual([]);
    expect(s.labels).toEqual({ "tags:a": "A" });
    expect(s.date).toBeNull();
    expect(parseFacetState(JSON.stringify({ date: { from: "2026-09-01", to: "2026-01-01" } })).date).toBeNull();
    expect(parseFacetState(JSON.stringify({ date: { bucket: "older" } })).date).toEqual({ bucket: "older" });
  });
});
