/* Long-form engine runs as the workspace shows them: at most one microphone
   session (or one System audio + mic session, #139 — both hold the mic),
   one file and one link at a time. A pure reducer over the engine's events
   (engine-download / -started / -progress / -segment / -warning / -done /
   -error), kept outside React so the routing rules are unit-tested. */

import type {
  EngineDone,
  EngineDownload,
  EngineError,
  EngineProgress,
  EngineResult,
  EngineSegmentEvent,
  EngineStarted,
  EngineStatus,
  EngineWarning,
  ItemType,
  Segment,
} from "./types";

export type RunKind = "mic" | "file" | "link" | "system";

/** Runs that record live from an input device and end with Stop. */
export const isCaptureKind = (kind: RunKind): boolean => kind === "mic" || kind === "system";

/** Warnings a run keeps for display (the latest ones). */
export const MAX_WARNINGS = 5;
export type RunStatus = "running" | "stopping" | "done" | "error";

/** A transcript line as the live view needs it (no word timings: an hour of
 *  them is dead weight in React state). */
export type LiveSegment = Pick<Segment, "id" | "start_ms" | "end_ms" | "text" | "raw" | "stt_error">;

export interface Run {
  kind: RunKind;
  /** Known at once for the mic (engine_start_mic returns it); for a file it
   *  is claimed from the run's first event, since transcribe_file only
   *  resolves at the end. Every engine-* event is routed by this id (never
   *  "the latest event"), so a mic session's progress can't show up as a
   *  file's. Since #153 every run opens with `engine-started`, so the claim
   *  happens on that event, before any progress. Should transcribe_file
   *  return the id up front one day, pass it to `started` and no claim
   *  happens. */
  sessionId: number | null;
  /** File name, the link (shortened), or the mic session's title. */
  label: string;
  status: RunStatus;
  /** Date.now() when the run started (the mic's elapsed clock). */
  startedAt: number;
  /** The run's archive item: provisional from `engine-started` (an untitled
   *  session's folder is renamed at the end), final from `engine-done`, or
   *  the item a failed run kept as interrupted. */
  itemId: string | null;
  /** The item's type, from `engine-started` (unknown until then). A mic
   *  session is a meeting when it records a room (#130). */
  itemType?: ItemType;
  /** The provisional id when the final one differs, so a selection on the
   *  live item can follow the rename. */
  previousItemId: string | null;
  progress: EngineProgress | null;
  /** A link run's download (#123), before its transcription starts. */
  download: EngineDownload | null;
  segments: LiveSegment[];
  /** `engine-warning` messages while it runs (#139: a device lost, device
   *  clocks realigned), oldest first, at most {@link MAX_WARNINGS}. */
  warnings: string[];
  result: EngineDone | null;
  error: string | null;
}

export interface RunsState {
  mic: Run | null;
  file: Run | null;
  /** A link run (#123): its id is known at once (engine_start_link
   *  returns it), like the mic's. */
  link: Run | null;
  /** A System audio + mic session (#139): the mic and a loopback device as
   *  two channels of a meeting; its id is known at once, like the mic's. */
  system: Run | null;
  /** Events of session ids no run has claimed yet, oldest first (#158): a
   *  mic session's `engine-started` can arrive before `engine_start_mic`
   *  resolves with its id. Replayed when a run starts with that id; capped
   *  at {@link MAX_BUFFERED} (events of sessions we never start — another
   *  window, the local API — are dropped from the front). */
  pending: EngineEventAction[];
}

export const initialRuns: RunsState = { mic: null, file: null, link: null, system: null, pending: [] };

/** How many unclaimed events {@link RunsState.pending} keeps. */
export const MAX_BUFFERED = 64;

/** An engine-* event, routed by its session id. */
export type EngineEventAction =
  | { type: "download"; payload: EngineDownload }
  | { type: "engine-started"; payload: EngineStarted }
  | { type: "progress"; payload: EngineProgress }
  | { type: "segment"; payload: EngineSegmentEvent }
  | { type: "warning"; payload: EngineWarning }
  | { type: "done"; payload: EngineDone }
  | { type: "error"; payload: EngineError };

