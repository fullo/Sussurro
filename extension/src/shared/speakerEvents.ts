/* Speaker events from the meeting page to the app (#131, protocol 2).
 *
 *   MAIN world (observer) ──{pf}──▶ ISOLATED ──▶ background ──{t}──▶ app /live
 *
 * The page stamps each event with `pf`, its position in the audio the
 * page's worklet produced, in (fractional) frames of FRAME_SAMPLES — the
 * page knows its own frames but not how the background numbers them on the
 * current connection. The background knows both (`SeqCounter`), so it turns
 * `pf` into `t`, milliseconds on the connection's audio clock, which is the
 * clock the app puts its segments on (protocol.rs: `t` is the position in
 * the audio sent on this connection). Pure — unit tested. */
import { FRAME_SAMPLES } from "./frame";

/** What saw a speaker: the audio's RTP contributing sources, or the page's
 *  speaking indicator (captions are a later fallback, not sent yet). */
export type SpeakerSource = "rtp" | "dom";

/** Observer health, per hook. */
export type HookState = "ok" | "missing" | "broken" | "unknown";

export interface HealthReport {
  /** `names_unavailable`: the app falls back to "Voice N" for new speakers. */
  state: "ok" | "names_unavailable";
  /** The selector set in use (null: none matched the page). */
  set: string | null;
  hooks: Record<string, HookState>;
}

/** Page → background (MAIN stamps `pf`; messages without a time have none). */
export type PageSpeakerMsg =
  | { type: "speaker_active"; id: string; name?: string; source: SpeakerSource; pf: number }
  | { type: "speaker_idle"; id: string; source: SpeakerSource; pf: number }
  | { type: "speaker_name"; id: string; name: string | null }
  | { type: "participants"; names: string[] }
  | ({ type: "observer_health" } & HealthReport);

/** Background → app, as sent on `/live`. */
export type WireSpeakerMsg =
  | { type: "speaker_active"; t: number; id: string; name?: string; source: SpeakerSource }
  | { type: "speaker_idle"; t: number; id: string }
  | { type: "speaker_name"; id: string; name: string | null }
  | { type: "participants"; names: string[] }
  | ({ type: "observer_health" } & HealthReport);

const ID_RE = /^[A-Za-z0-9:_.-]{1,64}$/;
const HOOK_STATES = new Set<HookState>(["ok", "missing", "broken", "unknown"]);

const cleanStr = (v: unknown, max: number): string | null => {
  if (typeof v !== "string") return null;
  const s = v.replace(/\s+/g, " ").trim();
  return s && s.length <= max ? s : null;
};

/** A page message checked field by field (the page's world is not
 *  trusted): ids `[A-Za-z0-9:_.-]{1,64}`, names ≤ 120 characters, at most
 *  500 participants, finite frame positions. `null`: dropped. */
export function sanitizePageSpeaker(raw: unknown): PageSpeakerMsg | null {
  if (!raw || typeof raw !== "object") return null;
  const m = raw as Record<string, unknown>;
  const id = typeof m.id === "string" && ID_RE.test(m.id) ? m.id : null;
  const source: SpeakerSource = m.source === "rtp" ? "rtp" : "dom";
  const pf = typeof m.pf === "number" && Number.isFinite(m.pf) ? Math.max(0, m.pf) : null;
  switch (m.type) {
    case "speaker_active": {
      if (!id || pf === null) return null;
      const name = cleanStr(m.name, 120);
      return { type: "speaker_active", id, source, pf, ...(name ? { name } : {}) };
    }
    case "speaker_idle":
      return id && pf !== null ? { type: "speaker_idle", id, source, pf } : null;
    case "speaker_name":
      return id ? { type: "speaker_name", id, name: cleanStr(m.name, 120) } : null;
    case "participants": {
      if (!Array.isArray(m.names)) return null;
      const names = m.names.map((n) => cleanStr(n, 120)).filter((n): n is string => !!n).slice(0, 500);
      return { type: "participants", names };
    }
    case "observer_health": {
      const hooks: Record<string, HookState> = {};
      if (m.hooks && typeof m.hooks === "object") {
        for (const [k, v] of Object.entries(m.hooks as Record<string, unknown>).slice(0, 16)) {
          if (/^[A-Za-z0-9_]{1,32}$/.test(k)) hooks[k] = HOOK_STATES.has(v as HookState) ? (v as HookState) : "unknown";
        }
      }
      const set = typeof m.set === "string" && ID_RE.test(m.set) ? m.set : null;
      return { type: "observer_health", state: m.state === "ok" ? "ok" : "names_unavailable", set, hooks };
    }
    default:
      return null;
  }
}

/** Page frames → ms on the connection's clock, given `offset` = wire seq −
 *  page seq of the connection's page frames. Never negative (an event from
 *  before this connection's first frame is at its start). */
export function frameToMs(pf: number, offset: number, rate: number): number {
  const frames = pf + offset;
  if (!Number.isFinite(frames) || rate <= 0) return 0;
  return Math.max(0, Math.round((frames * FRAME_SAMPLES * 1000) / rate));
}

/** The page's clock: performance time → fractional page frames, anchored on
 *  the last worklet block (frame `seq` arrived at `at`). Before any block,
 *  frames count from `armedAt`. */
