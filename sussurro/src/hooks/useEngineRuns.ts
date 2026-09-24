import { useCallback, useEffect, useReducer, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import {
  canStart,
  initialRuns,
  runsReducer,
  type RunKind,
} from "../lib/engineRuns";
import { baseName } from "../lib/format";
import type {
  CleanupLevel,
  EngineDone,
  EngineError,
  EngineProgress,
  EngineResult,
  EngineSegmentEvent,
  EngineStarted,
  EngineStatus,
  ItemType,
} from "../lib/types";

/** A run's language and cleanup level (#157); null = the dictation setting. */
export type RunArgs = { language: string | null; cleanupLevel: CleanupLevel | null };
const NO_OPTIONS: RunArgs = { language: null, cleanupLevel: null };

/** The long-form engine's runs (one mic session, one file), fed by the
 *  engine-* events and routed by session id. Mount it once per window. */
export function useEngineRuns() {
  const [runs, dispatch] = useReducer(runsReducer, initialRuns);
  /** engine_start_mic in flight: a file start waits (see `canStart`). */
  const [micStarting, setMicStarting] = useState(false);
  const mounted = useRef(true);

  useEffect(() => {
    mounted.current = true;
    const subs = [
      listen<EngineStarted>("engine-started", (e) => dispatch({ type: "engine-started", payload: e.payload })),
      listen<EngineProgress>("engine-progress", (e) => dispatch({ type: "progress", payload: e.payload })),
      listen<EngineSegmentEvent>("engine-segment", (e) => dispatch({ type: "segment", payload: e.payload })),
      listen<EngineDone>("engine-done", (e) => dispatch({ type: "done", payload: e.payload })),
      listen<EngineError>("engine-error", (e) => dispatch({ type: "error", payload: e.payload })),
    ];
    // Sessions survive a window reload or a UI switch (ui_v2): adopt the mic
    // session and a running file transcription (#158).
    invoke<EngineStatus>("engine_status")
      .then((status) => {
        if (mounted.current) dispatch({ type: "adopt", status, now: Date.now() });
      })
      .catch(() => {});
    return () => {
      mounted.current = false;
      subs.forEach((p) => p.then((f) => f()));
    };
  }, []);

  const startMic = useCallback(async (title: string, itemType: ItemType = "note", options: RunArgs = NO_OPTIONS) => {
    setMicStarting(true);
    try {
      const id = await invoke<number>("engine_start_mic", { itemType, title: title.trim() || null, ...options });
      dispatch({ type: "started", kind: "mic", sessionId: id, label: title.trim(), now: Date.now() });
      return null;
    } catch (e) {
      return String(e);
    } finally {
      setMicStarting(false);
    }
  }, []);

  const stopMic = useCallback(async () => {
    dispatch({ type: "stopping", kind: "mic" });
    try {
      await invoke<number>("engine_stop_mic");
      return null;
    } catch (e) {
      dispatch({ type: "failed", kind: "mic", error: String(e) });
      return String(e);
    }
  }, []);

  /** Transcribe a file; resolves with the item (or null on error/cancel). */
  const startFile = useCallback(
    async (path: string, itemType: ItemType, title = "", options: RunArgs = NO_OPTIONS): Promise<EngineResult | null> => {
      // transcribe_file blocks until the end (its result carries the session
      // id only then), so the run claims the id of its first event,
      // engine-started (#153). Were the id returned up front, pass it here.
      dispatch({ type: "started", kind: "file", sessionId: null, label: baseName(path), now: Date.now() });
      try {
        const result = await invoke<EngineResult>("transcribe_file", {
          path,
          itemType,
          title: title.trim() || null,
          ...options,
        });
        dispatch({ type: "resolved", kind: "file", result });
        return result;
      } catch (e) {
        dispatch({ type: "failed", kind: "file", error: String(e) });
        return null;
      }
    },
    [],
  );

  const cancel = useCallback(
    async (kind: RunKind) => {
      const id = runs[kind]?.sessionId;
      if (id === null || id === undefined) return false;
      return invoke<boolean>("engine_cancel", { sessionId: id }).catch(() => false);
    },
    [runs],
  );

  const dismiss = useCallback((kind: RunKind) => dispatch({ type: "dismiss", kind }), []);

  return {
    runs,
    startMic,
    stopMic,
    startFile,
    cancel,
    dismiss,
    canStartMic: canStart(runs, "mic") && !micStarting,
    canStartFile: canStart(runs, "file") && !micStarting,
    micStarting,
  };
}

export type EngineRuns = ReturnType<typeof useEngineRuns>;
