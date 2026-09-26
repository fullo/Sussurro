/* The name observer (#131, plan §4.3 layer 2; Teams #245, Zoom #246): runs
 * in the MAIN world of a meeting page — the frame that captures, so Zoom's
 * meeting iframe reads its own document — only while capturing, and
 * reports who is speaking and who is in the call. One observer for every
 * platform; what differs is its profile (profile.ts: selector sets, RTP
 * source, voting, checks).
 *
 * Every 100 ms it:
 * - polls the RTP sources of the remote audio receivers (sources.ts:
 *   CSRCs on Meet and Teams, SSRCs on Zoom): the audio-precise "who", sent
 *   as `speaker_active` / `speaker_idle` with `source: "rtp"` and a
 *   `csrc:<n>` / `ssrc:<n>` id;
 * - reads the page through the current selector set when the DOM changed
 *   (a MutationObserver on the attributes the sets use), at least once a
 *   second (dom.ts), re-picking the set every 30 s;
 * - votes source → name when one source speaks and one remote tile is lit
 *   (binder.ts; lag-aware on Teams and Zoom) and sends each locked binding
 *   as `speaker_name`;
 * - when 2 s of remote speech came without any source (a browser or
 *   transport that doesn't expose them, Zoom's WASM mode), sends the lit
 *   tiles themselves as the timeline (`source: "dom"`, a `tile:<hash>` id
 *   with the name), which the app compensates for lag;
 * - where the profile says so, learns the user's own tile from our mic
 *   (self.ts), and ignores the lit tile while the page shows something
 *   that makes it wrong (a `pause` hook: Zoom's screen share);
 * - sends the visible participants when they change, and the health
 *   (health.ts) when it changes: a broken hook stops sending names, never
 *   guesses.
 *
 * Times are the performance clock; the MAIN world turns them into page
 * frames. Nothing here throws into the page. Captions are not read (a
 * possible opt-in fallback later). */
import { binderSafeLit, type ObserverMsg } from "./messages";
import { NameBinder } from "./binder";
import { SourceTracker, toPerfTime, type SourceEntry } from "./sources";
import { pickSet, probe, watchedAttributes, type DomProbe } from "./dom";
import { assess, sameReport } from "./health";
import { nameKey } from "./guard";
import { SelfFromMic } from "./self";
import type { PlatformProfile } from "./profile";
import type { SelectorSet } from "./selectors";
import type { HealthReport } from "../../shared/speakerEvents";

type Entries = ArrayLike<{ source: number; timestamp: number; audioLevel?: number }>;

export interface ReceiverLike {
  getContributingSources?: () => Entries;
  getSynchronizationSources?: () => Entries;
}

export interface ObserverDeps {
  doc: Document;
  profile: PlatformProfile;
  /** Remote audio receivers of the page's peer connections. */
  receivers: () => ReceiverLike[];
  now: () => number;
  timeOrigin: number;
  /** Remote channel level (0..1), for the health check without sources. */
  remoteLevel: () => number;
  /** Our mic channel level (0..1), for the self check (self.ts). */
  micLevel?: () => number;
  emit: (m: ObserverMsg) => void;
  /** Selector sets in place of the profile's (tests). */
  sets?: readonly SelectorSet[];
  /** Report errors (kept by the MAIN world's diagnostics). */
  note?: (e: string) => void;
}

export const TICK_MS = 100;
const PROBE_EVERY_MS = 1_000;
const REPICK_EVERY_MS = 30_000;
const HEALTH_EVERY_MS = 1_000;
const PARTICIPANTS_EVERY_MS = 2_000;
/** Remote speech without any source before the lit tiles become the timeline. */
const DOM_TIMELINE_AFTER_MS = 2_000;
/** Channel level above which a channel counts as speech. */
const SPEECH_LEVEL = 0.01;

export class NameObserver {
  private d: ObserverDeps;
  private profile: PlatformProfile;
  private sets: readonly SelectorSet[];
  private set: SelectorSet;
  private sources: SourceTracker;
  private binder: NameBinder;
  private self: SelfFromMic | null;
  private probe: DomProbe | null = null;
  private lastProbe = -Infinity;
  private lastPick = -Infinity;
  private dirty = true;
  private wasSpeaking = new Map<string, boolean>();
  private transitions = 0;
  private speechMs = 0;
  private lastTick: number | null = null;
  private startAt = 0;
  private domOpen = new Map<string, string | null>();
  private sentNames = "";
  private lastParticipants = -Infinity;
  private health: HealthReport | null = null;
  private lastHealth = -Infinity;
  private timer: ReturnType<typeof setInterval> | undefined;
  private mo: MutationObserver | null = null;

