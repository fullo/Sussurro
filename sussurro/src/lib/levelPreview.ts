/* Live level preview for a picked audio device (#314), pure helpers.

   The React side (components/LevelMeter.tsx) polls `level_preview` and feeds
   each reading through `nextSilenceState`; this file holds the timing logic
   alone so it can be unit-tested without a fake backend or timers. */

/** How long a device must read silence before the UI hints that nothing is
 *  routed to it. */
export const SILENCE_HINT_MS = 3000;

/** A reading at or below this is "silence" — matches `levelToPercent`'s
 *  floor (0 dBFS never quite reaches exact 0 from a real device, but the
 *  backend reports a hard 0.0 once the rolling window is all zeros). */
export function isSilent(level: number): boolean {
  return !(level > 0);
}

/** Tracks how long the level has read silence, for the "no sound" hint.
 *  `since` is the timestamp (ms) silence started, or `null` while there is
 *  signal; pass the previous value in and the new one comes out — a small
 *  state machine, easy to unit test tick by tick. */
export function nextSilenceSince(level: number, since: number | null, now: number): number | null {
  if (!isSilent(level)) return null;
  return since ?? now;
}

/** Whether the "no sound on this device" hint should show right now. */
export function shouldShowSilenceHint(since: number | null, now: number): boolean {
  return since !== null && now - since >= SILENCE_HINT_MS;
}

export type LevelPreviewKind = "mic" | "system";

/** The hint text for a device that has read silence for a while — system
 *  audio suggests playing something, since a silent computer is otherwise
 *  expected (a mic instead suggests checking routing). */
export function silenceHintText(kind: LevelPreviewKind): string {
  return kind === "system"
    ? "No sound on this device — check that audio is routed to it, or play something to test."
    : "No sound on this device — check that audio is routed to it.";
}
