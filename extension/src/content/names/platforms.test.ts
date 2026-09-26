/* The engine's Teams/Zoom parts (#245, #246): SSRC and no-mirror tracking,
 * lag-aware votes, the user's tile from the mic, indicator coverage. */
import { describe, expect, it } from "vitest";
import { NameBinder, type LitTile } from "./binder";
import { assess } from "./health";
import { SelfFromMic } from "./self";
import { SourceTracker } from "./sources";

/** A poll where each receiver carries `sources` delivered at `ts`. */
const at = (ts: number, ...receivers: number[][]) => receivers.map((r) => r.map((source) => ({ source, timestamp: ts, audioLevel: 0.3 })));

describe("RTP source tracker, per platform", () => {
  it("names SSRCs by their own prefix (Zoom: one stream per participant)", () => {
    const s = new SourceTracker({ kind: "ssrc", hangMs: 600, mirrorPolls: 0, minLevel: 0.01 });
    expect(s.kind).toBe("ssrc");
    expect(s.update(100, at(90, [2001], [2002]))).toEqual([
      { kind: "active", id: "ssrc:2001", at: 90 },
      { kind: "active", id: "ssrc:2002", at: 90 },
    ]);
    // A quiet stream (comfort noise) is not speech.
    expect(s.update(200, [[{ source: 2001, timestamp: 190, audioLevel: 0.3 }], [{ source: 2002, timestamp: 190, audioLevel: 0.004 }]])).toEqual([
      { kind: "idle", id: "ssrc:2002", at: 90 },
    ]);
  });

  it("with the mirror rule off (Teams), a CSRC on the mix and its redundant track stays a speaker", () => {
    const s = new SourceTracker({ kind: "csrc", hangMs: 800, mirrorPolls: 0 });
    for (let t = 100; t <= 1_000; t += 100) s.update(t, at(t, [7], [7]));
    expect(s.isMirror(7)).toBe(false);
    expect(s.activeIds()).toEqual(["csrc:7"]);
    // An 800 ms hang bridges a 600 ms pause (Teams' stale entries).
    expect(s.update(1_600, at(1_000, [7]))).toEqual([]);
    expect(s.update(1_900, at(1_000, [7]))).toEqual([{ kind: "idle", id: "csrc:7", at: 1_000 }]);
  });
});

/** Teams/Zoom voting (the Teams profile's numbers). */
const LAGGED = { lockVotes: 5, margin: 3, voteDelayMs: 1_000, litHoldMs: 300, minEpisodes: 2, longEpisodeMs: 3_000 };

/**
 * A call replayed on a binder, polled every 100 ms: `turns` are
 * [source, name, ms] in order, each followed by 1 s of silence, and the
 * outline of the named tile lights `lagMs` after the turn starts and goes
 * out `lagMs` after it ends. Returns every binding change, in order.
 */
function replay(b: NameBinder, turns: [string, string, number][], lagMs: number, roster = ["Anna", "Bruno"]) {
  const spans: { id: string; name: string; from: number; to: number }[] = [];
  let t = 0;
  for (const [id, name, ms] of turns) {
    spans.push({ id, name, from: t, to: t + ms });
    t += ms + 1_000;
  }
  const out = [];
  for (let now = 0; now <= t + lagMs; now += 100) {
    const active = spans.filter((s) => now >= s.from && now < s.to).map((s) => s.id);
    const lit: LitTile[] = spans.filter((s) => now >= s.from + lagMs && now < s.to + lagMs).map((s) => ({ name: s.name }));
    out.push(...b.observe(now, active, lit, roster, null));
  }
  return out;
}

/** Back-to-back turns of 3 s (no silence between) with the outline 1 s
 *  late: each new speaker starts while the previous one's outline is lit. */
function alternation(b: NameBinder) {
  const turns = [
    ["csrc:1", "Anna"],
    ["csrc:2", "Bruno"],
    ["csrc:1", "Anna"],
    ["csrc:2", "Bruno"],
  ];
  const out = [];
  for (let now = 0; now < 12_000; now += 100) {
    const i = Math.floor(now / 3_000);
    const litI = Math.floor((now - 1_000) / 3_000);
    const lit = litI >= 0 ? [{ name: turns[litI][1] }] : [];
    out.push(...b.observe(now, [turns[i][0]], lit, ["Anna", "Bruno"], null));
  }
  return out;
}

