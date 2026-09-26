/* Google Meet's name-observer profile (#131, desk study #105): a few
 * server-mixed remote streams whose packets carry the speaker's CSRC, with
 * a mirror of the current speaker on another stream; the speaking marker
 * follows the audio closely, so votes are instant. */
import type { PlatformProfile } from "../names/profile";
import { MEET_SETS } from "./selectors";

export const MEET_PROFILE: PlatformProfile = {
  platform: "meet",
  sets: MEET_SETS,
  sources: { kind: "csrc", hangMs: 400, mirrorPolls: 3, minLevel: 0.001 },
  binder: { voteDelayMs: 0, minEpisodes: 1 },
  minCoverage: 0,
  selfFromMic: false,
};
