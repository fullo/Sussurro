/* Every Teams web selector set shipped, newest first (#245). The observer
 * uses the first one whose fingerprint passes on the page, re-checked every
 * 30 s — Teams serves more than one UI variant in the same tenant (#239),
 * so one set per variant can be live at once. Sets are bundled: no remote
 * selector config (a network call, and Sussurro is local-first). */
import type { SelectorSet } from "../../names/selectors";
import { TEAMS_2026_09A } from "./teams-2026-09a";

export const TEAMS_SETS: readonly SelectorSet[] = [TEAMS_2026_09A];
