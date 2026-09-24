/* Which audio tracks feed which channel. Pure — unit tested with fake
 * tracks; the MAIN-world hook feeds it and applies its answer to the audio
 * graph.
 *
 * Sources, in the order the policy trusts them (spike #104, plan E4):
 *
 * - **mic** (channel 0, the user):
 *   1. tracks the page *sends* on a peer connection — `addTrack`,
 *      `addTransceiver(track)`, `addStream`, `RTCRtpSender.replaceTrack`, and
 *      a `getSenders()` rescan. A replaced or removed sender drops its old
 *      track, so a device switch or a pre-join preview never doubles the
 *      mic;
 *   2. otherwise the page's own `getUserMedia` audio tracks (prompt-free:
 *      covers pages that don't send over `RTCPeerConnection`, e.g. Zoom);
 *   3. otherwise, only in the element fallback, the hook's own
 *      `getUserMedia` (may show a second permission prompt).
 * - **remote** (channel 1, everyone else):
 *   1. tracks received on any peer connection (`track` events and a
 *      `getReceivers()` rescan), several connections and tracks added or
 *      removed mid-call included — all mixed into the one channel;
 *   2. only when the page never created a peer connection: audio of the
 *      playing media elements (`srcObject` tracks, else `captureStream()`),
 *      minus anything known to be local;
 *   3. (Chrome) tab capture, decided by the background, not here.
 *
 * Ended tracks and closed connections drop out. */

export interface TrackLike {
  readonly id: string;
  readonly kind: string;
  readonly readyState: string;
}

export type MicVia = "sender" | "page-getUserMedia" | "own-getUserMedia" | "none";
export type RemoteVia = "peer-connection" | "media-element" | "none";

export interface Selection<T extends TrackLike> {
  mic: T[];
  remote: T[];
  micVia: MicVia;
  remoteVia: RemoteVia;
  /** The element fallback is active and no mic was found: the hook may ask
   *  for its own `getUserMedia`. */
  wantOwnMic: boolean;
}

const live = (t: TrackLike) => t.kind === "audio" && t.readyState !== "ended";

export class TrackRegistry<T extends TrackLike, PC = object, Sender = object> {
  /** sender → the track it currently sends, with its connection. */
  private senders = new Map<Sender, { pc: PC; track: T }>();
  /** remote tracks per connection. */
  private received = new Map<PC, Set<T>>();
  private pcs = new Set<PC>();
  private gum = new Set<T>();
  private own = new Set<T>();
  private elements = new Set<T>();
  /** Ever saw a peer connection (even a closed one). */
  pcSeen = false;

  addPc(pc: PC): void {
    this.pcs.add(pc);
    this.pcSeen = true;
  }

  /** The connection closed: its senders and remote tracks go. */
  closePc(pc: PC): void {
    this.pcs.delete(pc);
    this.received.delete(pc);
    for (const [s, v] of this.senders) if (v.pc === pc) this.senders.delete(s);
  }

  /** `sender` now sends `track` (null/undefined: nothing, or not audio). */
  setSender(pc: PC, sender: Sender, track: T | null | undefined): void {
    this.addPc(pc);
    if (track && track.kind === "audio") this.senders.set(sender, { pc, track });
    else this.senders.delete(sender);
  }

  removeSender(sender: Sender): void {
    this.senders.delete(sender);
  }

  addRemote(pc: PC, track: T): void {
    if (track.kind !== "audio") return;
    this.addPc(pc);
    let set = this.received.get(pc);
    if (!set) this.received.set(pc, (set = new Set()));
    set.add(track);
  }

  /** An audio track the page got from `getUserMedia`. */
  addPageGum(track: T): void {
    if (track.kind === "audio") this.gum.add(track);
  }

  /** The hook's own `getUserMedia` track (element fallback only). */
  addOwnGum(track: T): void {
    if (track.kind === "audio") this.own.add(track);
  }

  /** The current audio tracks of the playing media elements. */
  setElementTracks(tracks: Iterable<T>): void {
    this.elements = new Set([...tracks].filter((t) => t.kind === "audio"));
  }

  /** Forget ended tracks (they would only add silence). */
  prune(): void {
    for (const [s, v] of this.senders) if (!live(v.track)) this.senders.delete(s);
    for (const set of this.received.values()) for (const t of set) if (!live(t)) set.delete(t);
    for (const s of [this.gum, this.own]) for (const t of s) if (!live(t)) s.delete(t);
  }

  /** Every track known to be the user's (never counted as remote). */
  private localIds(): Set<string> {
    const ids = new Set<string>();
    for (const v of this.senders.values()) ids.add(v.track.id);
    for (const t of this.gum) ids.add(t.id);
    for (const t of this.own) ids.add(t.id);
    return ids;
  }

  select(): Selection<T> {
    const uniq = (ts: Iterable<T>) => {
      const m = new Map<string, T>();
      for (const t of ts) if (live(t) && !m.has(t.id)) m.set(t.id, t);
      return [...m.values()];
    };

    let remote: T[] = [];
    let remoteVia: RemoteVia = "none";
    const fromPc = uniq([...this.received.values()].flatMap((s) => [...s]));
    if (fromPc.length) {
      remote = fromPc;
      remoteVia = "peer-connection";
    } else if (!this.pcSeen) {
      const local = this.localIds();
      const fromEl = uniq(this.elements).filter((t) => !local.has(t.id));
      if (fromEl.length) {
        remote = fromEl;
        remoteVia = "media-element";
      }
    }

    let mic: T[] = [];
    let micVia: MicVia = "none";
    const sent = uniq([...this.senders.values()].map((v) => v.track));
    const pageGum = uniq(this.gum);
    const own = uniq(this.own);
    if (sent.length) {
      mic = sent;
      micVia = "sender";
    } else if (pageGum.length) {
      mic = pageGum;
      micVia = "page-getUserMedia";
    } else if (own.length && remoteVia === "media-element") {
      mic = own;
      micVia = "own-getUserMedia";
    }
    const wantOwnMic = remoteVia === "media-element" && micVia === "none";
    return { mic, remote, micVia, remoteVia, wantOwnMic };
  }
}

/** Tracks to connect and disconnect to move a channel from `before` to
 *  `after` (by track id). */
export function diffTracks<T extends TrackLike>(before: Iterable<T>, after: Iterable<T>): { add: T[]; remove: T[] } {
  const a = new Map([...before].map((t) => [t.id, t] as const));
  const b = new Map([...after].map((t) => [t.id, t] as const));
  return {
    add: [...b.values()].filter((t) => !a.has(t.id)),
    remove: [...a.values()].filter((t) => !b.has(t.id)),
  };
}
