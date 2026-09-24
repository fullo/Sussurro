/* The Meet name observer (#131, plan §4.3 layer 2): runs in the MAIN world
 * of a Meet page, only while capturing, and reports who is speaking and
 * who is in the call. Meet only — Teams and Zoom web get the channel rule
 * and voice clustering only in 0.9.
 *
 * Every 100 ms it:
 * - polls the CSRCs of the remote audio receivers (csrc.ts): the audio-
 *   precise "who", sent as `speaker_active` / `speaker_idle` with
 *   `source: "rtp"` and a `csrc:<n>` id;
 * - reads the page through the current selector set when the DOM changed
 *   (a MutationObserver on the attributes the sets use), at least once a
 *   second (dom.ts), re-picking the set every 30 s;
 * - votes CSRC → name when one CSRC speaks and one remote tile is lit
 *   (binder.ts) and sends each locked binding as `speaker_name`;
 * - when 2 s of remote speech came without any CSRC (a browser or
 *   transport that doesn't expose them), sends the lit tiles themselves as
 *   the timeline (`source: "dom"`, a
 *   `tile:<hash>` id with the name), which the app compensates for lag;
 * - sends the visible participants when they change, and the health
 *   (health.ts) when it changes: a broken hook stops sending names, never
 *   guesses.
 *
 * Times are the performance clock; the MAIN world turns them into page
 * frames. Nothing here throws into the page. Captions are not read (a
 * possible opt-in fallback later). */
import { binderSafeLit, type ObserverMsg } from "./messages";
import { NameBinder } from "./binder";
import { CsrcTracker, toPerfTime, type SourceEntry } from "./csrc";
import { pickSet, probe, watchedAttributes, type DomProbe } from "./dom";
import { assess, sameReport } from "./health";
import { nameKey } from "./names";
import { MEET_SETS } from "./selectors";
import type { SelectorSet } from "./selectors/types";
import type { HealthReport } from "../../shared/speakerEvents";

export interface ReceiverLike {
  getContributingSources?: () => ArrayLike<{ source: number; timestamp: number; audioLevel?: number }>;
}

export interface ObserverDeps {
  doc: Document;
  /** Remote audio receivers of the page's peer connections. */
  receivers: () => ReceiverLike[];
  now: () => number;
  timeOrigin: number;
  /** Remote channel level (0..1), for the health check without CSRCs. */
  remoteLevel: () => number;
  emit: (m: ObserverMsg) => void;
  sets?: readonly SelectorSet[];
  /** Report errors (kept by the MAIN world's diagnostics). */
  note?: (e: string) => void;
}

export const TICK_MS = 100;
const PROBE_EVERY_MS = 1_000;
const REPICK_EVERY_MS = 30_000;
const HEALTH_EVERY_MS = 1_000;
const PARTICIPANTS_EVERY_MS = 2_000;
/** Remote speech without any CSRC before the lit tiles become the timeline. */
const DOM_TIMELINE_AFTER_MS = 2_000;
/** Remote level above which the remote channel counts as speech. */
const SPEECH_LEVEL = 0.01;

export class MeetObserver {
  private d: ObserverDeps;
  private sets: readonly SelectorSet[];
  private set: SelectorSet;
  private csrc = new CsrcTracker();
  private binder = new NameBinder();
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
    this.sets = deps.sets ?? MEET_SETS;
    this.set = this.sets[0];
  }

  /** Start polling (the MAIN world, while armed). */
  start(): void {
    this.startAt = this.d.now();
    try {
      const MO = this.d.doc.defaultView?.MutationObserver;
      const target = this.d.doc.body ?? this.d.doc.documentElement;
      if (MO && target) {
        this.mo = new MO(() => {
          this.dirty = true;
        });
        this.mo.observe(target, { subtree: true, childList: true, attributes: true, attributeFilter: watchedAttributes(this.sets) });
      }
    } catch (e) {
      this.d.note?.(`meet observer: ${String(e)}`);
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
    for (const c of this.csrc.stop()) this.d.emit({ type: "speaker_idle", id: c.id, source: "rtp", at: c.at });
    this.closeDom(now);
  }

  /** One poll (public for tests, which drive the clock). */
  tick(): void {
    try {
      this.step();
    } catch (e) {
      this.d.note?.(`meet observer: ${String(e)}`);
    }
  }

  private step() {
    const now = this.d.now();
    if (this.lastTick === null) this.startAt = this.startAt || now;
    const dt = this.lastTick === null ? 0 : Math.min(1_000, Math.max(0, now - this.lastTick));
    this.lastTick = now;

    // ---- the audio ------------------------------------------------------
    const entries: SourceEntry[][] = [];
    for (const r of this.d.receivers()) {
      try {
        const list = r.getContributingSources?.() ?? [];
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
    const hadCsrc = this.csrc.seen;
    for (const c of this.csrc.update(now, entries)) {
      if (c.kind === "active") {
        const name = this.binder.nameOf(c.id);
        this.d.emit({ type: "speaker_active", id: c.id, source: "rtp", at: c.at, ...(name ? { name } : {}) });
      } else this.d.emit({ type: "speaker_idle", id: c.id, source: "rtp", at: c.at });
    }
    // CSRCs just showed up: the tile timeline is no longer needed.
    if (!hadCsrc && this.csrc.seen) this.closeDom(now);
    const active = this.csrc.activeIds();
    if (active.length || this.d.remoteLevel() > SPEECH_LEVEL) this.speechMs += dt;

    // ---- the page -------------------------------------------------------
    if (now - this.lastPick >= REPICK_EVERY_MS) {
      const picked = pickSet(this.d.doc, this.sets);
      this.set = picked.set;
      this.lastPick = now;
      this.onProbe(now, picked.probe);
    } else if (this.dirty || now - this.lastProbe >= PROBE_EVERY_MS) {
      this.onProbe(now, probe(this.d.doc, this.set));
    }

    // ---- names ----------------------------------------------------------
    const p = this.probe;
    if (p && this.speakingUsable()) {
      const remote = p.tiles.filter((t) => !t.self && t.name);
      const roster = remote.map((t) => t.name as string);
      for (const b of this.binder.observe(now, active, binderSafeLit(remote), roster, p.selfName)) {
        this.d.emit({ type: "speaker_name", id: b.id, name: b.name });
      }
    }

    // ---- health ---------------------------------------------------------
    if (now - this.lastHealth >= HEALTH_EVERY_MS) {
      this.lastHealth = now;
      const report = assess(this.startAt, {
        now,
        set: p ? this.set.id : null,
        tiles: p?.tiles.length ?? 0,
        named: p?.tiles.filter((t) => t.name).length ?? 0,
        rejected: p?.rejectedNames ?? 0,
        transitions: this.transitions,
        speechMs: this.speechMs,
        csrcSeen: this.csrc.seen,
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

  private onProbe(now: number, p: DomProbe) {
    this.probe = p;
    this.lastProbe = now;
    this.dirty = false;
    const remote = p.tiles.filter((t) => !t.self);
    for (const t of remote) {
      const was = this.wasSpeaking.get(t.key) ?? false;
      if (t.speaking && !was) this.transitions++;
      this.wasSpeaking.set(t.key, t.speaking);
    }
    // Tile timeline, only once the audio proved to have no CSRCs.
    if (!this.csrc.seen && this.speechMs >= DOM_TIMELINE_AFTER_MS && this.speakingUsable()) {
      const lit = new Map(remote.filter((t) => t.speaking).map((t) => [t.key, t.name] as const));
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
