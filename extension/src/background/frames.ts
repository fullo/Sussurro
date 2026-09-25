/* Which frame of a tab to capture (#287). Pure — unit tested.
 *
 * On Zoom's web client the call runs in a same-origin iframe, so the
 * content scripts run in every matching frame of the tab and each frame
 * opens its own port to the background. Exactly one of them is armed (the
 * "capture frame"); the audio of any other frame is dropped, so a page that
 * also shows hook activity in its top frame (a mic preview, say) is never
 * captured twice. Meet and Teams run the scripts in the top frame only:
 * one frame, picked at once when it shows a call.
 *
 * The pick uses the frames' snapshots (what the MAIN-world hook observed),
 * in tiers: a peer connection (the call itself), else some audio (a mic or
 * a received track), else nothing. A frame with a peer connection is
 * picked at once; otherwise the frames get FRAME_SETTLE_MS to report and
 * the best one is taken. Later, only a frame of a higher tier replaces the
 * capture frame (Start pressed before joining, or a frame that reported
 * late): the frame given up never had the call, so no call audio is lost
 * or doubled. Tiers only rise (the peer-connection count is "since load"),
 * so the pick can't flip back and forth. */
import type { CaptureSnapshot } from "../shared/messages";

/** How long after Start to wait for frames to report when none shows a
 *  peer connection yet, before arming the best of what is there. */
export const FRAME_SETTLE_MS = 500;

/** How much a frame looks like the call: 0 = no sign at all. The snapshot
 *  comes from the page (#217): anything malformed counts as no sign. */
export function callScore(s: CaptureSnapshot | null | undefined): number {
  if (!s || typeof s !== "object") return 0;
  const { pcCount, mic, remote } = s as Partial<CaptureSnapshot>;
  let n = 0;
  if (typeof pcCount === "number" && pcCount > 0) n += 4;
  if (remote?.via === "peer-connection" || (typeof remote?.tracks === "number" && remote.tracks > 0)) n += 2;
  if ((typeof mic?.via === "string" && mic.via !== "none") || (typeof mic?.tracks === "number" && mic.tracks > 0)) n += 1;
  return n;
}

/** 2 = a peer connection, 1 = some audio, 0 = nothing. */
export function callTier(s: CaptureSnapshot | null | undefined): 0 | 1 | 2 {
  const n = callScore(s);
  return n >= 4 ? 2 : n > 0 ? 1 : 0;
}

export interface FrameView {
  frameId: number;
  /** The frame's last snapshot (null: not reported yet). */
  capture: CaptureSnapshot | null;
}

export type FramePick =
  /** Arm this frame (it may already be the capture frame). */
  | { frame: number }
  /** No frame shows a call yet: ask again after `ms`. */
  | { wait: number }
  /** No frame to capture. */
  | { none: true };

/**
 * The frame to capture now. `current` is the capture frame (null before
 * one was picked), `sinceStartMs` the time since Start.
 */
export function pickCaptureFrame(frames: readonly FrameView[], current: number | null, sinceStartMs: number): FramePick {
  if (!frames.length) return { none: true };
  const score = new Map(frames.map((f) => [f.frameId, callScore(f.capture)]));
  const tier = new Map(frames.map((f) => [f.frameId, callTier(f.capture)]));
  // The best frame: highest score, then the top frame, then the oldest id.
  const best = [...frames].sort((a, b) => score.get(b.frameId)! - score.get(a.frameId)! || a.frameId - b.frameId)[0].frameId;
  if (current !== null && tier.has(current)) {
    return tier.get(best)! > tier.get(current)! ? { frame: best } : { frame: current };
  }
  if (tier.get(best) === 2 || sinceStartMs >= FRAME_SETTLE_MS) return { frame: best };
  return { wait: FRAME_SETTLE_MS - Math.max(0, sinceStartMs) };
}
