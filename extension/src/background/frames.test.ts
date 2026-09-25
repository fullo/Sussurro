import { describe, expect, it } from "vitest";
import { FRAME_SETTLE_MS, callScore, pickCaptureFrame, type FrameView } from "./frames";
import type { CaptureSnapshot } from "../shared/messages";

function snap(o: { pcs?: number; remote?: "peer-connection" | "none"; mic?: "sender" | "page-getUserMedia" | "none" } = {}): CaptureSnapshot {
  return {
    armed: false,
    ctxState: null,
    processor: null,
    pcCount: o.pcs ?? 0,
    mic: { via: o.mic ?? "none", tracks: 0, level: 0 },
    remote: { via: o.remote ?? "none", tracks: 0, level: 0 },
    remoteExternal: false,
    errors: [],
  };
}

const call = () => snap({ pcs: 1, remote: "peer-connection", mic: "sender" });
const preview = () => snap({ mic: "page-getUserMedia" });
const blank = () => snap();
const f = (frameId: number, capture: CaptureSnapshot | null): FrameView => ({ frameId, capture });

describe("callScore", () => {
  it("ranks peer connections over received audio over a mic, and nothing as 0", () => {
    expect(callScore(null)).toBe(0);
    expect(callScore(blank())).toBe(0);
    expect(callScore(preview())).toBe(1);
    expect(callScore(snap({ pcs: 1 }))).toBe(4);
    expect(callScore(call())).toBe(7);
    expect(callScore(snap({ pcs: 1 }))).toBeGreaterThan(callScore(preview()));
  });

  it("treats a malformed snapshot from the page as no sign", () => {
    expect(callScore({} as CaptureSnapshot)).toBe(0);
    expect(callScore({ pcCount: "9", mic: null, remote: 3 } as unknown as CaptureSnapshot)).toBe(0);
    expect(callScore("x" as unknown as CaptureSnapshot)).toBe(0);
  });

  it("counts armed tracks too", () => {
    const s = blank();
    s.remote.tracks = 1;
    expect(callScore(s)).toBe(2);
  });
});

describe("pickCaptureFrame", () => {
  it("no frames: nothing to pick", () => {
    expect(pickCaptureFrame([], null, 10_000)).toEqual({ none: true });
  });

  it("a single top frame showing a call is picked at once (Meet, Teams)", () => {
    expect(pickCaptureFrame([f(0, call())], null, 0)).toEqual({ frame: 0 });
  });

  it("Zoom: the meeting iframe wins over a top frame with only a mic preview", () => {
    expect(pickCaptureFrame([f(0, preview()), f(7, call())], null, 0)).toEqual({ frame: 7 });
    expect(pickCaptureFrame([f(7, call()), f(0, blank()), f(9, null)], null, 0)).toEqual({ frame: 7 });
  });

  it("waits for other frames while none shows a call, then takes the top frame", () => {
    expect(pickCaptureFrame([f(0, blank())], null, 100)).toEqual({ wait: FRAME_SETTLE_MS - 100 });
    expect(pickCaptureFrame([f(3, null), f(0, null)], null, FRAME_SETTLE_MS)).toEqual({ frame: 0 });
    expect(pickCaptureFrame([f(5, blank()), f(3, blank())], null, FRAME_SETTLE_MS + 1)).toEqual({ frame: 3 });
  });

  it("equal scores: the top frame, then the lowest id", () => {
    expect(pickCaptureFrame([f(4, call()), f(0, call())], null, 0)).toEqual({ frame: 0 });
    expect(pickCaptureFrame([f(9, call()), f(4, call())], null, 0)).toEqual({ frame: 4 });
  });

  it("keeps a capture frame that shows a call, even if another frame looks busier", () => {
    expect(pickCaptureFrame([f(0, preview()), f(7, call())], 0, 60_000)).toEqual({ frame: 0 });
    expect(pickCaptureFrame([f(0, snap({ pcs: 1 })), f(7, call())], 0, 60_000)).toEqual({ frame: 0 });
  });

  it("moves off a capture frame that shows nothing when another frame shows the call", () => {
    // Start pressed before joining: the top frame was armed, then the
    // meeting iframe connected its call.
    expect(pickCaptureFrame([f(0, blank()), f(7, call())], 0, 60_000)).toEqual({ frame: 7 });
  });

  it("keeps a silent capture frame while no other frame does better", () => {
    expect(pickCaptureFrame([f(0, blank()), f(7, blank())], 0, 60_000)).toEqual({ frame: 0 });
    expect(pickCaptureFrame([f(7, blank()), f(0, null)], 7, 60_000)).toEqual({ frame: 7 });
  });

  it("a capture frame that went away is replaced by the best remaining one", () => {
    expect(pickCaptureFrame([f(0, preview())], 7, 60_000)).toEqual({ frame: 0 });
  });
});
