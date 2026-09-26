/* Is the name observer still reading the page right? (#131; Teams/Zoom
 * #245/#246) Health is a cross-check, not "a selector found something":
 * the audio says when someone remote is speaking (an RTP source, else the
 * remote level), so a speaking marker that never lights while the audio
 * shows 20 s of speech is broken even if the selector still matches
 * elements. Where a platform serves UI variants without the speaking
 * indicator (Teams, #239), a coverage check catches it sooner: tiles are
 * found but too few carry the indicator at all. A broken hook stops
 * sending (no guessing) and the state becomes `names_unavailable`: the app
 * keeps "Voice N" for speakers it cannot name, and the side panel can say
 * so. Pure — unit tested. */
import type { HealthReport, HookState } from "../../shared/speakerEvents";
import type { SourceKind } from "./sources";

export interface HealthInput {
  now: number;
  /** The set in use (null: none). */
  set: string | null;
  /** Tiles found with a participant id. */
  tiles: number;
  /** Tiles whose name passed the guard. */
  named: number;
  /** Tiles with a name the guard rejected. */
  rejected: number;
  /** Tiles carrying a speaking indicator, lit or not (`undefined`: not
   *  checked). */
  indicated?: number;
  /** Least share of tiles with an indicator (0 or absent: no check). */
  minCoverage?: number;
  /** Speaking transitions seen on remote tiles since the start. */
  transitions: number;
  /** Remote speech heard since the start, ms (a source active or remote level). */
  speechMs: number;
  /** Any RTP source was ever reported. */
  csrcSeen: boolean;
  /** The hook name of the RTP sources in the report (default `csrc`). */
  sourceKind?: SourceKind;
  /** Sources with a locked name. */
  bound: number;
}

/** Before this, a hook that found nothing is "unknown", not "missing". */
export const GRACE_MS = 10_000;
/** Remote speech without a single lit tile after which `speaking` is broken. */
export const SPEECH_WITHOUT_GLOW_MS = 20_000;
/** Tiles needed before coverage means anything. */
export const COVERAGE_MIN_TILES = 2;

export function assess(start: number, i: HealthInput): HealthReport {
  const settled = i.now - start >= GRACE_MS;
  const tile: HookState = i.tiles > 0 ? "ok" : settled ? "missing" : "unknown";
  const tileName: HookState = i.tiles === 0 ? "unknown" : i.named > 0 ? "ok" : settled || i.rejected > 0 ? "broken" : "unknown";
  const uncovered =
    !!i.minCoverage && i.indicated !== undefined && settled && i.tiles >= COVERAGE_MIN_TILES && i.indicated < i.tiles * i.minCoverage;
  const speaking: HookState = uncovered ? "broken" : i.transitions > 0 ? "ok" : i.speechMs >= SPEECH_WITHOUT_GLOW_MS ? "broken" : "unknown";
  const rtp: HookState = i.csrcSeen ? "ok" : i.speechMs >= SPEECH_WITHOUT_GLOW_MS ? "missing" : "unknown";
  const hooks: Record<string, HookState> = { tile, tileName, speaking, [i.sourceKind ?? "csrc"]: rtp };
  // Names can still come when: tiles and names read, and either the glow
  // works or some source is already bound (it keeps its name without it).
  const bad = tile === "missing" || tileName === "broken" || (speaking === "broken" && i.bound === 0);
  return { state: bad ? "names_unavailable" : "ok", set: i.set, hooks };
}

/** Same report (for sending only changes). */
export function sameReport(a: HealthReport | null, b: HealthReport): boolean {
  if (!a || a.state !== b.state || a.set !== b.set) return false;
  const ka = Object.keys(a.hooks);
  return ka.length === Object.keys(b.hooks).length && ka.every((k) => a.hooks[k] === b.hooks[k]);
}
