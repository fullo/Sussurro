import { describe, expect, it } from "vitest";
import { initialSession, isCapturing, shouldTabCapture, step, TAB_CAPTURE_AFTER_MS, type Session, type SessionEvent } from "./session";

const OK = { type: "check", result: { ok: true, app: "0.9.0" } } as const;
const run = (events: SessionEvent[], from: Session = initialSession()) => {
  let s = from;
  const effects: string[][] = [];
  for (const e of events) {
    const r = step(s, e, () => 0.5);
    s = r.s;
    effects.push(r.effects.map((x) => (x.type === "schedule-retry" ? `retry:${x.ms}` : x.type)));
  }
  return { s, effects };
};

describe("capture session", () => {
  it("goes start → check → arm → connect → live → stop → done", () => {
    const { s, effects } = run([
      { type: "start" },
      OK,
      { type: "armed", rate: 48000 },
      { type: "ws-open" },
      { type: "ws-status", state: "started", itemId: "2026-09-24-sync" },
      { type: "stop" },
      { type: "ws-status", state: "done", itemId: "2026-09-24-weekly-sync" },
    ]);
    expect(effects).toEqual([["check"], ["arm"], ["connect"], ["send-start"], [], ["disarm", "send-stop"], ["close"]]);
    expect(s).toMatchObject({ phase: "done", rate: 48000, itemId: "2026-09-24-weekly-sync" });
  });

  it("never captures before an explicit start", () => {
    for (const e of [OK, { type: "armed", rate: 1 }, { type: "ws-open" }, { type: "retry" }] as SessionEvent[]) {
      const { s, effects } = step(initialSession(), e);
      expect(isCapturing(s)).toBe(false);
      expect(effects.map((x) => x.type)).not.toContain("arm");
      expect(effects.map((x) => x.type)).not.toContain("connect");
    }
  });

  it("stops on a failed check, telling why", () => {
    const { s, effects } = run([{ type: "start" }, { type: "check", result: { ok: false, reason: "not_paired" } }]);
    expect(s).toMatchObject({ phase: "error", problem: "not_paired" });
    expect(effects[1]).toEqual([]);
  });

  it("reconnects with backoff while the app is away, then resumes with a new start", () => {
    const live = run([{ type: "start" }, OK, { type: "armed", rate: 48000 }, { type: "ws-open" }]).s;
    const { s, effects } = run(
      [
        { type: "ws-closed" },
        { type: "retry" },
        { type: "check", result: { ok: false, reason: "not_running" } },
        { type: "retry" },
        OK,
        { type: "ws-open" },
      ],
      live,
    );
    expect(effects).toEqual([["retry:375"], ["check"], ["retry:750"], ["check"], ["connect"], ["send-start"]]);
    expect(s).toMatchObject({ phase: "live", attempt: 0 });
    expect(isCapturing(s)).toBe(true);
  });

  it("gives up on a fatal problem during a reconnect and disarms", () => {
    const { s, effects } = run([
      { type: "start" },
      OK,
      { type: "armed", rate: 48000 },
      { type: "ws-closed" }, // the first connection failed
      { type: "retry" },
      { type: "check", result: { ok: false, reason: "bad_token" } },
    ]);
    expect(effects.at(-1)).toEqual(["disarm"]);
    expect(s).toMatchObject({ phase: "error", problem: "bad_token" });
  });

  it("an app error ends the capture", () => {
    const { s, effects } = run([
      { type: "start" },
      OK,
      { type: "armed", rate: 48000 },
      { type: "ws-open" },
      { type: "ws-status", state: "error", message: "a meeting is already being recorded" },
    ]);
    expect(effects.at(-1)).toEqual(["disarm", "close"]);
    expect(s).toMatchObject({ phase: "error", message: "a meeting is already being recorded" });
  });

  it("keeps warnings without changing phase", () => {
    const live = run([{ type: "start" }, OK, { type: "armed", rate: 48000 }, { type: "ws-open" }]).s;
    const { s } = run([{ type: "ws-status", state: "warning", message: "3 mic audio frame(s) lost" }], live);
    expect(s).toMatchObject({ phase: "live", message: "3 mic audio frame(s) lost" });
  });

  it("tears down on stop or page loss in every phase", () => {
    const at = (events: SessionEvent[]) => run(events).s;
    const phases: [string, Session][] = [
      ["checking", at([{ type: "start" }])],
      ["arming", at([{ type: "start" }, OK])],
      ["connecting", at([{ type: "start" }, OK, { type: "armed", rate: 1 }])],
      ["reconnecting", at([{ type: "start" }, OK, { type: "armed", rate: 1 }, { type: "ws-closed" }])],
    ];
    for (const [phase, s] of phases) {
      expect(s.phase).toBe(phase);
      expect(step(s, { type: "stop" }).s.phase).toBe("idle");
      expect(step(s, { type: "page-gone" }).s.phase).toBe("idle");
    }
    expect(step(phases[3][1], { type: "stop" }).effects.map((e) => e.type)).toEqual(["disarm", "close", "cancel-retry"]);
    // Live: the app gets `stop` so it finishes the item.
    const live = at([{ type: "start" }, OK, { type: "armed", rate: 1 }, { type: "ws-open" }]);
    expect(step(live, { type: "page-gone" })).toMatchObject({ s: { phase: "stopping" }, effects: [{ type: "send-stop" }] });
    // The socket closing after stop means the app is done with it.
    expect(step(step(live, { type: "stop" }).s, { type: "ws-closed" }).s.phase).toBe("done");
  });

  it("disarms a page that arms after the user already stopped", () => {
    const stopped = run([{ type: "start" }, OK, { type: "stop" }]).s;
    expect(step(stopped, { type: "armed", rate: 48000 }).effects).toEqual([{ type: "disarm" }]);
    expect(step(stopped, { type: "ws-open" }).effects).toEqual([{ type: "close" }]);
  });

  it("can start again after done or error", () => {
    const done = run([{ type: "start" }, OK, { type: "armed", rate: 1 }, { type: "ws-open" }, { type: "stop" }, { type: "ws-closed" }]).s;
    expect(step(done, { type: "start" }).s).toEqual({ phase: "checking", attempt: 0 });
  });
});

describe("shouldTabCapture", () => {
  const base = { browser: "chrome" as const, capturing: true, remoteVia: "none", armedForMs: TAB_CAPTURE_AFTER_MS, alreadyTried: false };
  it("fires once, on Chrome, when no remote audio shows up in time", () => {
    expect(shouldTabCapture(base)).toBe(true);
    expect(shouldTabCapture({ ...base, browser: "firefox" })).toBe(false);
    expect(shouldTabCapture({ ...base, alreadyTried: true })).toBe(false);
    expect(shouldTabCapture({ ...base, armedForMs: 1000 })).toBe(false);
    expect(shouldTabCapture({ ...base, remoteVia: "peer-connection" })).toBe(false);
    expect(shouldTabCapture({ ...base, remoteVia: undefined })).toBe(false);
    expect(shouldTabCapture({ ...base, capturing: false })).toBe(false);
  });
});
