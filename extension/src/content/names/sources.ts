/* Who is speaking, from the audio itself (#131, desk studies #105/#239).
 * It carries no name: names come from the page (binder.ts).
 *
 * - `csrc` (Meet, Teams): the server mixes the speakers into a few remote
 *   audio streams and tags each RTP packet with the contributing source
 *   (CSRC) of the participant in it. The observer polls
 *   `RTCRtpReceiver.getContributingSources()`.
 * - `ssrc` (Zoom's WebRTC mode): each remote participant arrives as their
 *   own RTP stream, so the stream's synchronization source (SSRC) is the
 *   participant. The observer polls `getSynchronizationSources()` (same
 *   entry shape). Zoom's WASM mode has no receivers at all: nothing is
 *   ever `seen`, and the observer falls back to the page's timeline.
 *
 * Either way the id stays the same for a participant until they leave (a
 * rejoin may get a new one) and arrives with the audio, so it has no
 * indicator lag. The observer feeds every remote audio receiver's entries
 * here about 10 times a second. A source is active while its entries stay
 * fresh (a packet within `hangMs`, which also bridges the gaps between
 * words — Teams needs longer, #239: its CSRC entries go stale for about
 * 550 ms in natural pauses) and, when the browser reports it, loud enough.
 *
 * Mirror rule (Meet only, `mirrorPolls > 0`): a CSRC present on two
 * receivers or more for several polls in a row is the current speaker
 * echoed on another stream, not a person, and is ignored for the rest of
 * the call. Teams sends a redundant second track carrying the same CSRCs
 * as its mix, so there the rule would blacklist every real speaker: it is
 * off (`mirrorPolls: 0`). Pure — unit tested. */

export interface SourceEntry {
  source: number;
  /** When a packet of this source was last delivered (performance clock). */
  timestamp: number;
  /** 0..1 linear, when the stream carries audio levels. */
  audioLevel?: number;
}

export type SourceKind = "csrc" | "ssrc";

export interface SourceChange {
  kind: "active" | "idle";
  /** `csrc:<n>` or `ssrc:<n>` */
  id: string;
  /** Performance time the change happened (first / last packet). */
  at: number;
}

export interface SourceOptions {
  /** Which RTP source the entries are (the id prefix). */
  kind: SourceKind;
  hangMs: number;
  /** Polls in a row on ≥ 2 receivers before a source counts as a mirror;
   *  0 turns the mirror rule off. */
  mirrorPolls: number;
  /** Below this level a packet does not count as speech (when reported). */
  minLevel: number;
}

export const SOURCE_DEFAULTS: SourceOptions = { kind: "csrc", hangMs: 400, mirrorPolls: 3, minLevel: 0.001 };

/** A browser that stamps entries on the epoch (older engines) instead of
 *  the performance clock: bring them onto the performance clock. */
export function toPerfTime(ts: number, now: number, timeOrigin: number): number {
  return ts > now + 1e9 ? ts - timeOrigin : ts;
}

export const sourceId = (kind: SourceKind, source: number) => `${kind}:${source >>> 0}`;
export const csrcId = (source: number) => sourceId("csrc", source);

export class SourceTracker {
  private opts: SourceOptions;
  private active = new Map<number, { first: number; last: number }>();
  private onTwo = new Map<number, number>();
  private mirrors = new Set<number>();
  /** Some receiver reported a source at all (the transport exposes them). */
  seen = false;

  constructor(opts: Partial<SourceOptions> = {}) {
    this.opts = { ...SOURCE_DEFAULTS, ...opts };
  }

  get kind(): SourceKind {
    return this.opts.kind;
  }

  private id(source: number): string {
    return sourceId(this.opts.kind, source);
  }

  /** Sources speaking now (mirrors excluded). */
  activeIds(): string[] {
    return [...this.active.keys()].map((s) => this.id(s));
  }

  isMirror(source: number): boolean {
    return this.mirrors.has(source);
  }

  /** One poll: the entries of every remote audio receiver at `now`. */
  update(now: number, receivers: SourceEntry[][]): SourceChange[] {
    const out: SourceChange[] = [];
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
    if (this.opts.mirrorPolls > 0) {
      for (const [source, f] of fresh) {
        if (f.receivers >= 2) {
          const n = (this.onTwo.get(source) ?? 0) + 1;
          this.onTwo.set(source, n);
          if (n >= this.opts.mirrorPolls) this.mirrors.add(source);
        } else this.onTwo.delete(source);
      }
    }
    // Ended: no longer fresh, or found to be a mirror.
    for (const [source, a] of [...this.active]) {
      if (!fresh.has(source) || this.mirrors.has(source)) {
        this.active.delete(source);
        out.push({ kind: "idle", id: this.id(source), at: a.last });
      }
    }
    for (const [source, f] of fresh) {
      if (this.mirrors.has(source)) continue;
      const a = this.active.get(source);
      if (a) a.last = Math.max(a.last, f.last);
      else {
        this.active.set(source, { first: f.first, last: f.last });
        out.push({ kind: "active", id: this.id(source), at: f.first });
      }
    }
    return out;
  }

  /** Capture stopped: every active source ends now. */
  stop(): SourceChange[] {
    const out = [...this.active].map(([source, a]) => ({ kind: "idle" as const, id: this.id(source), at: a.last }));
    this.active.clear();
    return out;
  }
}
