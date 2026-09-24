/* Is the Meet observer still reading the page right? (#131) Health is a
 * cross-check, not "a selector found something": the audio says when
 * someone remote is speaking (a CSRC, else the remote level), so a
 * speaking marker that never lights while the audio shows 20 s of speech
 * is broken even if the selector still matches elements. A broken hook
 * stops sending (no guessing) and the state becomes `names_unavailable`:
 * the app keeps "Voice N" for speakers it cannot name, and the side panel
 * can say so. Pure — unit tested. */
import type { HealthReport, HookState } from "../../shared/speakers";

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
  /** Speaking transitions seen on remote tiles since the start. */
  transitions: number;
  /** Remote speech heard since the start, ms (CSRC active or remote level). */
  speechMs: number;
  /** Any CSRC was ever reported. */
  csrcSeen: boolean;
  /** CSRCs with a locked name. */
  bound: number;
}

/** Before this, a hook that found nothing is "unknown", not "missing". */
export const GRACE_MS = 10_000;
/** Remote speech without a single lit tile after which `speaking` is broken. */
export const SPEECH_WITHOUT_GLOW_MS = 20_000;

export function assess(start: number, i: HealthInput): HealthReport {
  const settled = i.now - start >= GRACE_MS;
  const tile: HookState = i.tiles > 0 ? "ok" : settled ? "missing" : "unknown";
  const tileName: HookState = i.tiles === 0 ? "unknown" : i.named > 0 ? "ok" : settled || i.rejected > 0 ? "broken" : "unknown";
  const speaking: HookState = i.transitions > 0 ? "ok" : i.speechMs >= SPEECH_WITHOUT_GLOW_MS ? "broken" : "unknown";
  const csrc: HookState = i.csrcSeen ? "ok" : i.speechMs >= SPEECH_WITHOUT_GLOW_MS ? "missing" : "unknown";
  const hooks: Record<string, HookState> = { tile, tileName, speaking, csrc };
  // Names can still come when: tiles and names read, and either the glow
  // works or some CSRC is already bound (it keeps its name without it).
  const bad = tile === "missing" || tileName === "broken" || (speaking === "broken" && i.bound === 0);
  return { state: bad ? "names_unavailable" : "ok", set: i.set, hooks };
}

/** Same report (for sending only changes). */
export function sameReport(a: HealthReport | null, b: HealthReport): boolean {
  if (!a || a.state !== b.state || a.set !== b.set) return false;
  const ka = Object.keys(a.hooks);
  return ka.length === Object.keys(b.hooks).length && ka.every((k) => a.hooks[k] === b.hooks[k]);
}
