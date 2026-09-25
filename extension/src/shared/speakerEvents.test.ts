import { describe, expect, it } from "vitest";
import { FRAME_SAMPLES } from "./frame";
import { MAX_PENDING, MAX_SPEAKER_IDS, SpeakerRelay, frameToMs, perfToFrame, sanitizePageSpeaker, type PageSpeakerMsg } from "./speakerEvents";

const RATE = 48_000;
/** Page frames in `ms` at 48 kHz. */
const frames = (ms: number) => (ms * RATE) / 1000 / FRAME_SAMPLES;

describe("the page clock", () => {
  it("maps performance time to page frames from the last worklet block", () => {
    // Block 9 (frames 9..10) arrived at t = 5000 ms.
    expect(perfToFrame(5_000, { seq: 9, at: 5_000 }, 0, RATE)).toBe(10);
    expect(perfToFrame(5_000 + (FRAME_SAMPLES / RATE) * 1000, { seq: 9, at: 5_000 }, 0, RATE)).toBeCloseTo(11);
    // An event just before the block arrived lies inside it.
    expect(perfToFrame(4_990, { seq: 9, at: 5_000 }, 0, RATE)).toBeLessThan(10);
    // Before the first block: from the arm time.
    expect(perfToFrame(1_000, null, 1_000, RATE)).toBe(0);
    expect(perfToFrame(2_000, null, 1_000, RATE)).toBeCloseTo(frames(1_000));
    expect(perfToFrame(0, null, 1_000, RATE)).toBe(0);
  });

  it("maps page frames to ms on the connection's clock", () => {
    expect(frameToMs(frames(1_000), 0, RATE)).toBe(1_000);
    // This connection's first frame was page frame 100 (offset −100).
    expect(frameToMs(100 + frames(2_500), -100, RATE)).toBe(2_500);
    expect(frameToMs(50, -100, RATE)).toBe(0);
    expect(frameToMs(Number.NaN, 0, RATE)).toBe(0);
  });
});

describe("page messages are checked", () => {
  it("keeps well-formed messages and drops the rest", () => {
    expect(sanitizePageSpeaker({ type: "speaker_active", id: "csrc:1", name: "  Anna\n ", source: "rtp", pf: 3 })).toEqual({
      type: "speaker_active",
      id: "csrc:1",
      name: "Anna",
      source: "rtp",
      pf: 3,
    });
    expect(sanitizePageSpeaker({ type: "speaker_idle", id: "tile:0a", source: "weird", pf: -2 })).toEqual({ type: "speaker_idle", id: "tile:0a", source: "dom", pf: 0 });
    expect(sanitizePageSpeaker({ type: "speaker_name", id: "csrc:1", name: 42 })).toEqual({ type: "speaker_name", id: "csrc:1", name: null });
    expect(sanitizePageSpeaker({ type: "participants", names: ["A", "", 3, "x".repeat(200), "B"] })).toEqual({ type: "participants", names: ["A", "B"] });
    const health = sanitizePageSpeaker({ type: "observer_health", state: "ok", set: "meet-2026-09a", hooks: { tile: "ok", speaking: "fine", "bad key": "ok" } });
    expect(health).toEqual({ type: "observer_health", state: "ok", set: "meet-2026-09a", hooks: { tile: "ok", speaking: "unknown" } });
    for (const bad of [null, 1, "x", {}, { type: "speaker_active", id: "a b", pf: 1 }, { type: "speaker_active", id: "csrc:1" }, { type: "speaker_idle", id: "csrc:1", pf: Infinity }, { type: "speaker_name" }, { type: "participants", names: "A" }, { type: "dance" }]) {
      expect(sanitizePageSpeaker(bad)).toBeNull();
    }
  });
});

