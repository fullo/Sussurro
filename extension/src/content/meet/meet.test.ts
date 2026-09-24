import { describe, expect, it } from "vitest";
import { NameBinder } from "./binder";
import { CsrcTracker, csrcId, toPerfTime } from "./csrc";
import { assess, sameReport } from "./health";
import { cleanName, nameKey, tileKey } from "./names";

describe("name guard", () => {
  it("keeps people's names as shown, whitespace collapsed", () => {
    expect(cleanName("  Anna   Rossi\n")).toBe("Anna Rossi");
    expect(cleanName("José Ñúñez")).toBe("José Ñúñez");
    expect(cleanName("李雷")).toBe("李雷");
    expect(cleanName("Anna (Guest)")).toBe("Anna (Guest)");
  });

  it("rejects what is not a name", () => {
    for (const bad of [null, undefined, "", "   ", "12:34", "3", "…", "spaces/abc/devices/1", "devices/9", "You", "(you)", "Tu", "Voi", "x".repeat(121)]) {
      expect(cleanName(bad as string)).toBeNull();
    }
  });

  it("compares names without case, accents or spacing; hashes tile ids", () => {
    expect(nameKey(" José  NUÑEZ ")).toBe(nameKey("jose nunez"));
    expect(tileKey("spaces/a/devices/1")).toBe(tileKey("spaces/a/devices/1"));
    expect(tileKey("spaces/a/devices/1")).not.toBe(tileKey("spaces/a/devices/2"));
    expect(tileKey("x")).toMatch(/^tile:[0-9a-f]{8}$/);
  });
});

/** A poll where each receiver carries `sources` delivered at `ts`. */
const at = (ts: number, ...receivers: number[][]) => receivers.map((r) => r.map((source) => ({ source, timestamp: ts, audioLevel: 0.3 })));

describe("CSRC tracker", () => {
  it("turns fresh contributing sources into active/idle with the packet times", () => {
    const c = new CsrcTracker({ hangMs: 400 });
    expect(c.update(0, [])).toEqual([]);
    expect(c.seen).toBe(false);
    expect(c.update(100, at(90, [11], []))).toEqual([{ kind: "active", id: "csrc:11", at: 90 }]);
    expect(c.seen).toBe(true);
    // Between words: packets 300 ms old still count.
    expect(c.update(400, at(100, [11]))).toEqual([]);
    expect(c.activeIds()).toEqual(["csrc:11"]);
    // Another participant on the second stream: both active.
    expect(c.update(500, at(480, [11], [22]))).toEqual([{ kind: "active", id: "csrc:22", at: 480 }]);
    // Silence: idle at the last packet.
    expect(c.update(1_000, at(480, [11], [22]))).toEqual([
      { kind: "idle", id: "csrc:11", at: 480 },
      { kind: "idle", id: "csrc:22", at: 480 },
    ]);
    expect(c.activeIds()).toEqual([]);
  });

  it("ignores quiet packets and a mirror on two receivers", () => {
    const c = new CsrcTracker({ hangMs: 400, mirrorPolls: 3, minLevel: 0.01 });
    expect(c.update(100, [[{ source: 5, timestamp: 100, audioLevel: 0.001 }]])).toEqual([]);
    // 7 speaks on stream 1 and is echoed on stream 2: a mirror after 3 polls.
    expect(c.update(200, at(200, [7], [7]))).toEqual([{ kind: "active", id: "csrc:7", at: 200 }]);
    c.update(300, at(300, [7], [7]));
    expect(c.update(400, at(400, [7], [7]))).toEqual([{ kind: "idle", id: "csrc:7", at: 300 }]);
    expect(c.isMirror(7)).toBe(true);
    // Sticky: even alone on one stream it no longer counts.
    expect(c.update(500, at(500, [7]))).toEqual([]);
  });

  it("stops every active source at once, and reads epoch timestamps", () => {
    const c = new CsrcTracker();
    c.update(100, at(100, [1, 2]));
    expect(c.stop().map((x) => x.id)).toEqual(["csrc:1", "csrc:2"]);
    expect(toPerfTime(1_700_000_000_500, 1_000, 1_700_000_000_000)).toBe(500);
    expect(toPerfTime(900, 1_000, 1_700_000_000_000)).toBe(900);
    expect(csrcId(-1)).toBe("csrc:4294967295");
  });
});

