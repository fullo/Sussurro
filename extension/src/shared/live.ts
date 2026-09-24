/* The live transcript the side panel mirrors (#129, E3: a mirror only —
 * editing happens in the app). Pure — unit tested.
 *
 * The background keeps one `LiveTranscript` per tab and applies to it every
 * app → extension `/live` message (`sussurro/src-tauri/src/api/protocol.rs`):
 *
 * - `segment {kind: "new" | "updated", segment}` — a line, merged by id;
 * - `speaker {id, label}` — a speaker's name (from #131);
 * - `status {state, item_id?, backlog_s?, …}` — the item and the backlog.
 *
 * Each applied action bumps `rev`; the panel takes a snapshot, then applies
 * the broadcast actions in order (a gap → a new snapshot). A reconnect is a
 * new `start` on the app, i.e. a new item: its lines go in a new `part`, so
 * the panel keeps showing the earlier ones. A new Start resets it all.
 *
 * The background imports this module: keep it free of React and of
 * `@sussurro/transcript` (its CSS import needs a DOM, which Chrome's
 * service worker lacks). The panel-only rendering is in sidepanel/lines.ts. */
import { chipFor, lineSpeakerId, type KnownSpeaker } from "./speakers";

/** `LiveSegment` as sent by the app. */
export interface LiveLine {
  id: number;
  channel: "mic" | "remote";
  start_ms: number;
  end_ms: number;
  text: string;
  speaker_id?: string;
  /** The engine could not transcribe this stretch. */
  failed?: boolean;
}

export type AppMessage =
  | { type: "segment"; kind: "new" | "updated"; segment: LiveLine }
  | { type: "speaker"; id: string; label: string; color?: string }
  | {
      type: "status";
      state: string;
      item_id?: string;
      backlog_s?: number;
      processed_s?: number;
      queue_len?: number;
      message?: string;
    };

/** One app item: the lines of one connection. */
export interface LivePart {
  itemId: string | null;
  /** In time order. */
  lines: LiveLine[];
  speakers: Record<string, KnownSpeaker>;
}

export interface LiveProgress {
  /** `recording` or `finishing`. */
  state: string;
  backlogS: number;
  processedS?: number;
  /** When the app said it (ms since epoch). */
  at: number;
}

export interface LiveTranscript {
  /** Names this transcript: a restarted background starts a new one, whose
   *  `rev` starts again from 0. */
  epoch: string;
  rev: number;
  parts: LivePart[];
  /** Last progress `status` of the current connection. */
  progress: LiveProgress | null;
  /** Last `warning` from the app (the session goes on). */
  warning: string | null;
}

export type LiveAction = { kind: "reset" } | { kind: "app"; msg: AppMessage; at: number };

export const initialTranscript = (epoch = ""): LiveTranscript => ({ epoch, rev: 0, parts: [], progress: null, warning: null });

const MAX_TEXT = 20_000;
const MAX_ID = 200;

const isObj = (v: unknown): v is Record<string, unknown> => !!v && typeof v === "object" && !Array.isArray(v);
const num = (v: unknown): number | undefined => (typeof v === "number" && Number.isFinite(v) ? v : undefined);
const str = (v: unknown, max = MAX_ID): string | undefined => (typeof v === "string" ? v.slice(0, max) : undefined);