  constructor(deps: ObserverDeps) {
    this.d = deps;
    this.profile = deps.profile;
    this.sets = deps.sets ?? deps.profile.sets;
    this.set = this.sets[0];
    this.sources = new SourceTracker(deps.profile.sources);
    this.binder = new NameBinder(deps.profile.binder);
    this.self = deps.profile.selfFromMic && deps.micLevel ? new SelfFromMic() : null;
  }

  private get label(): string {
    return `${this.profile.platform} observer`;
  }

  /** Start polling (the MAIN world, while armed). */
  start(): void {
    this.startAt = this.d.now();
    try {
      const MO = this.d.doc.defaultView?.MutationObserver;
      const target = this.d.doc.body ?? this.d.doc.documentElement;
      if (MO && target) {
        this.mo = new MO(() => this.pageChanged());
        this.mo.observe(target, { subtree: true, childList: true, attributes: true, attributeFilter: watchedAttributes(this.sets) });
      }
    } catch (e) {
      this.d.note?.(`${this.label}: ${String(e)}`);
    }
    this.timer = setInterval(() => this.tick(), TICK_MS);
  }

  /** Stop: every open speaker ends now. */
  stop(): void {
    clearInterval(this.timer);
    this.timer = undefined;
    this.mo?.disconnect();
    this.mo = null;
    const now = this.d.now();
    for (const c of this.sources.stop()) this.d.emit({ type: "speaker_idle", id: c.id, source: "rtp", at: c.at });
    this.closeDom(now);
  }

  /** The page changed: read it again at the next poll (the
   *  MutationObserver; tests, which have none). */
  pageChanged(): void {
    this.dirty = true;
  }

  /** One poll (public for tests, which drive the clock). */
  tick(): void {
    try {
      this.step();
    } catch (e) {
      this.d.note?.(`${this.label}: ${String(e)}`);
    }
  }

  private readEntries(now: number): SourceEntry[][] {
    const ssrc = this.sources.kind === "ssrc";
    const entries: SourceEntry[][] = [];
    for (const r of this.d.receivers()) {
      try {
        const list = (ssrc ? r.getSynchronizationSources?.() : r.getContributingSources?.()) ?? [];
        const out: SourceEntry[] = [];
        for (let i = 0; i < list.length; i++) {
          const e = list[i];
          out.push({ source: e.source, timestamp: toPerfTime(e.timestamp, now, this.d.timeOrigin), audioLevel: e.audioLevel });
        }
        entries.push(out);
      } catch {
        /* a closed receiver */
      }
    }
    return entries;
  }

