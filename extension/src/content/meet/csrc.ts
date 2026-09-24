/* Who is speaking, from the audio itself (#131, desk study #105): Meet
 * mixes the loudest speakers into a few remote audio streams and tags each
 * RTP packet with the contributing source (CSRC) of the participant in it.
 * A CSRC stays the same for a participant until they leave, and arrives
 * with the audio, so it has no indicator lag. It carries no name: names
 * come from the page (binder.ts).
 *
 * The observer polls `RTCRtpReceiver.getContributingSources()` on every
 * remote audio receiver about 10 times a second and feeds the entries
 * here. A CSRC is active while its entries stay fresh (a packet within
 * `hangMs`, which also bridges the gaps between words) and, when the
 * browser reports it, loud enough. A CSRC present on two receivers or more
 * for several polls in a row is a mirror (the current speaker echoed on
 * another stream, not a person) and is ignored for the rest of the call.
 * Pure — unit tested. */

export interface SourceEntry {
  source: number;
  /** When a packet of this source was last delivered (performance clock). */
  timestamp: number;
  /** 0..1 linear, when the stream carries audio levels. */
  audioLevel?: number;
}

export interface CsrcChange {
  kind: "active" | "idle";
  /** `csrc:<n>` */
  id: string;
  /** Performance time the change happened (first / last packet). */
  at: number;
}

export interface CsrcOptions {
  hangMs: number;
  /** Polls in a row on ≥ 2 receivers before a CSRC counts as a mirror. */
  mirrorPolls: number;
  /** Below this level a packet does not count as speech (when reported). */
  minLevel: number;
}

export const CSRC_DEFAULTS: CsrcOptions = { hangMs: 400, mirrorPolls: 3, minLevel: 0.001 };

/** A browser that stamps CSRC entries on the epoch (older engines) instead
 *  of the performance clock: bring them onto the performance clock. */
export function toPerfTime(ts: number, now: number, timeOrigin: number): number {
  return ts > now + 1e9 ? ts - timeOrigin : ts;
}

export const csrcId = (source: number) => `csrc:${source >>> 0}`;

export class CsrcTracker {
  private opts: CsrcOptions;
  private active = new Map<number, { first: number; last: number }>();
  private onTwo = new Map<number, number>();
  private mirrors = new Set<number>();
  /** Some receiver reported a CSRC at all (the transport exposes them). */
  seen = false;

  constructor(opts: Partial<CsrcOptions> = {}) {
    this.opts = { ...CSRC_DEFAULTS, ...opts };
  }

  /** CSRCs speaking now (mirrors excluded). */
  activeIds(): string[] {
    return [...this.active.keys()].map(csrcId);
  }

  isMirror(source: number): boolean {
    return this.mirrors.has(source);
  }

  /** One poll: the entries of every remote audio receiver at `now`. */
  update(now: number, receivers: SourceEntry[][]): CsrcChange[] {
    const out: CsrcChange[] = [];
    const fresh = new Map<number, { receivers: number; first: number; last: number }>();
    for (const entries of receivers) {
      const here = new Set<number>();
      for (const e of entries) {
        if (typeof e?.source !== "number" || !Number.isFinite(e.timestamp)) continue;
        this.seen = true;
        if (here.has(e.source)) continue;
        here.add(e.source);
        if (now - e.timestamp > this.opts.hangMs) continue;
        if (typeof e.audioLevel === "number" && e.audioLevel < this.opts.minLevel) continue;
        const f = fresh.get(e.source);
        if (f) {
          f.receivers++;
          f.last = Math.max(f.last, e.timestamp);
        } else fresh.set(e.source, { receivers: 1, first: e.timestamp, last: e.timestamp });
      }
    }
    for (const [source, f] of fresh) {
      if (f.receivers >= 2) {
        const n = (this.onTwo.get(source) ?? 0) + 1;
        this.onTwo.set(source, n);
        if (n >= this.opts.mirrorPolls) this.mirrors.add(source);
      } else this.onTwo.delete(source);
    }
    // Ended: no longer fresh, or found to be a mirror.
    for (const [source, a] of [...this.active]) {
      if (!fresh.has(source) || this.mirrors.has(source)) {
        this.active.delete(source);
        out.push({ kind: "idle", id: csrcId(source), at: a.last });
      }
    }
    for (const [source, f] of fresh) {
      if (this.mirrors.has(source)) continue;
      const a = this.active.get(source);
      if (a) a.last = Math.max(a.last, f.last);
      else {
        this.active.set(source, { first: f.first, last: f.last });
        out.push({ kind: "active", id: csrcId(source), at: f.first });
      }
    }
    return out;
  }

  /** Capture stopped: every active CSRC ends now. */
  stop(): CsrcChange[] {
    const out = [...this.active].map(([source, a]) => ({ kind: "idle" as const, id: csrcId(source), at: a.last }));
    this.active.clear();
    return out;
  }
}
