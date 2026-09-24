import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { getVersion } from "@tauri-apps/api/app";
import { listen } from "@tauri-apps/api/event";
import { check } from "@tauri-apps/plugin-updater";
import { relaunch } from "@tauri-apps/plugin-process";
import { cleanupProfile, cleanupServerChanged, mergeKeyStorage, modelListed, patchCleanupProfile } from "../lib/llmProfiles";
import type {
  HistoryEntry,
  OllamaStatus,
  Permissions,
  Settings,
  UsageStats,
} from "../lib/types";

/** App-wide state and actions for the workspace: settings, dictation
 *  status, models, permissions and the transient status message. Every
 *  screen (and the onboarding) renders from this one controller, so there is
 *  a single source of truth for every setting. */
export function useAppController() {
  const [settings, setSettings] = useState<Settings | null>(null);
  const [history, setHistory] = useState<HistoryEntry[]>([]);
  const [status, setStatus] = useState("idle");
  const [modelReady, setModelReady] = useState(true);
  const [downloadingModel, setDownloadingModel] = useState(false);
  const [pullingModel, setPullingModel] = useState(false);
  const [busy, setBusy] = useState("");
  /** Models on the cleanup profile's server; null = unreachable → free-text fallback */
  const [ollamaModels, setOllamaModels] = useState<string[] | null>(null);
  /** GGML whisper models already present in the models folder (reuse). */
  const [installedWhisper, setInstalledWhisper] = useState<string[]>([]);
  /** This build ships the llama-server sidecar Qwen3-ASR needs (#117). */
  const [sidecarAvailable, setSidecarAvailable] = useState(false);
  /** Info shown when an installed Ollama model was auto-selected for cleanup. */
  const [modelAdoptNote, setModelAdoptNote] = useState<string | null>(null);
  const [inputDevices, setInputDevices] = useState<string[]>([]);
  const [defaultPrompts, setDefaultPrompts] = useState<string[]>(["", "", ""]);
  const [ollamaStatus, setOllamaStatus] = useState<OllamaStatus | null>(null);
  const [permissions, setPermissions] = useState<Permissions | null>(null);
  const [micTest, setMicTest] = useState(false);
  const [micLevel, setMicLevel] = useState(0);
  const [stats, setStats] = useState<UsageStats | null>(null);
  const [version, setVersion] = useState("");

  useEffect(() => {
    getVersion().then(setVersion).catch(() => setVersion(""));
  }, []);

  const checkOllama = async () => {
    try {
      setOllamaStatus(await invoke<OllamaStatus>("ollama_status"));
    } catch {
      setOllamaStatus(null);
    }
  };

  const checkPermissions = async () => {
    try {
      setPermissions(await invoke<Permissions>("check_permissions"));
    } catch {
      setPermissions(null);
    }
  };

  const loadOllamaModels = async () => {
    try {
      const models = await invoke<string[]>("list_ollama_models");
      setOllamaModels(models.length > 0 ? models : null);
    } catch {
      setOllamaModels(null);
    }
  };

  const loadWhisperModels = async () => {
    try {
      setInstalledWhisper(await invoke<string[]>("list_whisper_models"));
    } catch {
      setInstalledWhisper([]);
    }
  };

  const refresh = async () => {
    setSettings(await invoke<Settings>("get_settings"));
    setHistory(await invoke<HistoryEntry[]>("get_history", { n: 20 }));
    setModelReady(await invoke<boolean>("model_is_downloaded"));
    loadWhisperModels();
    invoke<UsageStats>("usage_stats").then(setStats).catch(() => {});
  };

  useEffect(() => {
    refresh();
    loadOllamaModels();
    invoke<string[]>("list_input_devices").then(setInputDevices).catch(() => {});
    invoke<string[]>("get_default_prompts").then(setDefaultPrompts).catch(() => {});
    invoke<boolean>("stt_sidecar_available").then(setSidecarAvailable).catch(() => setSidecarAvailable(false));
    checkOllama();
    checkPermissions();
    const unlisten = listen<string>("pipeline-status", (e) => {
      setStatus(e.payload);
      if (e.payload === "idle") refresh();
    });
    return () => {
      unlisten.then((f) => f());
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // Resolves false (after showing the error) when the backend rejects the save.
  const save = async (next: Settings): Promise<boolean> => {
    const serverChanged = cleanupServerChanged(settings, next);
    setSettings(next);
    // The old server's model list must not be adopted into the new profile.
    if (serverChanged) setOllamaModels(null);
    try {
      const saved = await invoke<Settings | null>("set_settings", { settings: next });
      // Where each API key ended up: OS credential store or file fallback (#159).
      if (saved) setSettings((cur) => (cur ? mergeKeyStorage(cur, saved) : cur));
      setBusy("");
      setModelReady(await invoke<boolean>("model_is_downloaded"));
      loadWhisperModels();
      if (serverChanged) {
        loadOllamaModels();
        checkOllama();
      }
      return true;
    } catch (e) {
      setBusy(String(e));
      return false;
    }
  };

  // If Ollama is running with models but the configured cleanup model isn't one
  // of them, adopt an installed model instead of forcing a specific download —
  // and tell the user what was picked (Matteo's onboarding feedback).
  // The model goes into the cleanup profile (#119).
  const cleanup = settings ? cleanupProfile(settings) : null;
  useEffect(() => {
    const s = settings;
    const profile = s ? cleanupProfile(s) : null;
    if (!s || !profile || profile.api !== "ollama") return;
    if (!ollamaModels || ollamaModels.length === 0) return;
    if (modelListed(ollamaModels, profile.model)) return;
    const pick =
      ollamaModels.find((m) => /llama3\.2|qwen2\.5|gemma2|phi3|mistral|instruct/i.test(m)) ||
      ollamaModels[0];
    setModelAdoptNote(
      `Found ${ollamaModels.length} model${ollamaModels.length > 1 ? "s" : ""} on Ollama — selected “${pick}” for cleanup in the “${profile.name}” profile. Change it anytime.`
    );
    save(patchCleanupProfile(s, { model: pick }));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [ollamaModels, cleanup?.id, cleanup?.api, cleanup?.model]);

  const recordingNow = status.startsWith("recording");

  // Feed the VU meter while a mic test or a real recording is running.
  useEffect(() => {
    if (!micTest && !recordingNow) {
      setMicLevel(0);
      return;
    }
    // Dictation takes over the recorder: the backend already stopped the test.
    if (micTest && recordingNow) setMicTest(false);
    const id = setInterval(() => {
      invoke<number>("mic_level").then(setMicLevel).catch(() => {});
    }, 120);
    return () => clearInterval(id);
  }, [micTest, recordingNow]);

  // Don't leave the mic open if the window unmounts mid-test.
  useEffect(() => () => {
    invoke("stop_mic_test").catch(() => {});
  }, []);

  const toggleMicTest = async () => {
    try {
      if (micTest) {
        await invoke("stop_mic_test");
        setMicTest(false);
      } else {
        await invoke("start_mic_test");
        setMicTest(true);
      }
    } catch (e) {
      setBusy(String(e));
    }
  };

  // RMS → dB, mapped so -60 dB (silence) = 0% and 0 dB (clipping) = 100%.
  const vuPct = micLevel > 0
    ? Math.max(0, Math.min(100, ((20 * Math.log10(micLevel) + 60) / 60) * 100))
    : 0;

  const downloadModel = async () => {
    setDownloadingModel(true);
    setBusy("Downloading model — this can take a while…");
    try {
      await invoke("download_model");
      setBusy("");
      setModelReady(true);
    } catch (e) {
      setBusy(String(e));
    } finally {
      setDownloadingModel(false);
    }
  };

  const pullOllamaModel = async () => {
    if (!settings) return;
    setPullingModel(true);
    setBusy(`Pulling ${cleanupProfile(settings)?.model ?? "the model"}… (can take minutes)`);
    try {
      await invoke("pull_ollama_model");
      setBusy("");
      checkOllama();
      loadOllamaModels();
    } catch (e) {
      setBusy(String(e));
    } finally {
      setPullingModel(false);
    }
  };

  /** Show a message for a few seconds, then clear it. */
  const flash = (msg: string, ms = 4000) => {
    setBusy(msg);
    setTimeout(() => setBusy(""), ms);
  };

  const copyDiagnostics = async () => {
    try {
      const report = await invoke<string>("diagnostics");
      await invoke("copy_text", { text: report });
      flash("Diagnostics copied to clipboard.", 3000);
    } catch (e) {
      setBusy(String(e));
    }
  };

  const checkForUpdates = async () => {
    setBusy("Checking for updates…");
    try {
      const update = await check();
      if (update) {
        setBusy(`Updating to ${update.version}…`);
        await update.downloadAndInstall();
        await relaunch();
      } else {
        flash("You're on the latest version.", 3000);
      }
    } catch (e) {
      setBusy(`Update check failed: ${e}`);
    }
  };

  const state = status.split(":")[0]; // idle | recording | processing | error

  return {
    settings,
    setSettings,
    save,
    refresh,
    history,
    setHistory,
    stats,
    status,
    state,
    recordingNow,
    modelReady,
    downloadingModel,
    downloadModel,
    pullingModel,
    pullOllamaModel,
    busy,
    setBusy,
    flash,
    ollamaModels,
    loadOllamaModels,
    installedWhisper,
    sidecarAvailable,
    modelAdoptNote,
    inputDevices,
    defaultPrompts,
    ollamaStatus,
    checkOllama,
    permissions,
    checkPermissions,
    micTest,
    toggleMicTest,
    vuPct,
    version,
    copyDiagnostics,
    checkForUpdates,
  };
}

export type AppController = ReturnType<typeof useAppController>;

/** The controller once settings have loaded — what every card renders from. */
export type Ctl = AppController & { settings: Settings };