  private step() {
    const now = this.d.now();
    if (this.lastTick === null) this.startAt = this.startAt || now;
    const dt = this.lastTick === null ? 0 : Math.min(1_000, Math.max(0, now - this.lastTick));
    this.lastTick = now;

    // ---- the audio ------------------------------------------------------
    const hadSources = this.sources.seen;
    for (const c of this.sources.update(now, this.readEntries(now))) {
      if (c.kind === "active") {
        const name = this.binder.nameOf(c.id);
        this.d.emit({ type: "speaker_active", id: c.id, source: "rtp", at: c.at, ...(name ? { name } : {}) });
      } else this.d.emit({ type: "speaker_idle", id: c.id, source: "rtp", at: c.at });
    }
    // Sources just showed up: the tile timeline is no longer needed.
    if (!hadSources && this.sources.seen) this.closeDom(now);
    const active = this.sources.activeIds();
    const remoteSpeaking = active.length > 0 || this.d.remoteLevel() > SPEECH_LEVEL;
    if (remoteSpeaking) this.speechMs += dt;

    // ---- the page -------------------------------------------------------
    if (now - this.lastPick >= REPICK_EVERY_MS) {
      const picked = pickSet(this.d.doc, this.sets);
      this.set = picked.set;
      this.lastPick = now;
      this.onProbe(now, picked.probe);
    } else if (this.dirty || now - this.lastProbe >= PROBE_EVERY_MS) {
      this.onProbe(now, probe(this.d.doc, this.set));
    }

    // ---- the user's own tile, from our mic --------------------------------
    const p = this.probe;
    if (this.self && p && !p.paused && this.speakingUsable()) {
      const lit = p.tiles.filter((t) => t.speaking && !t.self).map((t) => t.key);
      if (this.self.observe(now, (this.d.micLevel?.() ?? 0) > SPEECH_LEVEL, remoteSpeaking, lit)) this.probe = this.withSelf(p);
    }

    // ---- names ----------------------------------------------------------
    const q = this.probe;
    const remote = q ? q.tiles.filter((t) => !t.self && t.name) : [];
    const readable = !!q && !q.paused && this.speakingUsable();
    const bindings = this.binder.observe(
      now,
      active,
      readable ? binderSafeLit(remote) : [],
      remote.map((t) => t.name as string),
      q?.selfName ?? null,
    );
    for (const b of bindings) this.d.emit({ type: "speaker_name", id: b.id, name: b.name });

    // ---- health ---------------------------------------------------------
    if (now - this.lastHealth >= HEALTH_EVERY_MS) {
      this.lastHealth = now;
      const report = assess(this.startAt, {
        now,
        set: q ? this.set.id : null,
        tiles: q?.tiles.length ?? 0,
        named: q?.tiles.filter((t) => t.name).length ?? 0,
        rejected: q?.rejectedNames ?? 0,
        indicated: q?.indicated,
        minCoverage: this.profile.minCoverage,
        transitions: this.transitions,
        speechMs: this.speechMs,
        csrcSeen: this.sources.seen,
        sourceKind: this.sources.kind,
        bound: this.binder.bindings().length,
      });
      if (!sameReport(this.health, report)) {
        this.health = report;
        this.d.emit({ type: "observer_health", ...report });
      }
    }
  }

  /** The speaking marker is not known to be broken. */
  private speakingUsable(): boolean {
    return this.health?.hooks.speaking !== "broken";
  }

  /** The probe with the tile learned from the mic marked as the user's. */
  private withSelf(p: DomProbe): DomProbe {
    const key = this.self?.key;
    if (!key) return p;
    const tiles = p.tiles.map((t) => (t.key === key ? { ...t, self: true } : t));
    const mine = tiles.find((t) => t.key === key);
    return { ...p, tiles, selfName: p.selfName ?? mine?.name ?? null };
  }

  private onProbe(now: number, raw: DomProbe) {
    const p = this.withSelf(raw);
    this.probe = p;
    this.lastProbe = now;
    this.dirty = false;
    const remote = p.tiles.filter((t) => !t.self);
    for (const t of remote) {
      const was = this.wasSpeaking.get(t.key) ?? false;
      if (t.speaking && !was) this.transitions++;
      this.wasSpeaking.set(t.key, t.speaking);
    }
    // Tile timeline, only once the audio proved to have no sources; none
    // while the page says its lit tile is wrong.
    if (!this.sources.seen && this.speechMs >= DOM_TIMELINE_AFTER_MS && this.speakingUsable()) {
      const lit = new Map(p.paused ? [] : remote.filter((t) => t.speaking).map((t) => [t.key, t.name] as const));
      for (const [key] of [...this.domOpen]) {
        if (!lit.has(key)) {
          this.domOpen.delete(key);
          this.d.emit({ type: "speaker_idle", id: key, source: "dom", at: now });
        }
      }
      for (const [key, name] of lit) {
        if (this.domOpen.has(key)) continue;
        this.domOpen.set(key, name);
        this.d.emit({ type: "speaker_active", id: key, source: "dom", at: now, ...(name ? { name } : {}) });
      }
    }
    // Visible participants, when they change.
    const seen = new Set<string>();
    const names: string[] = [];
    for (const t of remote) {
      if (!t.name) continue;
      const k = nameKey(t.name);
      if (seen.has(k)) continue;
      seen.add(k);
      names.push(t.name);
    }
    const sig = names.map(nameKey).sort().join("\n");
    if (names.length && sig !== this.sentNames && now - this.lastParticipants >= PARTICIPANTS_EVERY_MS) {
      this.sentNames = sig;
      this.lastParticipants = now;
      this.d.emit({ type: "participants", names });
    }
  }

  private closeDom(now: number) {
    for (const key of this.domOpen.keys()) this.d.emit({ type: "speaker_idle", id: key, source: "dom", at: now });
    this.domOpen.clear();
  }
}
