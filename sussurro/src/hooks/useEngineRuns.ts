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
  EngineDownload,
  EngineError,
  EngineProgress,
  EngineResult,
  EngineSegmentEvent,
  EngineStarted,
  EngineStatus,
  EngineWarning,
  ItemType,
} from "../lib/types";

/** A run's language and cleanup level (#157); null = the dictation setting. */
export type RunArgs = {
  language: string | null;
  cleanupLevel: CleanupLevel | null;
  /** "Identify voices" (P11, #134): label a transcription's voices "Voice N".
   *  Omitted = off; the backend ignores it for notes. */
  identifyVoices?: boolean;
  /** "Save audio" (P9, #141): keep the run's audio in the item folder.
   *  Omitted = the per-app default (off unless turned on in Settings). */
  saveAudio?: boolean;
};
const NO_OPTIONS: RunArgs = { language: null, cleanupLevel: null };

/** A System audio + mic session's devices (#139). `mic` null = the
 *  dictation's input device. */
export type SystemDevices = { mic: string | null; system: string };

/** The long-form engine's runs (one mic or system-audio session, one file,
 *  one link), fed
 *  by the engine-* events and routed by session id. Mount it once per
 *  window. */
export function useEngineRuns() {
  const [runs, dispatch] = useReducer(runsReducer, initialRuns);
  /** engine_start_mic in flight: a file start waits (see `canStart`). */
  const [micStarting, setMicStarting] = useState(false);
  /** engine_start_link in flight: a file start waits too (#123). */
  const [linkStarting, setLinkStarting] = useState(false);
  /** engine_start_system in flight (#139): a file start waits too. */
  const [systemStarting, setSystemStarting] = useState(false);
  const mounted = useRef(true);

  useEffect(() => {
    mounted.current = true;
    const subs = [
      listen<EngineDownload>("engine-download", (e) => dispatch({ type: "download", payload: e.payload })),
      listen<EngineStarted>("engine-started", (e) => dispatch({ type: "engine-started", payload: e.payload })),
      listen<EngineProgress>("engine-progress", (e) => dispatch({ type: "progress", payload: e.payload })),
      listen<EngineSegmentEvent>("engine-segment", (e) => dispatch({ type: "segment", payload: e.payload })),
      listen<EngineWarning>("engine-warning", (e) => dispatch({ type: "warning", payload: e.payload })),
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
      dispatch({ type: "started", kind: "mic", sessionId: id, label: title.trim(), now: Date.now(), itemType });
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

  /** Start a System audio + mic session (#139): returns null once it
   *  started, else the reason it could not. */
  const startSystem = useCallback(async (devices: SystemDevices, title: string, options: RunArgs = NO_OPTIONS) => {
    setSystemStarting(true);
    try {
      const id = await invoke<number>("engine_start_system", {
        systemDevice: devices.system,
        micDevice: devices.mic,
        title: title.trim() || null,
        language: options.language,
        cleanupLevel: options.cleanupLevel,
        saveAudio: options.saveAudio,
      });
      dispatch({ type: "started", kind: "system", sessionId: id, label: title.trim(), now: Date.now(), itemType: "meeting" });
      return null;
    } catch (e) {
      return String(e);
    } finally {
      setSystemStarting(false);
    }
  }, []);

  const stopSystem = useCallback(async () => {
    dispatch({ type: "stopping", kind: "system" });
    try {
      await invoke<number>("engine_stop_system");
      return null;
    } catch (e) {
      dispatch({ type: "failed", kind: "system", error: String(e) });
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

  /** Transcribe a link (#123): returns null once the run started, else the
   *  reason it could not (invalid link, yt-dlp missing, archive). */
  const startLink = useCallback(
    async (url: string, title: string, label: string, allowLocal: boolean, options: RunArgs = NO_OPTIONS) => {
      setLinkStarting(true);
      try {
        const id = await invoke<number>("engine_start_link", {
          url: url.trim(),
          title: title.trim() || null,
          allowLocal,
          ...options,
        });
        dispatch({ type: "started", kind: "link", sessionId: id, label, now: Date.now() });
        return null;
      } catch (e) {
        return String(e);
      } finally {
        setLinkStarting(false);
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
    startLink,
    startSystem,
    stopSystem,
    cancel,
    dismiss,
    canStartMic: canStart(runs, "mic") && !micStarting && !systemStarting,
    canStartFile: canStart(runs, "file") && !micStarting && !linkStarting && !systemStarting,
    canStartLink: canStart(runs, "link") && !linkStarting,
    canStartSystem: canStart(runs, "system") && !systemStarting && !micStarting,
    micStarting,
    linkStarting,
    systemStarting,
  };
}

export type EngineRuns = ReturnType<typeof useEngineRuns>;
