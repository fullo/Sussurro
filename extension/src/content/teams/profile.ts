/* Microsoft Teams web's name-observer profile (#245, desk study #239).
 *
 * Teams sends one server-mixed remote audio track whose RTP packets carry
 * per-participant CSRCs (several at once = overlap, which the app leaves
 * unnamed), plus a redundant track carrying the same CSRCs — so Meet's
 * mirror rule is off here. Its CSRC entries go stale for about 550 ms in
 * natural pauses: an 800 ms hang keeps a turn whole. The speaking outline
 * trails the audio by about 1 s and also lights on typing and noise, so
 * votes are lag-aware and need two turns (or one long one), and the user's
 * own tile is learned from our mic when its marker is missing. One UI
 * variant has no outline at all: the coverage check turns that into
 * `names_unavailable`. Every number is the desk study's first guess, to be
 * measured on real calls (#184). */
import type { PlatformProfile } from "../names/profile";
import { TEAMS_SETS } from "./selectors";

export const TEAMS_PROFILE: PlatformProfile = {
  platform: "teams",
  sets: TEAMS_SETS,
  sources: { kind: "csrc", hangMs: 800, mirrorPolls: 0, minLevel: 0.001 },
  binder: { voteDelayMs: 1_000, litHoldMs: 300, minEpisodes: 2, longEpisodeMs: 3_000 },
  minCoverage: 0.5,
  selfFromMic: true,
};
