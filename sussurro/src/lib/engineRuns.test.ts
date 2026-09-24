import { describe, expect, it } from "vitest";
import {
  canStart,
  describeProgress,
  discardStep,
  initialRuns,
  MAX_BUFFERED,
  routeEvent,
  runsReducer,
  showDiscard,
  wasCancelled,
  type RunsAction,
  type RunsState,
} from "./engineRuns";
import type { EngineProgress } from "./types";

const progress = (session_id: number, over: Partial<EngineProgress> = {}): EngineProgress => ({
  session_id,
  processed_s: 10,
  ingested_s: 12,
  total_s: 40,
  backlog_s: 2,
  queue_len: 1,
  segments_done: 1,
  ...over,
});

const seg = (session_id: number, id: number, start_ms: number, text = `line ${id}`) => ({
  type: "segment" as const,
  payload: { session_id, segment: { id, start_ms, end_ms: start_ms + 900, raw: text, text, words: [{ w: "x", start_ms: 0, end_ms: 1 }] } },
});

const run = (actions: RunsAction[], from: RunsState = initialRuns) => actions.reduce(runsReducer, from);

describe("runsReducer routing", () => {
  it("routes by session id: a mic session's progress never lands on the file", () => {
    const s = run([
      { type: "started", kind: "mic", sessionId: 7, label: "", now: 0 },
      { type: "started", kind: "file", sessionId: null, label: "call.wav", now: 0 },
      { type: "progress", payload: progress(7, { processed_s: 99 }) },
    ]);
    expect(s.mic?.progress?.processed_s).toBe(99);
    expect(s.file?.progress).toBeNull();
    expect(s.file?.sessionId).toBeNull(); // the mic's id was not claimed
  });

  it("lets a running file claim the first unknown session id, then only that one", () => {
    const s = run([
      { type: "started", kind: "file", sessionId: null, label: "call.wav", now: 0 },
      { type: "progress", payload: progress(3) },
      { type: "progress", payload: progress(4, { processed_s: 1 }) }, // someone else's
    ]);
    expect(s.file?.sessionId).toBe(3);
    expect(s.file?.progress?.session_id).toBe(3);
    expect(routeEvent(s, 4)).toBeNull();
  });

  it("routes no event when nothing is running (it is only buffered)", () => {
    const s = run([{ type: "progress", payload: progress(1) }]);
    expect(s.mic).toBeNull();
    expect(s.file).toBeNull();
    expect(s.pending).toHaveLength(1);
  });

  // #158 finding 6: the engine thread emits engine-started as soon as the
  // item exists, which can beat engine_start_mic's reply to the webview.
  it("replays a mic session's early events once its id is known", () => {
    const started = {
      type: "engine-started" as const,
      payload: { session_id: 9, item_id: "2026/09/2026-09-24-untitled", item_type: "note" as const, title: "", source: "mic" },
    };
    const s = run([
      started,
      { type: "progress", payload: progress(9, { processed_s: 3 }) },
      { type: "progress", payload: progress(12) }, // someone else's session
      { type: "started", kind: "mic", sessionId: 9, label: "", now: 0 },
    ]);
    expect(s.mic?.itemId).toBe("2026/09/2026-09-24-untitled");
    expect(s.mic?.progress?.processed_s).toBe(3);
    expect(s.pending.map((e) => e.payload.session_id)).toEqual([12]);
  });

  it("keeps a bounded buffer of unclaimed events", () => {
    const flood = Array.from({ length: MAX_BUFFERED + 10 }, (_, i) => ({ type: "progress" as const, payload: progress(100 + i) }));
    const s = run(flood);
    expect(s.pending).toHaveLength(MAX_BUFFERED);
    expect(s.pending[0].payload.session_id).toBe(110); // oldest dropped
  });

  it("collects segments in time order, dedups by id, drops word timings", () => {
    const s = run([
      { type: "started", kind: "mic", sessionId: 1, label: "", now: 0 },
      seg(1, 1, 5000),
      seg(1, 0, 0),
      seg(1, 1, 5000, "line 1 again"),
    ]);
    expect(s.mic?.segments.map((x) => x.text)).toEqual(["line 0", "line 1 again"]);
    expect(s.mic?.segments[0]).not.toHaveProperty("words");
  });

  it("finishes on done and keeps the result", () => {
    const s = run([
      { type: "started", kind: "mic", sessionId: 1, label: "", now: 0 },
      { type: "stopping", kind: "mic" },
      {
        type: "done",
        payload: { session_id: 1, item_id: "2026/09/x", item_type: "note", title: "x", text: "t", segments: 1, duration_s: 3 },
      },
    ]);
    expect(s.mic?.status).toBe("done");
    expect(s.mic?.result?.item_id).toBe("2026/09/x");
  });

  it("marks errors and cancellations", () => {
    const s = run([
      { type: "started", kind: "file", sessionId: null, label: "a.wav", now: 0 },
      { type: "progress", payload: progress(2) },
      { type: "error", payload: { session_id: 2, error: "cancelled" } },
      { type: "failed", kind: "file", error: "transcribe_file: cancelled" },
    ]);
    expect(s.file?.status).toBe("error");
    expect(s.file?.error).toBe("cancelled"); // the event's message wins
    expect(wasCancelled(s.file!)).toBe(true);
  });

  it("uses transcribe_file's result when no event was routed", () => {
    const s = run([
      { type: "started", kind: "file", sessionId: null, label: "a.wav", now: 0 },
      { type: "resolved", kind: "file", result: { item_id: "i", item_type: "transcription", title: "a", text: "", segments: 0, duration_s: 0 } },
    ]);
    expect(s.file?.status).toBe("done");
    expect(s.file?.result?.item_type).toBe("transcription");
  });

  it("dismisses only finished runs", () => {
    const live = run([{ type: "started", kind: "mic", sessionId: 1, label: "", now: 0 }]);
    expect(run([{ type: "dismiss", kind: "mic" }], live).mic).not.toBeNull();
    const done = run([{ type: "error", payload: { session_id: 1, error: "boom" } }, { type: "dismiss", kind: "mic" }], live);
    expect(done.mic).toBeNull();
  });
});