export type RunsAction =
  | { type: "started"; kind: RunKind; sessionId: number | null; label: string; now: number; itemType?: ItemType }
  /** engine_status at mount: runs started elsewhere (#158). */
  | { type: "adopt"; status: EngineStatus; now: number }
  | { type: "stopping"; kind: RunKind }
  | EngineEventAction
  /** transcribe_file resolved: its result, even if no event was routed. */
  | { type: "resolved"; kind: RunKind; result: EngineResult }
  /** transcribe_file / engine_start_mic rejected. */
  | { type: "failed"; kind: RunKind; error: string }
  | { type: "dismiss"; kind: RunKind };

/** A run still capturing or finishing (not done, not failed). */
export type LiveRun = Run & { status: "running" | "stopping" };

const isLive = (r: Run | null): r is LiveRun => !!r && (r.status === "running" || r.status === "stopping");

/** Which run an event with this session id belongs to: the mic or the link
 *  when the id matches, else the running file (claiming the id on its
 *  first event). A download event only ever belongs to a link run: a file
 *  never claims one. Events of sessions we did not start (another window,
 *  the local API) are ignored. */
export function routeEvent(state: RunsState, sessionId: number, type?: EngineEventAction["type"]): RunKind | null {
  if (state.mic && state.mic.sessionId === sessionId) return "mic";
  if (state.system && state.system.sessionId === sessionId) return "system";
  if (state.link && state.link.sessionId === sessionId) return "link";
  if (state.file && state.file.sessionId === sessionId) return "file";
  if (type !== "download" && isLive(state.file) && state.file.sessionId === null) return "file";
  return null;
}

function update(state: RunsState, kind: RunKind, f: (r: Run) => Run): RunsState {
  const run = state[kind];
  return run ? { ...state, [kind]: f(run) } : state;
}

/** The final item id, remembering the provisional one if it changed. */
function finalId(run: Run, itemId: string): Pick<Run, "itemId" | "previousItemId"> {
  const moved = run.itemId !== null && run.itemId !== itemId;
  return { itemId, previousItemId: moved ? run.itemId : run.previousItemId };
}

export function runsReducer(state: RunsState, action: RunsAction): RunsState {
  switch (action.type) {
    case "started": {
      const started: RunsState = {
        ...state,
        [action.kind]: {
          kind: action.kind,
          sessionId: action.sessionId,
          label: action.label,
          status: "running",
          startedAt: action.now,
          itemId: null,
          ...(action.itemType ? { itemType: action.itemType } : {}),
          previousItemId: null,
          progress: null,
          download: null,
          segments: [],
          warnings: [],
          result: null,
          error: null,
        } satisfies Run,
      };
      // Replay what this session emitted before its id was known here.
      const id = action.sessionId;
      if (id === null) return started;
      const mine = state.pending.filter((e) => e.payload.session_id === id);
      if (mine.length === 0) return started;
      const rest = state.pending.filter((e) => e.payload.session_id !== id);
      return mine.reduce<RunsState>(runsReducer, { ...started, pending: rest });
    }
    case "adopt": {
      // Sessions that outlive the UI that started them — a window reload
      // mid-run (#158): the mic session, the oldest file
      // and link transcriptions, unless a run of that kind is live here.
      const { status, now } = action;
      const adopt: { kind: RunKind; sessionId: number; label: string }[] = [];
      if (status.mic_session !== null) adopt.push({ kind: "mic", sessionId: status.mic_session, label: "Microphone" });
      if (status.system_session != null)
        adopt.push({ kind: "system", sessionId: status.system_session, label: "System audio + mic" });
      const file = status.file_sessions?.[0];
      if (file) adopt.push({ kind: "file", sessionId: file.session_id, label: file.label });
      const link = status.link_sessions?.[0];
      if (link) adopt.push({ kind: "link", sessionId: link.session_id, label: link.label });
      return adopt.reduce<RunsState>((s, a) => {
        const current = s[a.kind];
        if (isLive(current) || current?.sessionId === a.sessionId) return s;
        return runsReducer(s, { type: "started", ...a, now });
      }, state);
    }
    case "stopping":
      return update(state, action.kind, (r) => (r.status === "running" ? { ...r, status: "stopping" } : r));
    case "download":
    case "engine-started":
    case "progress":
    case "segment":
    case "warning":
    case "done":
    case "error": {
      const kind = routeEvent(state, action.payload.session_id, action.type);
      if (!kind) return { ...state, pending: [...state.pending, action].slice(-MAX_BUFFERED) };
      return update(state, kind, (r) => {
        const run = { ...r, sessionId: action.payload.session_id };
        switch (action.type) {
          case "download":
            return { ...run, download: action.payload };
          case "engine-started":
            return { ...run, itemId: action.payload.item_id, itemType: action.payload.item_type };
          case "progress":
            return { ...run, progress: action.payload };
          case "segment": {
            const { id, start_ms, end_ms, text, raw, stt_error } = action.payload.segment;
            // A re-sent segment (same id) replaces the earlier copy.
            const segments = run.segments.filter((s) => s.id !== id);
            segments.push({ id, start_ms, end_ms, text, raw, ...(stt_error ? { stt_error } : {}) });
            segments.sort((a, b) => a.start_ms - b.start_ms);
            return { ...run, segments };
          }
          case "warning":
            return { ...run, warnings: [...run.warnings, action.payload.message].slice(-MAX_WARNINGS) };
          case "done":
            return { ...run, status: "done", result: action.payload, ...finalId(run, action.payload.item_id) };
          case "error":
            // The item a failed run kept as interrupted, or nothing left.
            return { ...run, status: "error", error: action.payload.error, itemId: action.payload.item_id ?? null };
        }
      });
    }
    case "resolved":
      return update(state, action.kind, (r) =>
        r.status === "done"
          ? r
          : {
              ...r,
              status: "done",
              error: null,
              ...finalId(r, action.result.item_id),
              result: { ...action.result, session_id: action.result.session_id ?? r.sessionId ?? -1 },
            },
      );
    case "failed":
      return update(state, action.kind, (r) =>
        r.status === "done" ? r : { ...r, status: "error", error: r.error ?? action.error },
      );
    case "dismiss":
      return isLive(state[action.kind]) ? state : { ...state, [action.kind]: null };
  }
}

