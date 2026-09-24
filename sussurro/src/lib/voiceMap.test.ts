import { describe, expect, it } from "vitest";
import {
  NO_SPEAKER_COLOR,
  VIEW_H,
  VIEW_PAD,
  VIEW_W,
  layoutVoiceMap,
  mapCaption,
  nearestPoint,
  pointLabel,
  pointRadius,
  shorten,
  stepPoint,
  voiceMapShown,
} from "./voiceMap";
import type { DocSpeaker, Item, ItemMeta, Segment, VoiceMap } from "./types";

const v1: DocSpeaker = { id: "voice:1", label: "Voice 1", color: "#0f766e" };
const v2: DocSpeaker = { id: "voice:2", label: "Anna", color: "#7e22ce" };

function seg(id: number, start: number, speaker?: string, text = `line ${id}`): Segment {
  return { id, start_ms: start, end_ms: start + 900, raw: text, text, ...(speaker ? { speaker_id: speaker } : {}) };
}

function item(speakers: DocSpeaker[], segments: Segment[]): Pick<Item, "segments"> {
  return { segments: { version: 1, speakers, segments } };
}

const meta = (type: string) => ({ type }) as unknown as ItemMeta;

describe("voiceMapShown", () => {
  it("needs voice data on two lines, and never on notes or while recording", () => {
    expect(voiceMapShown({ meta: meta("meeting"), embedded_segments: 12 })).toBe(true);
    expect(voiceMapShown({ meta: meta("transcription"), embedded_segments: 2 })).toBe(true);
    expect(voiceMapShown({ meta: meta("transcription"), embedded_segments: 1 })).toBe(false);
    expect(voiceMapShown({ meta: meta("meeting") })).toBe(false);
    expect(voiceMapShown({ meta: meta("note"), embedded_segments: 40 })).toBe(false);
    expect(voiceMapShown({ meta: meta("meeting"), embedded_segments: 40, recording: true })).toBe(false);
  });
});

describe("layoutVoiceMap", () => {
  const map: VoiceMap = {
    points: [
      { segment_id: 2, speaker_id: "voice:1", x: -1, y: 0.5 },
      { segment_id: 1, speaker_id: "voice:1", x: 1, y: -0.5 },
      { segment_id: 3, speaker_id: "voice:2", x: 0, y: 0 },
      { segment_id: 9, speaker_id: "voice:2", x: 5, y: 5 }, // line deleted since
    ],
    total: 4,
    explained: 0.42,
  };
  const doc = item(
    [v2, v1],
    [seg(1, 1000, "voice:1"), seg(2, 61_000, "voice:2", "  moved   to Anna  "), seg(3, 3_600_000)],
  );

  it("takes speakers, colours and text from the item as it is now", () => {
    const { points } = layoutVoiceMap(map, doc);
    expect(points.map((p) => p.segmentId)).toEqual([1, 2, 3]); // time order, deleted line dropped
    const [a, b, c] = points;
    expect([a.speakerId, a.label, a.color]).toEqual(["voice:1", "Voice 1", "#0f766e"]);
    // Line 2 was moved to Anna after the map was computed.
    expect([b.speakerId, b.label, b.color, b.text]).toEqual(["voice:2", "Anna", "#7e22ce", "moved to Anna"]);
    expect([c.speakerId, c.label, c.color, c.time]).toEqual([null, "No speaker", NO_SPEAKER_COLOR, "01:00:00"]);
    expect(b.time).toBe("00:01:01");
  });

  it("fits the points in the viewBox with one scale, +y up", () => {
    const { points } = layoutVoiceMap(map, doc);
    const [a, b, c] = points;
    // x spans 2, y spans 1: x decides the scale and fills the width.
    expect(a.x).toBeCloseTo(VIEW_W - VIEW_PAD);
    expect(b.x).toBeCloseTo(VIEW_PAD);
    expect(c.x).toBeCloseTo(VIEW_W / 2);
    expect(c.y).toBeCloseTo(VIEW_H / 2);
    const scale = (VIEW_W - 2 * VIEW_PAD) / 2;
    expect(b.y).toBeCloseTo(VIEW_H / 2 - 0.5 * scale); // y = +0.5 is above the centre
    expect(a.y).toBeCloseTo(VIEW_H / 2 + 0.5 * scale);
    for (const p of points) {
      expect(p.x).toBeGreaterThanOrEqual(VIEW_PAD - 1e-9);
      expect(p.y).toBeGreaterThanOrEqual(VIEW_PAD - 1e-9);
      expect(p.y).toBeLessThanOrEqual(VIEW_H - VIEW_PAD + 1e-9);
    }
  });

  it("puts every point in the centre when there is no spread", () => {
    const flat: VoiceMap = {
      points: [
        { segment_id: 1, x: 0, y: 0 },
        { segment_id: 2, x: 0, y: 0 },
      ],
      total: 2,
      explained: 0,
    };
    const { points } = layoutVoiceMap(flat, doc);
    expect(points.every((p) => p.x === VIEW_W / 2 && p.y === VIEW_H / 2)).toBe(true);
    expect(layoutVoiceMap({ points: [], total: 0, explained: 0 }, doc)).toEqual({ points: [], legend: [] });
  });

  it("builds the legend in the document's speaker order, no speaker last", () => {
    const { legend } = layoutVoiceMap(map, doc);
    expect(legend).toEqual([
      { id: "voice:2", label: "Anna", color: "#7e22ce", count: 1 },
      { id: "voice:1", label: "Voice 1", color: "#0f766e", count: 1 },
      { id: null, label: "No speaker", color: NO_SPEAKER_COLOR, count: 1 },
    ]);
    // A speaker without points on the map is left out.
    const only1 = layoutVoiceMap({ ...map, points: map.points.slice(1, 2) }, doc);
    expect(only1.legend.map((l) => l.id)).toEqual(["voice:1"]);
  });

  it("labels a failed line and shortens long text", () => {
    const failed = item([v1], [{ ...seg(1, 0, "voice:1", ""), stt_error: "boom" }, seg(2, 5, "voice:1", "word ".repeat(80))]);
    const { points } = layoutVoiceMap(
      {
        points: [
          { segment_id: 1, x: 0, y: 0 },
          { segment_id: 2, x: 1, y: 1 },
        ],
        total: 2,
        explained: 0.5,
      },
      failed,
    );
    expect(points[0].text).toBe("[not transcribed]");
    expect(points[1].text.endsWith("…")).toBe(true);
    expect([...points[1].text].length).toBeLessThanOrEqual(141);
    expect(pointLabel(points[0])).toBe("Voice 1, 00:00:00: [not transcribed]");
  });
});