/** Validate an app message (anything else → null: it is ignored). */
export function parseAppMessage(raw: unknown): AppMessage | null {
  if (!isObj(raw)) return null;
  switch (raw.type) {
    case "segment": {
      const s = raw.segment;
      if ((raw.kind !== "new" && raw.kind !== "updated") || !isObj(s)) return null;
      const id = num(s.id);
      const start = num(s.start_ms);
      if (id === undefined || start === undefined || typeof s.text !== "string") return null;
      const line: LiveLine = {
        id,
        channel: s.channel === "mic" ? "mic" : "remote",
        start_ms: Math.max(0, start),
        end_ms: Math.max(start, num(s.end_ms) ?? start),
        text: s.text.slice(0, MAX_TEXT),
      };
      const speaker = str(s.speaker_id);
      if (speaker) line.speaker_id = speaker;
      if (s.failed === true) line.failed = true;
      return { type: "segment", kind: raw.kind, segment: line };
    }
    case "speaker": {
      const id = str(raw.id);
      const label = str(raw.label);
      if (!id || label === undefined) return null;
      const color = str(raw.color, 16);
      return color ? { type: "speaker", id, label, color } : { type: "speaker", id, label };
    }
    case "status": {
      const state = str(raw.state, 32);
      if (!state) return null;
      const m: AppMessage = { type: "status", state };
      const item = str(raw.item_id, 500);
      if (item) m.item_id = item;
      const backlog = num(raw.backlog_s);
      if (backlog !== undefined) m.backlog_s = Math.max(0, backlog);
      const processed = num(raw.processed_s);
      if (processed !== undefined) m.processed_s = processed;
      const queue = num(raw.queue_len);
      if (queue !== undefined) m.queue_len = queue;
      const message = str(raw.message, 500);
      if (message) m.message = message;
      return m;
    }
    default:
      return null;
  }
}

const emptyPart = (itemId: string | null = null): LivePart => ({ itemId, lines: [], speakers: {} });

/** Replace the last part (or start the first). */
function withCurrent(t: LiveTranscript, f: (p: LivePart) => LivePart): LivePart[] {
  const parts = t.parts.length ? t.parts : [emptyPart()];
  return [...parts.slice(0, -1), f(parts[parts.length - 1])];
}

/** Merge a line by id, keeping time order. */
function upsert(lines: LiveLine[], line: LiveLine): LiveLine[] {
  const i = lines.findIndex((l) => l.id === line.id);
  if (i >= 0) {
    const next = lines.slice();
    next[i] = line;
    if (next[i].start_ms === lines[i].start_ms) return next;
    return next.sort(byTime);
  }
  const last = lines[lines.length - 1];
  if (!last || byTime(last, line) <= 0) return [...lines, line];
  return [...lines, line].sort(byTime);
}

const byTime = (a: LiveLine, b: LiveLine) => a.start_ms - b.start_ms || a.id - b.id;

export function applyLive(t: LiveTranscript, a: LiveAction): LiveTranscript {
  const rev = t.rev + 1;
  if (a.kind === "reset") return { ...initialTranscript(t.epoch), rev };
  const m = a.msg;
  switch (m.type) {
    case "segment":
      return { ...t, rev, parts: withCurrent(t, (p) => ({ ...p, lines: upsert(p.lines, m.segment) })) };
    case "speaker":
      return {
        ...t,
        rev,
        parts: withCurrent(t, (p) => ({ ...p, speakers: { ...p.speakers, [m.id]: m.color ? { label: m.label, color: m.color } : { label: m.label } } })),
      };
    case "status": {
      let parts = t.parts;
      let progress = t.progress;
      let warning = t.warning;
      const cur = parts[parts.length - 1];
      if (m.item_id) {
        if (!cur) parts = [emptyPart(m.item_id)];
        else if (cur.itemId === null) parts = withCurrent(t, (p) => ({ ...p, itemId: m.item_id! }));
        else if (cur.itemId !== m.item_id) {
          parts =
            m.state === "started"
              ? // A new item on the same tab: the connection was lost and
                // the background reconnected (a new `start` on the app).
                [...parts, emptyPart(m.item_id)]
              : // The same item, renamed at the end (an untitled meeting
                // takes its final title's folder name with `done`).
                withCurrent(t, (p) => ({ ...p, itemId: m.item_id! }));
        }
      }
      switch (m.state) {
        case "ready":
        case "started":
          progress = null;
          warning = null;
          break;
        case "recording":
        case "finishing":
          progress = { state: m.state, backlogS: m.backlog_s ?? 0, processedS: m.processed_s, at: a.at };
          break;
        case "done":
        case "error":
          progress = null;
          break;
        case "warning":
          warning = m.message ?? warning;
          break;
      }
      return { ...t, rev, parts, progress, warning };
    }
  }
}

// ---- the panel's copy -----------------------------------------------------------

