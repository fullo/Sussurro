/* Reconnect delays for the `/live` WebSocket: exponential with "equal
 * jitter" (half fixed, half random), capped. Pure — the random source is
 * injectable so the tests are deterministic. */

export interface BackoffOptions {
  /** First delay, ms. */
  baseMs: number;
  /** Largest delay, ms. */
  maxMs: number;
  factor: number;
}

export const RECONNECT: BackoffOptions = { baseMs: 500, maxMs: 15_000, factor: 2 };

/** Delay before reconnect attempt `attempt` (1 = the first retry). */
export function backoffDelay(attempt: number, opts: BackoffOptions = RECONNECT, random: () => number = Math.random): number {
  const n = Math.max(1, Math.floor(attempt));
  const ceiling = Math.min(opts.maxMs, opts.baseMs * opts.factor ** (n - 1));
  const r = Math.min(1, Math.max(0, random()));
  return Math.round(ceiling / 2 + (ceiling / 2) * r);
}