describe("name binder", () => {
  const lit = (name: string) => [{ name }];
  const roster = ["Bruno", "Chiara"];

  it("locks after enough agreeing votes with a margin, then keeps the name", () => {
    const b = new NameBinder({ lockVotes: 5, margin: 3 });
    for (let i = 0; i < 4; i++) expect(b.observe(i * 100, ["csrc:1"], lit("Bruno"), roster, "Ada")).toEqual([]);
    expect(b.observe(400, ["csrc:1"], lit("Bruno"), roster, "Ada")).toEqual([{ id: "csrc:1", name: "Bruno" }]);
    expect(b.nameOf("csrc:1")).toBe("Bruno");
    // Agreeing votes change nothing.
    expect(b.observe(500, ["csrc:1"], lit("bruno"), roster, "Ada")).toEqual([]);
  });

  it("does not vote on overlaps, several lit tiles, namesakes or the user's own name", () => {
    const b = new NameBinder({ lockVotes: 1, margin: 1 });
    expect(b.observe(0, ["csrc:1", "csrc:2"], lit("Bruno"), roster, null)).toEqual([]);
    expect(b.observe(0, ["csrc:1"], [{ name: "Bruno" }, { name: "Chiara" }], roster, null)).toEqual([]);
    expect(b.observe(0, ["csrc:1"], lit("Bruno"), ["Bruno", "bruno "], null)).toEqual([]);
    expect(b.observe(0, ["csrc:1"], lit("Ada"), ["Ada"], "ada")).toEqual([]);
    expect(b.observe(0, [], lit("Bruno"), roster, null)).toEqual([]);
    expect(b.bindings()).toEqual([]);
  });

  it("needs a margin when the glow is mixed", () => {
    const b = new NameBinder({ lockVotes: 3, margin: 3 });
    const seq = ["Bruno", "Chiara", "Bruno", "Chiara", "Bruno", "Bruno", "Bruno"];
    const out = seq.flatMap((n, i) => b.observe(i, ["csrc:1"], lit(n), roster, null));
    // Bruno 5 vs Chiara 2 at the last vote: margin 3 reached only then.
    expect(out).toEqual([{ id: "csrc:1", name: "Bruno" }]);
  });

  it("keeps one live CSRC per name, but lets a rejoin take it later", () => {
    const b = new NameBinder({ lockVotes: 1, margin: 1, rejoinMs: 30_000 });
    expect(b.observe(0, ["csrc:1"], lit("Bruno"), roster, null)).toEqual([{ id: "csrc:1", name: "Bruno" }]);
    expect(b.observe(1_000, ["csrc:2"], lit("Bruno"), roster, null)).toEqual([]);
    // csrc:1 unheard for 30 s: Bruno rejoined with a new CSRC.
    expect(b.observe(40_000, ["csrc:2"], lit("Bruno"), roster, null)).toEqual([{ id: "csrc:2", name: "Bruno" }]);
  });

  it("drops a binding after sustained contradiction", () => {
    const b = new NameBinder({ lockVotes: 1, margin: 1, unlockVotes: 3 });
    b.observe(0, ["csrc:1"], lit("Bruno"), roster, null);
    expect(b.observe(1, ["csrc:1"], lit("Chiara"), roster, null)).toEqual([]);
    expect(b.observe(2, ["csrc:1"], lit("Chiara"), roster, null)).toEqual([]);
    // Third contradiction: dropped, and this vote starts a fresh tally.
    expect(b.observe(3, ["csrc:1"], lit("Chiara"), roster, null)).toEqual([
      { id: "csrc:1", name: null },
      { id: "csrc:1", name: "Chiara" },
    ]);
  });
});

describe("health", () => {
  const base = { set: "meet-2026-09a", tiles: 3, named: 3, rejected: 0, transitions: 0, speechMs: 0, csrcSeen: true, bound: 0 };

  it("flags a speaking marker that never lights while the audio shows speech", () => {
    expect(assess(0, { ...base, now: 30_000, speechMs: 10_000 }).state).toBe("ok");
    const broken = assess(0, { ...base, now: 30_000, speechMs: 25_000 });
    expect(broken.hooks.speaking).toBe("broken");
    expect(broken.state).toBe("names_unavailable");
    // Names already bound to CSRCs keep working without the glow.
    expect(assess(0, { ...base, now: 30_000, speechMs: 25_000, bound: 2 }).state).toBe("ok");
    expect(assess(0, { ...base, now: 30_000, speechMs: 25_000, transitions: 4 }).hooks.speaking).toBe("ok");
  });

  it("reports a transport without CSRCs", () => {
    const h = assess(0, { ...base, now: 30_000, speechMs: 25_000, csrcSeen: false, transitions: 2 });
    expect(h.hooks.csrc).toBe("missing");
    expect(h.state).toBe("ok");
  });

  it("compares reports", () => {
    const a = assess(0, { ...base, now: 1 });
    expect(sameReport(null, a)).toBe(false);
    expect(sameReport(a, { ...a, hooks: { ...a.hooks } })).toBe(true);
    expect(sameReport(a, { ...a, hooks: { ...a.hooks, tile: "missing" } })).toBe(false);
  });
});