export function perfToFrame(at: number, anchor: { seq: number; at: number } | null, armedAt: number, rate: number): number {
  const perMs = rate / FRAME_SAMPLES / 1000;
  if (!anchor) return Math.max(0, (at - armedAt) * perMs);
  // Block `seq` covers frame [seq, seq + 1) and was posted when it ended.
  return Math.max(0, anchor.seq + 1 + (at - anchor.at) * perMs);
}

/** Most page messages held while the connection's clock is not known yet. */
export const MAX_PENDING = 256;
/** Distinct speaker ids kept per tab (the app follows at most 1000,
 *  `MAX_SPEAKER_KEYS`): a page inventing ids can't grow the state. */
export const MAX_SPEAKER_IDS = 1000;

const sameList = (a: readonly string[], b: readonly string[]) => a.length === b.length && a.every((x, i) => x === b[i]);

/** `map.set(k, v)` unless `k` is new and the map is full. */
function setBounded<V>(map: Map<string, V>, k: string, v: V): boolean {
  if (!map.has(k) && map.size >= MAX_SPEAKER_IDS) return false;
  map.set(k, v);
  return true;
}

/**
 * Per-tab relay in the background (#131): keeps the page's speaker state,
 * so a new connection (a reconnect is a new item on the app) starts with
 * the participants, names and active speakers already known, and converts
 * timed messages to the connection's clock.
 */
export class SpeakerRelay {
  private participants: string[] | null = null;
  private names = new Map<string, string | null>();
  private active = new Map<string, { name?: string; source: SpeakerSource }>();
  private health: HealthReport | null = null;
  private offset: number | null = null;
  private pending: PageSpeakerMsg[] = [];
  private rate = 48_000;

  /** The page's health, for the side panel. */
  get lastHealth(): HealthReport | null {
    return this.health;
  }

  /** A new connection: its clock is not known until its first page frame.
   *  Returns the state to send right after `start`. */
  begin(rate: number): WireSpeakerMsg[] {
    this.rate = rate > 0 ? rate : 48_000;
    this.offset = null;
    this.pending = [];
    const out: WireSpeakerMsg[] = [];
    if (this.participants) out.push({ type: "participants", names: [...this.participants] });
    for (const [id, name] of this.names) out.push({ type: "speaker_name", id, name });
    if (this.health) out.push({ type: "observer_health", ...this.health });
    for (const [id, a] of this.active) out.push({ type: "speaker_active", t: 0, id, source: a.source, ...(a.name ? { name: a.name } : {}) });
    return out;
  }

  /** A page frame went out as wire `seq` (mic channel, page producer):
   *  returns the held messages, now on the connection's clock. */
  frame(pageSeq: number, wireSeq: number): WireSpeakerMsg[] {
    const first = this.offset === null;
    this.offset = wireSeq - pageSeq;
    if (!first || !this.pending.length) return [];
    const held = this.pending;
    this.pending = [];
    return held.flatMap((m) => this.wire(m) ?? []);
  }

  /** A message from the page. `live`: a connection is open (else only the
   *  state is kept). Returns what to send now: nothing for a speaker past
   *  [`MAX_SPEAKER_IDS`] or a participant list equal to the last (#217). */
  page(m: PageSpeakerMsg, live: boolean): WireSpeakerMsg[] {
    switch (m.type) {
      case "speaker_active":
        if (!setBounded(this.active, m.id, { name: m.name, source: m.source })) return [];
        break;
      case "speaker_idle":
        this.active.delete(m.id);
        break;
      case "speaker_name":
        if (!setBounded(this.names, m.id, m.name)) return [];
        break;
      case "participants":
        if (this.participants && sameList(this.participants, m.names)) return [];
        this.participants = [...m.names];
        break;
      case "observer_health":
        this.health = { state: m.state, set: m.set, hooks: { ...m.hooks } };
        break;
    }
    if (!live) return [];
    const timed = m.type === "speaker_active" || m.type === "speaker_idle";
    if (timed && this.offset === null) {
      this.pending.push(m);
      if (this.pending.length > MAX_PENDING) this.pending.splice(0, this.pending.length - MAX_PENDING);
      return [];
    }
    const w = this.wire(m);
    return w ? [w] : [];
  }

  /** The page went away: forget everything. */
  clear(): void {
    this.participants = null;
    this.names.clear();
    this.active.clear();
    this.health = null;
    this.offset = null;
    this.pending = [];
  }

  private wire(m: PageSpeakerMsg): WireSpeakerMsg | null {
    const t = () => frameToMs((m as { pf: number }).pf, this.offset ?? 0, this.rate);
    switch (m.type) {
      case "speaker_active":
        return { type: "speaker_active", t: t(), id: m.id, source: m.source, ...(m.name ? { name: m.name } : {}) };
      case "speaker_idle":
        return { type: "speaker_idle", t: t(), id: m.id };
      case "speaker_name":
        return { type: "speaker_name", id: m.id, name: m.name };
      case "participants":
        return { type: "participants", names: [...m.names] };
      case "observer_health":
        return { type: "observer_health", state: m.state, set: m.set, hooks: { ...m.hooks } };
    }
  }
}
