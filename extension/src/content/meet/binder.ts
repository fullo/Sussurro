/* CSRC → display name, by voting (#131, desk study #105). A CSRC says who
 * is speaking but not their name; the page's lit tile says a name but lags
 * and breaks often. When exactly one CSRC is speaking and exactly one
 * remote tile is lit, that is one vote for (CSRC, name). A binding locks
 * after `lockVotes` agreeing votes with a clear margin over the runner-up;
 * once locked, the CSRC names every line of that participant without the
 * page (a later page change costs nothing for them). Rules that keep a
 * wrong name out, since a wrong name is worse than none:
 *
 * - one name per CSRC, and one live CSRC per name (a name already bound
 *   to a CSRC heard in the last `rejoinMs` is not bound again; after that,
 *   a rejoin with a new CSRC may take it);
 * - a name shown on two tiles (namesakes) is never bound;
 * - the user's own tile and name never vote;
 * - a locked binding contradicted by `unlockVotes` votes in a row is
 *   dropped (the app then forgets the name for that CSRC).
 *
 * Pure — unit tested. */
import { nameKey } from "./names";

export interface BindOptions {
  lockVotes: number;
  /** The winner needs this many more votes than the runner-up. */
  margin: number;
  unlockVotes: number;
  rejoinMs: number;
}

export const BIND_DEFAULTS: BindOptions = { lockVotes: 5, margin: 3, unlockVotes: 10, rejoinMs: 30_000 };

export interface Binding {
  id: string;
  /** null: a binding was dropped. */
  name: string | null;
}

export interface LitTile {
  name: string;
}

export class NameBinder {
  private opts: BindOptions;
  private votes = new Map<string, Map<string, { name: string; n: number }>>();
  private bound = new Map<string, { key: string; name: string; against: number }>();
  private lastHeard = new Map<string, number>();

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

  /** One observation at `now`: the CSRCs speaking, the remote tiles lit
   *  (names already guarded, self excluded), every name on the page (for
   *  namesakes) and the user's own name. Returns bindings that changed. */
  observe(now: number, active: string[], lit: LitTile[], roster: string[], selfName: string | null): Binding[] {
    for (const id of active) this.lastHeard.set(id, now);
    if (active.length !== 1 || lit.length !== 1) return [];
    const id = active[0];
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
      if (b.against < this.opts.unlockVotes) return out;
      this.bound.delete(id);
      this.votes.delete(id);
      out.push({ id, name: null });
    }
    const tally = this.votes.get(id) ?? new Map<string, { name: string; n: number }>();
    this.votes.set(id, tally);
    const v = tally.get(key) ?? { name, n: 0 };
    v.n++;
    tally.set(key, v);
    const ranked = [...tally.values()].sort((a, b) => b.n - a.n);
    const best = ranked[0];
    const second = ranked[1]?.n ?? 0;
    if (best.n < this.opts.lockVotes || best.n - second < this.opts.margin) return out;
    const bestKey = nameKey(best.name);
    // One live CSRC per name.
    for (const [other, ob] of this.bound) {
      if (other !== id && ob.key === bestKey && now - (this.lastHeard.get(other) ?? -Infinity) < this.opts.rejoinMs) return out;
    }
    this.bound.set(id, { key: bestKey, name: best.name, against: 0 });
    out.push({ id, name: best.name });
    return out;
  }
}
