/* The user's own tile, learned from our own microphone (#239; Teams and
 * Zoom, whose self marker may be missing from a UI variant). We capture
 * the mic channel, which a bot never has: when the user has been speaking
 * for a while, nobody remote has, and exactly one tile lights up, that tile
 * is the user's. After `votes` such polls for one tile, with no other tile
 * ever voted for as much, it is the user's for the rest of the call
 * (sticky): never a remote speaker, never bindable, not a participant.
 * The waits cover the outline's lag (it trails the audio by about 1 s).
 * Pure — unit tested. */

export interface SelfOptions {
  /** The mic speaking, and the remote side quiet, for this long first. */
  holdMs: number;
  /** Polls agreeing on one tile. */
  votes: number;
}

export const SELF_DEFAULTS: SelfOptions = { holdMs: 1_000, votes: 10 };

export class SelfFromMic {
  private opts: SelfOptions;
  private micSince: number | null = null;
  private quietSince: number | null = null;
  private tally = new Map<string, number>();
  private decided: string | null = null;

  constructor(opts: Partial<SelfOptions> = {}) {
    this.opts = { ...SELF_DEFAULTS, ...opts };
  }

  /** The tile key found to be the user's (null: not yet). */
  get key(): string | null {
    return this.decided;
  }

  /** One poll: whether our mic carries speech, whether the remote side
   *  does, and the keys of the lit tiles that are not already known to be
   *  the user's. Returns the key when it was just decided. */
  observe(now: number, micSpeaking: boolean, remoteSpeaking: boolean, lit: string[]): string | null {
    this.micSince = micSpeaking ? (this.micSince ?? now) : null;
    this.quietSince = remoteSpeaking ? null : (this.quietSince ?? now);
    if (this.decided || this.micSince === null || this.quietSince === null) return null;
    if (now - this.micSince < this.opts.holdMs || now - this.quietSince < this.opts.holdMs) return null;
    if (lit.length !== 1) return null;
    const n = (this.tally.get(lit[0]) ?? 0) + 1;
    this.tally.set(lit[0], n);
    const others = Math.max(0, ...[...this.tally].filter(([k]) => k !== lit[0]).map(([, v]) => v));
    if (n < this.opts.votes || others * 2 >= n) return null;
    this.decided = lit[0];
    return this.decided;
  }
}