/** Whether a run of this kind can be started now. A mic or link session
 *  waits until a running file has claimed its session id: its first events
 *  could otherwise arrive before engine_start_mic / engine_start_link
 *  resolves and be taken for the file's. (Callers also block a file start
 *  while either is in flight, for the same reason.) */
export function canStart(state: RunsState, kind: RunKind): boolean {
  if (isLive(state[kind])) return false;
  // A mic session and a System audio + mic session both hold the mic.
  if (kind === "mic" && isLive(state.system)) return false;
  if (kind === "system" && isLive(state.mic)) return false;
  if (kind !== "file" && isLive(state.file) && state.file.sessionId === null) return false;
  return true;
}

export function isRunning(run: Run | null): run is LiveRun {
  return isLive(run);
}

/** A cancelled run ends with the engine's "cancelled" error: nothing saved. */
export function wasCancelled(run: Run): boolean {
  return run.status === "error" && /cancel/i.test(run.error ?? "");
}

/** Whether New shows Discard for a mic run (#158): only while it records.
 *  Once Stop is pressed the run is finishing into a saved note, and a stray
 *  click on Discard must not throw that away. */
export function showDiscard(run: Run | null): boolean {
  return !!run && run.status === "running";
}

/** Discard asks first (#158): since #153 the session's item holds all that
 *  was said so far. `ask` opens the confirmation, `keep` closes it, and
 *  `confirm` cancels the session — only from an open confirmation. */
export type DiscardStep = "idle" | "confirming";
export type DiscardAction = "ask" | "keep" | "confirm";

export function discardStep(step: DiscardStep, action: DiscardAction): { step: DiscardStep; cancel: boolean } {
  switch (action) {
    case "ask":
      return { step: "confirming", cancel: false };
    case "keep":
      return { step: "idle", cancel: false };
    case "confirm":
      return { step: "idle", cancel: step === "confirming" };
  }
}

/** Short status line under a running session: "Transcribing · 2 s behind". */
export function describeProgress(run: Run): string {
  if (run.status === "stopping") {
    const left = run.progress ? Math.max(0, Math.round(run.progress.backlog_s)) : 0;
    return left > 0 ? `Finishing · ${left} s of audio left` : "Finishing…";
  }
  const p = run.progress;
  if (!p) return isCaptureKind(run.kind) ? "Listening…" : run.kind === "link" && run.itemId === null ? "Downloading…" : "Starting…";
  const behind = Math.round(p.backlog_s);
  const queue = p.queue_len > 0 ? ` · ${p.queue_len} queued` : "";
  return behind >= 1 ? `Transcribing · ${behind} s behind${queue}` : `Up to date${queue}`;
}