describe("runsReducer with the #153 events", () => {
  const started = (session_id: number, item_id: string) => ({
    type: "engine-started" as const,
    payload: { session_id, item_id, item_type: "note" as const, title: "", source: "mic" },
  });

  it("claims the file's id on engine-started and tracks the item", () => {
    const s = run([
      { type: "started", kind: "file", sessionId: null, label: "a.wav", now: 0 },
      started(9, "2026/09/2026-09-24-a"),
    ]);
    expect(s.file?.sessionId).toBe(9);
    expect(s.file?.itemId).toBe("2026/09/2026-09-24-a");
  });

  it("follows an untitled item's rename at the end", () => {
    const s = run([
      { type: "started", kind: "mic", sessionId: 1, label: "", now: 0 },
      started(1, "2026/09/2026-09-24-untitled"),
      {
        type: "done",
        payload: { session_id: 1, item_id: "2026/09/2026-09-24-idee", item_type: "note", title: "Idee", text: "", segments: 2, duration_s: 9 },
      },
    ]);
    expect(s.mic?.itemId).toBe("2026/09/2026-09-24-idee");
    expect(s.mic?.previousItemId).toBe("2026/09/2026-09-24-untitled");
  });

  it("keeps the interrupted item a failed run left behind", () => {
    const s = run([
      { type: "started", kind: "mic", sessionId: 1, label: "", now: 0 },
      started(1, "x/untitled"),
      { type: "error", payload: { session_id: 1, error: "device lost", item_id: "x/untitled" } },
    ]);
    expect(s.mic?.status).toBe("error");
    expect(s.mic?.itemId).toBe("x/untitled");
    const cancelled = run([
      { type: "started", kind: "mic", sessionId: 1, label: "", now: 0 },
      started(1, "x/untitled"),
      { type: "error", payload: { session_id: 1, error: "cancelled" } },
    ]);
    expect(cancelled.mic?.itemId).toBeNull();
  });

  it("keeps the stt_error marker of a failed segment", () => {
    const s = run([
      { type: "started", kind: "mic", sessionId: 1, label: "", now: 0 },
      {
        type: "segment",
        payload: { session_id: 1, segment: { id: 0, start_ms: 0, end_ms: 900, raw: "", text: "", stt_error: "model crashed" } },
      },
    ]);
    expect(s.mic?.segments[0].stt_error).toBe("model crashed");
  });
});