/** The side panel's copy of a tab's transcript: a snapshot from the
 *  background plus the broadcast actions, applied in `rev` order. */
export interface Mirror {
  t: LiveTranscript | null;
  /** Actions received while waiting for a snapshot. */
  pending: { epoch: string; rev: number; action: LiveAction }[];
}

export const emptyMirror = (): Mirror => ({ t: null, pending: [] });

/** Most actions kept while waiting for a snapshot. */
const MAX_PENDING = 500;

/** A broadcast action. `refetch`: the copy missed something (a gap in
 *  `rev`, another epoch): ask for a new snapshot. */
export function mirrorReceive(m: Mirror, epoch: string, rev: number, action: LiveAction): { m: Mirror; refetch: boolean; applied: boolean } {
  const t = m.t;
  if (t && t.epoch === epoch) {
    if (rev <= t.rev) return { m, refetch: false, applied: false };
    if (rev === t.rev + 1) return { m: { ...m, t: applyLive(t, action) }, refetch: false, applied: true };
  }
  // Out of step, or no snapshot yet: keep it for after the next snapshot.
  const pending = [...m.pending, { epoch, rev, action }].slice(-MAX_PENDING);
  return { m: { t, pending }, refetch: t !== null, applied: false };
}

/** A snapshot from the background, plus what arrived meanwhile. */
export function mirrorSnapshot(m: Mirror, snap: LiveTranscript): { m: Mirror; refetch: boolean } {
  let t = snap;
  let refetch = false;
  const later = m.pending.filter((p) => p.epoch === snap.epoch && p.rev > snap.rev).sort((a, b) => a.rev - b.rev);
  for (const p of later) {
    if (p.rev !== t.rev + 1) {
      refetch = true;
      break;
    }
    t = applyLive(t, p.action);
  }
  return { m: { t, pending: refetch ? later.filter((p) => p.rev > t.rev) : [] }, refetch };
}

/** The current (latest) item. */
export function currentItemId(t: LiveTranscript): string | null {
  for (let i = t.parts.length - 1; i >= 0; i--) if (t.parts[i].itemId) return t.parts[i].itemId;
  return null;
}

export function lineCount(t: LiveTranscript): number {
  return t.parts.reduce((n, p) => n + p.lines.length, 0);
}


/** One line of text for a screen reader: "Anna: hello". */
export function announceText(p: LivePart, line: LiveLine): string {
  const sid = lineSpeakerId(line);
  const who = sid ? chipFor(sid, p.speakers[sid]).label : null;
  const text = line.failed && !line.text.trim() ? "not transcribed" : line.text.trim();
  return who ? `${who}: ${text}` : text;
}

export interface BacklogView {
  text: string;
  tone: "ok" | "lag" | "busy";
}

/** Behind this many seconds, the transcript is "behind". */
export const BACKLOG_SHOW_S = 3;
/** Behind this many, Sussurro is falling behind (a slow model). */
export const BACKLOG_LAG_S = 30;

export function formatSeconds(s: number): string {
  const r = Math.round(s);
  if (r < 60) return `${r} s`;
  const m = Math.floor(r / 60);
  const rest = r % 60;
  return rest ? `${m} min ${rest} s` : `${m} min`;
}

/** The backlog / latency indicator (null: nothing to show). */
export function backlogView(t: LiveTranscript): BacklogView | null {
  const p = t.progress;
  if (!p) return null;
  if (p.state === "finishing") {
    return {
      text: p.backlogS >= 1 ? `Finishing: about ${formatSeconds(p.backlogS)} of audio left to transcribe.` : "Finishing the transcript…",
      tone: "busy",
    };
  }
  if (p.backlogS < BACKLOG_SHOW_S) return { text: "Transcript up to date.", tone: "ok" };
  if (p.backlogS < BACKLOG_LAG_S) return { text: `Transcript ${formatSeconds(p.backlogS)} behind.`, tone: "busy" };
  return {
    text: `Transcript ${formatSeconds(p.backlogS)} behind: Sussurro is transcribing slower than the meeting. Nothing is lost; it catches up after the call.`,
    tone: "lag",
  };
}