describe("lag-aware name binder (Teams, Zoom)", () => {
  it("keeps a two-person call steady where instant votes fight the late outline", () => {
    // Meet's instant votes count every new turn against the previous
    // speaker's trailing outline: bindings get dropped mid-call.
    const instant = alternation(new NameBinder({ lockVotes: 5, margin: 3, unlockVotes: 10 }));
    expect(instant.some((b) => b.name === null)).toBe(true);
    const lagged = new NameBinder(LAGGED);
    expect(alternation(lagged)).toEqual([
      { id: "csrc:1", name: "Anna" },
      { id: "csrc:2", name: "Bruno" },
    ]);
  });

  it("binds only after a second turn, or one long turn", () => {
    // One 2.5 s turn: votes in its interior, but a single short episode.
    expect(replay(new NameBinder(LAGGED), [["csrc:1", "Anna", 2_500]], 1_000)).toEqual([]);
    expect(
      replay(
        new NameBinder(LAGGED),
        [
          ["csrc:1", "Anna", 2_500],
          ["csrc:1", "Anna", 2_500],
        ],
        1_000,
      ),
    ).toEqual([{ id: "csrc:1", name: "Anna" }]);
    // One turn voted over ≥ 3 s is enough.
    expect(replay(new NameBinder(LAGGED), [["csrc:1", "Anna", 5_000]], 1_000)).toEqual([{ id: "csrc:1", name: "Anna" }]);
  });

  it("never votes on an outline lit before the turn began, a flicker or an overlap", () => {
    // Anna's outline lit from before csrc:1's turn (typing, noise): it
    // never counts, however long the turn.
    const b = new NameBinder(LAGGED);
    for (let now = 0; now <= 6_000; now += 100) b.observe(now, now >= 500 ? ["csrc:1"] : [], [{ name: "Anna" }], ["Anna"], null);
    expect(b.bindings()).toEqual([]);
    // An outline that flickers every 200 ms never holds 300 ms.
    const f = new NameBinder(LAGGED);
    for (let now = 0; now <= 8_000; now += 100) f.observe(now, ["csrc:1"], Math.floor(now / 200) % 2 ? [{ name: "Anna" }] : [], ["Anna"], null);
    expect(f.bindings()).toEqual([]);
    // Two sources at once (overlap on the mix): no votes.
    const o = new NameBinder(LAGGED);
    for (let now = 0; now <= 8_000; now += 100) o.observe(now, ["csrc:1", "csrc:2"], now > 1_000 ? [{ name: "Anna" }] : [], ["Anna"], null);
    expect(o.bindings()).toEqual([]);
  });
});

describe("the user's own tile from the mic", () => {
  it("is the one tile lit while only our mic speaks, after the lag, and stays", () => {
    const s = new SelfFromMic({ holdMs: 1_000, votes: 10 });
    let decided: string | null = null;
    for (let now = 0; now <= 3_000 && !decided; now += 100) decided = s.observe(now, true, false, now >= 1_000 ? ["tile:self"] : []);
    expect(decided).toBe("tile:self");
    expect(s.key).toBe("tile:self");
    // Sticky: later evidence changes nothing.
    expect(s.observe(10_000, true, false, ["tile:other"])).toBeNull();
    expect(s.key).toBe("tile:self");
  });

  it("learns nothing while the remote side speaks, before the hold, or from two lit tiles", () => {
    const s = new SelfFromMic({ holdMs: 1_000, votes: 10 });
    for (let now = 0; now <= 5_000; now += 100) s.observe(now, true, true, ["tile:a"]);
    for (let now = 5_000; now <= 10_000; now += 100) s.observe(now, true, false, ["tile:a", "tile:b"]);
    // The mic drops every 800 ms: never held for 1 s.
    for (let now = 10_000; now <= 15_000; now += 100) s.observe(now, now % 800 !== 0, false, ["tile:a"]);
    expect(s.key).toBeNull();
    // Split evidence: no clear winner.
    const t = new SelfFromMic({ holdMs: 0, votes: 4 });
    for (let i = 0; i < 12; i++) t.observe(i * 100, true, false, [i % 2 ? "tile:a" : "tile:b"]);
    expect(t.key).toBeNull();
  });
});

describe("health: indicator coverage (Teams UI variants)", () => {
  const base = { set: "teams-2026-09a", tiles: 4, named: 4, rejected: 0, transitions: 0, speechMs: 0, csrcSeen: true, bound: 0, minCoverage: 0.5 };

  it("reports the speaking hook broken when too few tiles carry the indicator", () => {
    expect(assess(0, { ...base, now: 5_000, indicated: 0 }).state).toBe("ok"); // grace time
    const h = assess(0, { ...base, now: 15_000, indicated: 1 });
    expect(h.hooks.speaking).toBe("broken");
    expect(h.state).toBe("names_unavailable");
    expect(assess(0, { ...base, now: 15_000, indicated: 2 }).hooks.speaking).toBe("unknown");
    // Without the check (Meet, Zoom) the same page is fine.
    expect(assess(0, { ...base, now: 15_000, indicated: 0, minCoverage: 0 }).state).toBe("ok");
    // Names already bound survive a variant switch.
    expect(assess(0, { ...base, now: 15_000, indicated: 0, bound: 1 }).state).toBe("ok");
  });

  it("names the RTP hook after the source kind", () => {
    const h = assess(0, { ...base, now: 30_000, speechMs: 25_000, csrcSeen: false, transitions: 3, sourceKind: "ssrc", indicated: 4 });
    expect(h.hooks).toMatchObject({ ssrc: "missing", speaking: "ok" });
    expect(h.hooks.csrc).toBeUndefined();
  });
});