describe("speaker relay", () => {
  const active = (id: string, pf: number, name?: string): PageSpeakerMsg => ({ type: "speaker_active", id, source: "rtp", pf, ...(name ? { name } : {}) });

  it("holds timed messages until the connection's first page frame, then converts them", () => {
    const r = new SpeakerRelay();
    expect(r.begin(RATE)).toEqual([]);
    expect(r.page(active("csrc:1", 12), true)).toEqual([]);
    expect(r.page({ type: "speaker_name", id: "csrc:1", name: "Anna" }, true)).toEqual([{ type: "speaker_name", id: "csrc:1", name: "Anna" }]);
    // Page frame 10 went out as wire frame 0.
    expect(r.frame(10, 0)).toEqual([{ type: "speaker_active", t: frameToMs(2, 0, RATE), id: "csrc:1", source: "rtp" }]);
    expect(r.frame(11, 1)).toEqual([]);
    expect(r.page({ type: "speaker_idle", id: "csrc:1", source: "rtp", pf: 10 + frames(3_000) }, true)).toEqual([{ type: "speaker_idle", t: 3_000, id: "csrc:1" }]);
  });

  it("replays the page's state to a new connection (a reconnect is a new item)", () => {
    const r = new SpeakerRelay();
    r.begin(RATE);
    r.frame(0, 0);
    r.page({ type: "participants", names: ["Anna", "Bo"] }, true);
    r.page({ type: "speaker_name", id: "csrc:1", name: "Anna" }, true);
    r.page(active("csrc:1", 5, "Anna"), true);
    r.page(active("csrc:2", 6), true);
    r.page({ type: "speaker_idle", id: "csrc:2", source: "rtp", pf: 7 }, true);
    r.page({ type: "observer_health", state: "ok", set: "meet-2026-09a", hooks: { tile: "ok" } }, true);
    // While reconnecting, state only.
    expect(r.page({ type: "participants", names: ["Anna", "Bo", "Cy"] }, false)).toEqual([]);
    expect(r.lastHealth?.state).toBe("ok");
    expect(r.begin(RATE)).toEqual([
      { type: "participants", names: ["Anna", "Bo", "Cy"] },
      { type: "speaker_name", id: "csrc:1", name: "Anna" },
      { type: "observer_health", state: "ok", set: "meet-2026-09a", hooks: { tile: "ok" } },
      { type: "speaker_active", t: 0, id: "csrc:1", source: "rtp", name: "Anna" },
    ]);
    r.clear();
    expect(r.begin(RATE)).toEqual([]);
    expect(r.lastHealth).toBeNull();
  });

  it("bounds what it holds", () => {
    const r = new SpeakerRelay();
    r.begin(RATE);
    for (let i = 0; i < MAX_PENDING + 10; i++) r.page(active(`csrc:${i}`, i), true);
    expect(r.frame(0, 0)).toHaveLength(MAX_PENDING);
  });

  it("caps the speakers it keeps and drops repeated participant lists (#217)", () => {
    const r = new SpeakerRelay();
    r.begin(RATE);
    r.frame(0, 0);
    for (let i = 0; i < MAX_SPEAKER_IDS + 50; i++) r.page({ type: "speaker_name", id: `csrc:${i}`, name: `P${i}` }, true);
    expect(r.page({ type: "speaker_name", id: "csrc:new", name: "X" }, true)).toEqual([]);
    expect(r.page({ type: "speaker_name", id: "csrc:1", name: "Renamed" }, true)).toHaveLength(1);
    for (let i = 0; i < MAX_SPEAKER_IDS; i++) r.page(active(`csrc:${i}`, 1), true);
    expect(r.page(active("csrc:new", 1), true)).toEqual([]);
    expect(r.page(active("csrc:3", 2), true)).toHaveLength(1);
    const replay = r.begin(RATE);
    expect(replay.filter((m) => m.type === "speaker_name")).toHaveLength(MAX_SPEAKER_IDS);
    // The same participants again: nothing to send; a change is sent.
    r.frame(0, 0);
    const list = (names: string[]): PageSpeakerMsg => ({ type: "participants", names });
    expect(r.page(list(["Anna", "Bo"]), true)).toHaveLength(1);
    expect(r.page(list(["Anna", "Bo"]), true)).toEqual([]);
    expect(r.page(list(["Anna", "Bo"]), false)).toEqual([]);
    expect(r.page(list(["Anna"]), true)).toEqual([{ type: "participants", names: ["Anna"] }]);
  });
});