describe("shorten", () => {
  it("cuts at a word boundary", () => {
    expect(shorten("short")).toBe("short");
    expect(shorten("alpha beta gamma delta", 12)).toBe("alpha beta…");
    expect(shorten("abcdefghijklmnop", 5)).toBe("abcde…");
  });
});

describe("hit-testing and keyboard", () => {
  const pts = [
    { x: 10, y: 10 },
    { x: 20, y: 10 },
    { x: 100, y: 100 },
  ];
  it("finds the nearest point within reach", () => {
    expect(nearestPoint(pts, 14, 10, 8)).toBe(0);
    expect(nearestPoint(pts, 17, 11, 8)).toBe(1);
    expect(nearestPoint(pts, 60, 60, 8)).toBe(-1);
    expect(nearestPoint([], 0, 0, 8)).toBe(-1);
  });

  it("steps through the lines in time order", () => {
    expect(stepPoint(5, -1, "ArrowRight")).toBe(0);
    expect(stepPoint(5, -1, "ArrowLeft")).toBe(4);
    expect(stepPoint(5, 2, "ArrowDown")).toBe(3);
    expect(stepPoint(5, 4, "ArrowRight")).toBe(4);
    expect(stepPoint(5, 0, "ArrowUp")).toBe(0);
    expect(stepPoint(50, 5, "PageDown")).toBe(15);
    expect(stepPoint(50, 5, "PageUp")).toBe(0);
    expect(stepPoint(5, 2, "Home")).toBe(0);
    expect(stepPoint(5, 2, "End")).toBe(4);
    expect(stepPoint(5, 2, "a")).toBeNull();
    expect(stepPoint(0, -1, "ArrowRight")).toBeNull();
  });

  it("shrinks the dots as the map fills up", () => {
    expect(pointRadius(10)).toBeGreaterThan(pointRadius(300));
    expect(pointRadius(300)).toBeGreaterThan(pointRadius(5000));
  });
});

describe("mapCaption", () => {
  it("says what the map shows, how approximate it is, and when it is sampled", () => {
    const c = mapCaption({ points: new Array(40), total: 40, explained: 0.337 });
    expect(c).toContain("Closeness is approximate");
    expect(c).toContain("about 34%");
    expect(c).not.toContain("Showing");
    const s = mapCaption({ points: new Array(5000), total: 12345, explained: 0.2 });
    expect(s).toContain("Showing 5,000 of 12,345 lines.");
    expect(mapCaption({ points: [], total: 0, explained: 0 })).not.toContain("%");
  });
});
