/* Every Zoom web client selector set shipped, newest first (#246). The
 * observer uses the first one whose fingerprint passes on the page,
 * re-checked every 30 s. Sets are bundled: no remote selector config (a
 * network call, and Sussurro is local-first). */
import type { SelectorSet } from "../../names/selectors";
import { ZOOM_2026_09A } from "./zoom-2026-09a";

export const ZOOM_SETS: readonly SelectorSet[] = [ZOOM_2026_09A];
