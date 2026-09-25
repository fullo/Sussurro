/* What differs between the meeting platforms for the name observer (#131;
 * Teams #245, Zoom #246; desk study #239): the selector sets, which RTP
 * source says who speaks and how, how the binder votes, and the extra
 * health and self checks. The observer itself is one (observer.ts). Every
 * number here that is not Meet's is a first guess from the desk study, to
 * be measured on real calls (#184). Data only. */
import type { Platform } from "../../shared/platform";
import type { BindOptions } from "./binder";
import type { SelectorSet } from "./selectors";
import type { SourceOptions } from "./sources";

export interface PlatformProfile {
  platform: Platform;
  /** Selector sets, newest first. */
  sets: readonly SelectorSet[];
  /** The RTP source tracker (`kind` picks CSRCs or SSRCs). */
  sources: Partial<SourceOptions> & Pick<SourceOptions, "kind">;
  binder: Partial<BindOptions>;
  /** Least share of tiles carrying a speaking indicator (0: no check). */
  minCoverage: number;
  /** Learn the user's own tile from the mic (self.ts). */
  selfFromMic: boolean;
}
