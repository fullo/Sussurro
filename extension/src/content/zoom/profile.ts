/* The Zoom web client's name-observer profile (#246, desk study #239).
 *
 * Zoom runs the meeting in a same-origin `/wc/` iframe (the observer runs
 * in that frame, the one that captures, #287) and has two transports:
 * - **WebRTC mode**: each remote participant arrives as their own RTP
 *   stream, so the stream's SSRC is the participant (no CSRCs, no mirror);
 *   the page only supplies the name.
 * - **WASM mode** (the older transport, and Zoom's automatic fallback):
 *   no RTP receivers at all. Nothing is ever seen, and after 2 s of remote
 *   speech the lit tiles become the timeline (`source: "dom"`), which the
 *   app shifts by its lag — lower quality, and the health report says
 *   `ssrc: missing`.
 * The active-speaker marker lags and flickers (a 500 ms hold) and sticks to
 * the presenter during a screen share (the set's `pause` hook), so votes
 * are lag-aware and need two turns (or one long one); the user's own tile
 * is learned from our mic when its marker is missing. Every number is the
 * desk study's first guess, to be measured on real calls (#184). */
import type { PlatformProfile } from "../names/profile";
import { ZOOM_SETS } from "./selectors";

export const ZOOM_PROFILE: PlatformProfile = {
  platform: "zoom",
  sets: ZOOM_SETS,
  sources: { kind: "ssrc", hangMs: 600, mirrorPolls: 0, minLevel: 0.01 },
  binder: { voteDelayMs: 1_000, litHoldMs: 500, minEpisodes: 2, longEpisodeMs: 3_000 },
  // Zoom may mark only the active tile: no coverage check (the 20 s
  // speech-without-glow check still applies).
  minCoverage: 0,
  selfFromMic: true,
};
