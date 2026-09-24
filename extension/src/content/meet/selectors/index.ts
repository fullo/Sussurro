/* Every Meet selector set shipped, newest first (#131). The observer uses
 * the first one whose fingerprint passes on the page. Sets are bundled: no
 * remote selector config (a network call, and Sussurro is local-first). */
import { MEET_2026_09A } from "./meet-2026-09a";
import type { SelectorSet } from "./types";

export const MEET_SETS: readonly SelectorSet[] = [MEET_2026_09A];
