/* RTP source → display name, by voting (#131, desk studies #105/#239). A
 * source (CSRC or SSRC) says who is speaking but not their name; the
 * page's lit tile says a name but lags and breaks often. When exactly one
 * source is speaking and exactly one remote tile is lit, that is one vote
 * for (source, name). A binding locks after `lockVotes` agreeing votes with
 * a clear margin over the runner-up; once locked, the source names every
 * line of that participant without the page (a later page change costs
 * nothing for them). Rules that keep a wrong name out, since a wrong name
 * is worse than none:
 *
 * - one name per source, and one live source per name (a name already
 *   bound to a source heard in the last `rejoinMs` is not bound again;
 *   after that, a rejoin with a new source may take it);
 * - a name shown on two tiles (namesakes) is never bound;
 * - the user's own tile and name never vote;
 * - a locked binding contradicted by `unlockVotes` votes in a row is
 *   dropped (the app then forgets the name for that source).
 *
 * **Lag-aware voting** (`voteDelayMs > 0`; Teams and Zoom, #239). Their
 * speaking outline trails the audio by about a second, so an instant vote
 * pairs a new speaker with the previous speaker's outline — in a two-person
 * call that swaps every name. A vote then counts only in the *interior* of
 * a turn: the source has been the only one speaking for `voteDelayMs`, the
 * one lit tile has been the only one lit for `litHoldMs` (flicker), and it
 * lit up after the turn began (not the previous speaker's trailing
 * outline). Votes also count *episodes* (turns), not only polls: a binding
 * needs `minEpisodes` turns, or one turn voted over `longEpisodeMs` —
 * a single burst of noise that lights a tile can't bind. Meet keeps the
 * instant votes (`voteDelayMs: 0`, `minEpisodes: 1`).
 *
 * Pure — unit tested. */
import { nameKey } from "./guard";

export interface BindOptions {
  lockVotes: number;
  /** The winner needs this many more votes than the runner-up. */
  margin: number;
  unlockVotes: number;
  rejoinMs: number;
  /** A vote needs the source alone this long (0: at once). */
  voteDelayMs: number;
  /** …and the lit tile alone this long. */
  litHoldMs: number;
  /** Turns a binding needs (1: any)… */
  minEpisodes: number;
  /** …unless one turn was voted this long. */
  longEpisodeMs: number;
}

export const BIND_DEFAULTS: BindOptions = {
  lockVotes: 5,
  margin: 3,
  unlockVotes: 10,
  rejoinMs: 30_000,
  voteDelayMs: 0,
  litHoldMs: 0,
  minEpisodes: 1,
  longEpisodeMs: 3_000,
};

export interface Binding {
  id: string;
  /** null: a binding was dropped. */
  name: string | null;
}

export interface LitTile {
  name: string;
}

interface Tally {
  name: string;
  /** Votes (polls). */
  n: number;
  /** Turns with at least one vote. */
  episodes: number;
  lastEpisode: number;
  /** First vote of the current turn, and the longest voted turn. */
  episodeStart: number;
  longest: number;
}

export class NameBinder {
  private opts: BindOptions;
  private votes = new Map<string, Map<string, Tally>>();
  private bound = new Map<string, { key: string; name: string; against: number }>();
  private lastHeard = new Map<string, number>();
  /** The source speaking alone, since when, and its turn number. */
  private solo: { id: string; since: number; episode: number } | null = null;
  private episodes = 0;
  /** The name lit alone, and since when. */
  private lit: { key: string; since: number } | null = null;

  constructor(opts: Partial<BindOptions> = {}) {
    this.opts = { ...BIND_DEFAULTS, ...opts };
  }

  nameOf(id: string): string | null {
    return this.bound.get(id)?.name ?? null;
  }

  /** Every locked binding. */
  bindings(): Binding[] {
    return [...this.bound].map(([id, b]) => ({ id, name: b.name }));
  }

  /** One observation at `now`: the sources speaking, the remote tiles lit
   *  (names already guarded, self excluded), every name on the page (for
   *  namesakes) and the user's own name. Call it on every poll, with no
   *  lit tiles when the page can't be read, so the turns stay current.
   *  Returns bindings that changed. */
  observe(now: number, active: string[], lit: LitTile[], roster: string[], selfName: string | null): Binding[] {
    for (const id of active) this.lastHeard.set(id, now);
    if (active.length === 1) {
      if (this.solo?.id !== active[0]) this.solo = { id: active[0], since: now, episode: ++this.episodes };
    } else this.solo = null;
    if (lit.length === 1) {
      const k = nameKey(lit[0].name);
      if (this.lit?.key !== k) this.lit = { key: k, since: now };
    } else this.lit = null;

    if (!this.solo || !this.lit || lit.length !== 1) return [];
    const { id, since, episode } = this.solo;
    const o = this.opts;
    if (o.voteDelayMs > 0) {
      if (now - since < o.voteDelayMs) return [];
      if (now - this.lit.since < o.litHoldMs) return [];
      // Lit before the turn began: the previous turn's trailing outline.
      if (this.lit.since <= since) return [];
    }
    const name = lit[0].name;
    const key = nameKey(name);
    if (selfName && nameKey(selfName) === key) return [];
    if (roster.filter((n) => nameKey(n) === key).length > 1) return [];

    const out: Binding[] = [];
    const b = this.bound.get(id);
    if (b) {
      if (b.key === key) {
        b.against = 0;
        return out;
      }
      b.against++;
      if (b.against < o.unlockVotes) return out;
      this.bound.delete(id);
      this.votes.delete(id);
      out.push({ id, name: null });
    }
    const tally = this.votes.get(id) ?? new Map<string, Tally>();
    this.votes.set(id, tally);
    const v = tally.get(key) ?? { name, n: 0, episodes: 0, lastEpisode: -1, episodeStart: now, longest: 0 };
    v.n++;
    if (v.lastEpisode !== episode) {
      v.episodes++;
      v.lastEpisode = episode;
      v.episodeStart = now;
    }
    v.longest = Math.max(v.longest, now - v.episodeStart);
    tally.set(key, v);
    const ranked = [...tally.values()].sort((a, b) => b.n - a.n);
    const best = ranked[0];
    const second = ranked[1]?.n ?? 0;
    if (best.n < o.lockVotes || best.n - second < o.margin) return out;
    if (best.episodes < o.minEpisodes && best.longest < o.longEpisodeMs) return out;
    const bestKey = nameKey(best.name);
    // One live source per name.
    for (const [other, ob] of this.bound) {
      if (other !== id && ob.key === bestKey && now - (this.lastHeard.get(other) ?? -Infinity) < o.rejoinMs) return out;
    }
    this.bound.set(id, { key: bestKey, name: best.name, against: 0 });
    out.push({ id, name: best.name });
    return out;
  }
}
