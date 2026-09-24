/* The Audio tab's transport (#142): drives one <audio> element per saved
   file through a playlist (lib/replay.ts). Framework-free and written
   against a tiny media interface, so the tests run it on fake elements.

   - The span being played ("entry") names its files; only those play, the
     others are paused. The "master" is the longest-running of them and
     gives the clock; the others are nudged back in sync when they drift
     more than SYNC_TOLERANCE_S (two channel files of one call).
   - tick() is called on a short timer while playing: at the end
     of a span it seeks to the next one (skipping everything between), at
     the end of the playlist it stops. */

import { fromVirtual, locate, toVirtual, type Playlist } from "./replay";

/** What the player needs of an HTMLMediaElement. */
export interface MediaLike {
  currentTime: number;
  readonly duration: number;
  readonly paused: boolean;
  readonly ended: boolean;
  playbackRate: number;
  play(): Promise<void> | void;
  pause(): void;
}

const SYNC_TOLERANCE_S = 0.15;
/** A span counts as over this close to its end. */
const END_EPSILON_MS = 20;

export class ReplayPlayer {
  private pl: Playlist;
  private index = 0;
  /** Real time while paused (the elements may not have loaded yet). */
  private pos = 0;
  playing = false;
  rate = 1;

  constructor(
    private readonly media: Map<string, MediaLike>,
    playlist: Playlist,
    private readonly onError: (e: unknown) => void = () => {},
  ) {
    this.pl = playlist;
    const first = playlist.entries[0];
    if (first) this.pos = first.start_ms;
  }

  get playlist(): Playlist {
    return this.pl;
  }

  /** Index of the span being played. */
  get entryIndex(): number {
    return this.index;
  }

  private active(): MediaLike[] {
    const e = this.pl.entries[this.index];
    if (!e) return [];
    return e.files.map((f) => this.media.get(f)).filter((m): m is MediaLike => !!m);
  }

  /** The clock: of the active elements, the one with the longest file. */
  private master(): MediaLike | undefined {
    let best: MediaLike | undefined;
    for (const m of this.active()) {
      const d = Number.isFinite(m.duration) ? m.duration : 0;
      const bd = best && Number.isFinite(best.duration) ? best.duration : -1;
      if (!best || d > bd) best = m;
    }
    return best;
  }

  /** Current real time (ms). */
  now(): number {
    if (!this.playing) return this.pos;
    const m = this.master();
    return m ? m.currentTime * 1000 : this.pos;
  }

  /** Current position on the playlist's timeline (ms). */
  virtualNow(): number {
    return toVirtual(this.pl, this.now());
  }

  private start(m: MediaLike) {
    m.playbackRate = this.rate;
    try {
      const p = m.play();
      if (p && typeof p.catch === "function") p.catch((e) => this.onError(e));
    } catch (e) {
      this.onError(e);
    }
  }

  /** Put the elements of span `index` at real time `ms`; pause the rest. */
  private cue(index: number, ms: number) {
    this.index = index;
    this.pos = ms;
    const active = new Set(this.active());
    for (const m of this.media.values()) {
      if (!active.has(m)) {
        if (!m.paused) m.pause();
        continue;
      }
      m.currentTime = ms / 1000;
      if (this.playing) this.start(m);
    }
  }

  private atEnd(): boolean {
    const last = this.pl.entries[this.pl.entries.length - 1];
    return !last || (this.index >= this.pl.entries.length - 1 && this.pos >= last.end_ms - END_EPSILON_MS);
  }

  play() {
    if (this.pl.entries.length === 0 || this.playing) return;
    if (this.atEnd()) {
      this.index = 0;
      this.pos = this.pl.entries[0].start_ms;
    }
    this.playing = true;
    this.cue(this.index, this.pos);
  }

  pause() {
    if (!this.playing) return;
    this.pos = this.now();
    this.playing = false;
    for (const m of this.media.values()) if (!m.paused) m.pause();
  }

  toggle() {
    if (this.playing) this.pause();
    else this.play();
  }

  /** Go to real time `ms`: inside a span, or the next span when `ms` falls
   *  in a skipped gap; past the last span = the end. */
  seek(ms: number) {
    const at = locate(this.pl, ms);
    if (!at) {
      const n = this.pl.entries.length;
      if (n === 0) return;
      this.pause();
      this.index = n - 1;
      this.pos = this.pl.entries[n - 1].end_ms;
      return;
    }
    this.cue(at.index, at.ms);
  }

  /** Go to a position on the playlist's timeline. */
  seekVirtual(v: number) {
    const at = fromVirtual(this.pl, v);
    if (!at) return;
    if (at.index === this.pl.entries.length - 1 && at.ms >= this.pl.entries[at.index].end_ms) {
      this.seek(at.ms);
      return;
    }
    this.cue(at.index, at.ms);
  }

  /** ±ms on the playlist's timeline (so a skip never lands in a gap). */
  skip(deltaMs: number) {
    this.seekVirtual(this.virtualNow() + deltaMs);
  }

  setRate(rate: number) {
    this.rate = rate;
    for (const m of this.media.values()) m.playbackRate = rate;
  }

  /** Another playlist (the speaker filter changed): keep the position,
   *  moving to the next span of the new playlist when it's in a gap. */
  setPlaylist(pl: Playlist) {
    const t = this.now();
    const wasPlaying = this.playing;
    this.pause();
    this.pl = pl;
    this.index = 0;
    this.pos = pl.entries[0]?.start_ms ?? 0;
    if (pl.entries.length === 0) return;
    const at = locate(pl, t) ?? { index: 0, ms: pl.entries[0].start_ms };
    this.playing = wasPlaying;
    this.cue(at.index, at.ms);
  }

  /** On each clock tick while playing: advance past span ends, keep channels
   *  in sync. Returns whether it is still playing. */
  tick(): boolean {
    if (!this.playing) return false;
    const e = this.pl.entries[this.index];
    const m = this.master();
    if (!e || !m) {
      this.pause();
      return false;
    }
    const t = m.currentTime * 1000;
    this.pos = t;
    if (t >= e.end_ms - END_EPSILON_MS || m.ended) {
      const next = this.index + 1;
      if (next >= this.pl.entries.length) {
        this.pause();
        this.pos = e.end_ms;
        return false;
      }
      this.cue(next, this.pl.entries[next].start_ms);
      return true;
    }
    for (const o of this.active()) {
      if (o === m) continue;
      const d = Number.isFinite(o.duration) ? o.duration : Infinity;
      if (m.currentTime >= d) continue; // this channel's file is over
      if (Math.abs(o.currentTime - m.currentTime) > SYNC_TOLERANCE_S) o.currentTime = m.currentTime;
      if (o.paused) this.start(o);
    }
    return true;
  }
}