describe("canStart", () => {
  it("allows one run per kind and holds the mic until the file has an id", () => {
    expect(canStart(initialRuns, "mic")).toBe(true);
    const fileStarting = run([{ type: "started", kind: "file", sessionId: null, label: "a", now: 0 }]);
    expect(canStart(fileStarting, "file")).toBe(false);
    expect(canStart(fileStarting, "mic")).toBe(false);
    const fileClaimed = run([{ type: "progress", payload: progress(5) }], fileStarting);
    expect(canStart(fileClaimed, "mic")).toBe(true);
  });
});

// #158 finding 7: a UI mounted mid-run (ui_v2 switched, window reloaded)
// adopts what engine_status reports — the file transcription too, so it
// can be followed and cancelled instead of running unseen.
describe("adopt", () => {
  const status = (mic: number | null, files: { session_id: number; label: string }[] = []) => ({
    active: (mic === null ? 0 : 1) + files.length,
    mic_session: mic,
    file_sessions: files,
  });

  it("adopts the mic session and a running file", () => {
    const s = run([{ type: "adopt", status: status(3, [{ session_id: 4, label: "call.wav" }]), now: 0 }]);
    expect(s.mic).toMatchObject({ sessionId: 3, status: "running" });
    expect(s.file).toMatchObject({ sessionId: 4, label: "call.wav", status: "running" });
    // It can't be doubled, and its events land on it.
    expect(canStart(s, "file")).toBe(false);
    const p = run([{ type: "progress", payload: progress(4, { processed_s: 20 }) }], s);
    expect(p.file?.progress?.processed_s).toBe(20);
    expect(p.mic?.progress).toBeNull();
  });

  it("never replaces a run this window already follows", () => {
    const own = run([{ type: "started", kind: "file", sessionId: 8, label: "mine.wav", now: 0 }]);
    const s = run([{ type: "adopt", status: status(null, [{ session_id: 8, label: "mine.wav" }]), now: 5 }], own);
    expect(s).toEqual(own);
    expect(run([{ type: "adopt", status: status(null), now: 0 }])).toEqual(initialRuns);
  });
});

describe("Discard (#158)", () => {
  const recording = run([{ type: "started", kind: "mic", sessionId: 4, label: "", now: 0 }]);

  it("is offered only while the session records, never once Stop was pressed", () => {
    expect(showDiscard(null)).toBe(false);
    expect(showDiscard(recording.mic)).toBe(true);
    const stopping = run([{ type: "stopping", kind: "mic" }], recording);
    expect(showDiscard(stopping.mic)).toBe(false);
    const done = run(
      [{ type: "done", payload: { session_id: 4, item_id: "a", item_type: "note", title: "t", text: "", segments: 1, duration_s: 1 } }],
      stopping,
    );
    expect(showDiscard(done.mic)).toBe(false);
  });

  it("cancels the session only after an explicit confirmation", () => {
    // A single click never discards.
    expect(discardStep("idle", "confirm")).toEqual({ step: "idle", cancel: false });
    const asked = discardStep("idle", "ask");
    expect(asked).toEqual({ step: "confirming", cancel: false });
    // Keep recording closes the question without cancelling.
    expect(discardStep(asked.step, "keep")).toEqual({ step: "idle", cancel: false });
    expect(discardStep(asked.step, "confirm")).toEqual({ step: "idle", cancel: true });
  });
});

describe("describeProgress", () => {
  it("reports backlog and queue", () => {
    const s = run([
      { type: "started", kind: "mic", sessionId: 1, label: "", now: 0 },
    ]);
    expect(describeProgress(s.mic!)).toBe("Listening…");
    const behind = run([{ type: "progress", payload: progress(1, { backlog_s: 2.4, queue_len: 2 }) }], s);
    expect(describeProgress(behind.mic!)).toBe("Transcribing · 2 s behind · 2 queued");
    const caught = run([{ type: "progress", payload: progress(1, { backlog_s: 0.2, queue_len: 0 }) }], s);
    expect(describeProgress(caught.mic!)).toBe("Up to date");
    const stopping = run([{ type: "stopping", kind: "mic" }], behind);
    expect(describeProgress(stopping.mic!)).toBe("Finishing · 2 s of audio left");
  });
});
