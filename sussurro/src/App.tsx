import { useEffect, useRef, useState, type ReactNode } from "react";
import { invoke } from "@tauri-apps/api/core";
import { getVersion } from "@tauri-apps/api/app";
import { listen } from "@tauri-apps/api/event";
import { check } from "@tauri-apps/plugin-updater";
import { relaunch } from "@tauri-apps/plugin-process";
import { open as openDialog, save as saveDialog } from "@tauri-apps/plugin-dialog";
import { openUrl } from "@tauri-apps/plugin-opener";
import "./App.css";

interface OllamaStatus {
  installed: boolean;
  running: boolean;
  has_model: boolean;
}

type CleanupLevel = "none" | "light" | "medium" | "high";

interface Snippet {
  cue: string;
  text: string;
}

interface AppStyle {
  app_match: string;
  style: string;
  /** Per-app output language (ISO code); "" = follow the global setting. */
  language: string;
}

interface Settings {
  hotkey: string;
  push_to_talk: boolean;
  whisper_model: string;
  engine: "whisper" | "parakeet";
  ollama_url: string;
  ollama_model: string;
  cleanup_level: CleanupLevel;
  output_language: string;
  dictionary: string[];
  autostart: boolean;
  sound_feedback: boolean;
  language: string;
  snippets: Snippet[];
  live_preview: boolean;
  app_styles: AppStyle[];
  models_dir: string;
  input_device: string;
  command_hotkey: string;
  whisper_mode: boolean;
  stream_injection: boolean;
  voice_commands: boolean;
  prompt_overrides: { light: string; medium: string; high: string };
  history_retention_days: number;
  api_enabled: boolean;
  api_port: number;
  /** Dictate-to-file: append dictations to this file instead of pasting. */
  output_file: string;
}

const LANGUAGES: [string, string][] = [
  ["auto", "Auto-detect"],
  ["it", "Italiano"],
  ["en", "English"],
  ["es", "Español"],
  ["fr", "Français"],
  ["de", "Deutsch"],
  ["pt", "Português"],
  ["nl", "Nederlands"],
  ["ja", "日本語"],
  ["zh", "中文"],
];

interface HistoryEntry {
  timestamp: string;
  raw: string;
  cleaned: string;
}

interface UsageStats {
  total_dictations: number;
  total_words: number;
  today_dictations: number;
  today_words: number;
  week_dictations: number;
  week_words: number;
}

/** 12 → "12", 1234 → "1,234", 15200 → "15.2k" */
function fmtCount(n: number): string {
  if (n >= 10_000) return `${(n / 1000).toFixed(1)}k`;
  return n.toLocaleString("en-US");
}

const MODELS = [
  { file: "ggml-base.en.bin", label: "Base · English · 148 MB · fastest" },
  { file: "ggml-small.bin", label: "Small · multilingual · 488 MB" },
  { file: "ggml-medium.bin", label: "Medium · multilingual · 1.5 GB" },
  { file: "ggml-large-v3-turbo-q5_0.bin", label: "Large v3 Turbo · multilingual · 574 MB · best" },
];

const CLEANUP_LEVELS: { value: CleanupLevel; label: string; hint: string }[] = [
  { value: "none", label: "None", hint: "raw transcript" },
  { value: "light", label: "Light", hint: "fillers + grammar" },
  { value: "medium", label: "Medium", hint: "clarity" },
  { value: "high", label: "High", hint: "rewrite" },
];

/* ---------- Info tooltip ---------- */

function Tip({ text }: { text: string }) {
  return (
    <span className="tip" tabIndex={0} role="note" aria-label={text}>
      ?
      <span className="tip-bubble">{text}</span>
    </span>
  );
}

/* ---------- Collapsible section ---------- */

function CollapsibleCard({
  storageKey,
  title,
  className = "card",
  headerExtra,
  defaultOpen = false,
  children,
}: {
  storageKey: string;
  title: ReactNode;
  className?: string;
  headerExtra?: ReactNode;
  defaultOpen?: boolean;
  children: ReactNode;
}) {
  // First run: only the cards marked defaultOpen are expanded. Afterwards the
  // user's own open/closed choice (localStorage) always wins.
  const [open, setOpen] = useState(() => {
    const stored = localStorage.getItem(storageKey);
    return stored === null ? defaultOpen : stored === "1";
  });
  return (
    <details
      className={`collapsible ${className}`}
      open={open}
      onToggle={(e) => {
        const o = (e.target as HTMLDetailsElement).open;
        setOpen(o);
        localStorage.setItem(storageKey, o ? "1" : "0");
      }}
    >
      <summary>
        <h2>{title}</h2>
        <span className="summary-right">
          {headerExtra}
          <span className="chevron" aria-hidden="true">▾</span>
        </span>
      </summary>
      {children}
    </details>
  );
}

/** Visually de-emphasized group for rarely-touched settings. */
function AdvancedGroup({ children }: { children: ReactNode }) {
  return (
    <div className="advanced-group">
      <span className="advanced-label">Advanced</span>
      {children}
    </div>
  );
}

/* ---------- About dialog ---------- */

interface LicensePackage {
  name: string;
  version: string;
  license: string;
  repository: string;
  ecosystem: "rust" | "npm";
  textId: number;
}
interface LicensesData {
  packages: LicensePackage[];
  texts: string[];
}

function AboutDialog({
  version,
  onClose,
}: {
  version: string;
  onClose: () => void;
}) {
  const [data, setData] = useState<LicensesData | null>(null);
  const [error, setError] = useState("");
  const [query, setQuery] = useState("");
  const [expanded, setExpanded] = useState<string | null>(null);

  // Load the (large) license list lazily — it's a static asset, not bundled.
  useEffect(() => {
    fetch("/licenses.json")
      .then((r) => (r.ok ? r.json() : Promise.reject(r.status)))
      .then(setData)
      .catch((e) => setError(`Couldn't load licenses (${e})`));
  }, []);

  // Close on Escape.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && onClose();
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);

  const q = query.trim().toLowerCase();
  const packages = (data?.packages ?? []).filter(
    (p) =>
      !q ||
      p.name.toLowerCase().includes(q) ||
      p.license.toLowerCase().includes(q),
  );
  const rustCount = data?.packages.filter((p) => p.ecosystem === "rust").length ?? 0;
  const npmCount = (data?.packages.length ?? 0) - rustCount;

  return (
    <div
      className="modal-backdrop"
      onClick={onClose}
      role="presentation"
    >
      <div
        className="modal about-modal"
        role="dialog"
        aria-modal="true"
        aria-label="About Sussurro"
        onClick={(e) => e.stopPropagation()}
      >
        <div className="modal-head">
          <div className="brand">
            <span className="daruma idle" aria-hidden="true" />
            <h2>Sussurro {version}</h2>
          </div>
          <button
            type="button"
            className="btn-ghost modal-close"
            onClick={onClose}
            aria-label="Close"
          >
            ×
          </button>
        </div>

        <p className="about-tagline">
          Fully-local voice dictation. Your voice never leaves this machine.
        </p>
        <p className="about-credit">
          Made with a daruma's patience by{" "}
          <a
            href="https://darumahq.it"
            onClick={(e) => {
              e.preventDefault();
              openUrl("https://darumahq.it/");
            }}
          >
            DarumaHQ.it
          </a>
          {" · "}
          <a
            href="https://github.com/fullo/Sussurro"
            onClick={(e) => {
              e.preventDefault();
              openUrl("https://github.com/fullo/Sussurro");
            }}
          >
            Source on GitHub
          </a>
        </p>

        <div className="about-licenses">
          <div className="about-licenses-head">
            <h3>Third-party licenses</h3>
            {data && (
              <span className="license-count">
                {rustCount} Rust · {npmCount} npm
              </span>
            )}
          </div>

          {error && <p className="busy" role="alert">{error}</p>}
          {!data && !error && <p className="muted">Loading licenses…</p>}

          {data && (
            <>
              <input
                type="search"
                className="license-search"
                placeholder="Filter by name or license…"
                value={query}
                onChange={(e) => setQuery(e.target.value)}
              />
              <ul className="license-list">
                {packages.map((p) => {
                  const key = `${p.ecosystem}:${p.name}@${p.version}`;
                  const isOpen = expanded === key;
                  const text = p.textId >= 0 ? data.texts[p.textId] : "";
                  return (
                    <li key={key} className="license-item">
                      <button
                        type="button"
                        className="license-row"
                        aria-expanded={isOpen}
                        onClick={() => setExpanded(isOpen ? null : key)}
                      >
                        <span className="license-name">
                          {p.name}
                          <span className="license-ver"> {p.version}</span>
                        </span>
                        <span className="license-id">{p.license}</span>
                      </button>
                      {isOpen && (
                        <div className="license-detail">
                          {p.repository && (
                            <a
                              href={p.repository}
                              onClick={(e) => {
                                e.preventDefault();
                                openUrl(p.repository);
                              }}
                            >
                              {p.repository}
                            </a>
                          )}
                          <pre>{text || "No license text bundled — see the SPDX identifier above."}</pre>
                        </div>
                      )}
                    </li>
                  );
                })}
                {packages.length === 0 && (
                  <li className="muted">No packages match “{query}”.</li>
                )}
              </ul>
            </>
          )}
        </div>
      </div>
    </div>
  );
}

/* ---------- Hotkey recorder widget ---------- */

/** Map a KeyboardEvent to the tauri-plugin-global-shortcut string, or null if incomplete. */
function comboFromEvent(e: React.KeyboardEvent): string | null {
  const code = e.code;
  // Ignore presses of bare modifier keys — wait for the main key.
  if (/^(Control|Shift|Alt|Meta)(Left|Right)?$/.test(code)) return null;

  let key: string | null = null;
  if (/^Key[A-Z]$/.test(code)) key = code.slice(3);
  else if (/^Digit[0-9]$/.test(code)) key = code.slice(5);
  else if (/^F([1-9]|1[0-9]|2[0-4])$/.test(code)) key = code;
  else if (code === "Space") key = "Space";
  else if (/^Arrow(Up|Down|Left|Right)$/.test(code)) key = code.slice(5);
  else if (
    ["Comma", "Period", "Slash", "Semicolon", "Quote", "Minus", "Equal",
     "Backquote", "BracketLeft", "BracketRight", "Backslash", "Home", "End",
     "PageUp", "PageDown", "Insert", "Enter", "Tab"].includes(code)
  ) key = code;
  if (!key) return null;

  const mods: string[] = [];
  if (e.ctrlKey || e.metaKey) mods.push("CommandOrControl");
  if (e.altKey) mods.push("Alt");
  if (e.shiftKey) mods.push("Shift");

  // A bare letter/digit as a global hotkey would hijack normal typing.
  const isFKey = /^F\d+$/.test(key);
  if (mods.length === 0 && !isFKey) return null;

  return [...mods, key].join("+");
}

function HotkeyRecorder({
  value,
  onChange,
}: {
  value: string;
  onChange: (combo: string) => void;
}) {
  const [capturing, setCapturing] = useState(false);
  const btnRef = useRef<HTMLButtonElement>(null);

  const parts = value.split("+").map((p) =>
    p === "CommandOrControl" ? (navigator.platform.includes("Mac") ? "⌘" : "Ctrl") : p
  );

  const onKeyDown = (e: React.KeyboardEvent) => {
    if (!capturing) return;
    e.preventDefault();
    e.stopPropagation();
    if (e.code === "Escape") {
      setCapturing(false);
      return;
    }
    const combo = comboFromEvent(e);
    if (combo) {
      setCapturing(false);
      onChange(combo);
      btnRef.current?.blur();
    }
  };

  return (
    <button
      ref={btnRef}
      type="button"
      className={`hotkey-recorder${capturing ? " capturing" : ""}`}
      onClick={() => setCapturing(true)}
      onKeyDown={onKeyDown}
      onBlur={() => setCapturing(false)}
      aria-label={capturing ? "Press the new shortcut, Esc to cancel" : `Shortcut: ${value}. Click to change`}
    >
      {capturing ? (
        <span className="hotkey-hint">Press keys… <em>Esc to cancel</em></span>
      ) : (
        <span className="hotkey-keys">
          {parts.map((p, i) => (
            <kbd key={i}>{p}</kbd>
          ))}
        </span>
      )}
    </button>
  );
}

/* ---------- App ---------- */

export default function App() {
  const [settings, setSettings] = useState<Settings | null>(null);
  const [history, setHistory] = useState<HistoryEntry[]>([]);
  const [status, setStatus] = useState("idle");
  const [modelReady, setModelReady] = useState(true);
  const [downloadingModel, setDownloadingModel] = useState(false);
  const [pullingModel, setPullingModel] = useState(false);
  const [busy, setBusy] = useState("");
  const [confirmClear, setConfirmClear] = useState(false);
  /** null = Ollama unreachable → free-text fallback */
  const [ollamaModels, setOllamaModels] = useState<string[] | null>(null);
  const [inputDevices, setInputDevices] = useState<string[]>([]);
  const [defaultPrompts, setDefaultPrompts] = useState<string[]>(["", "", ""]);
  const [pillHover, setPillHover] = useState(false);
  /** timestamp of the history entry being edited, and its draft text */
  const [editing, setEditing] = useState<{ ts: string; draft: string } | null>(null);
  const [historyQuery, setHistoryQuery] = useState("");
  const [searchResults, setSearchResults] = useState<HistoryEntry[] | null>(null);
  const [ollamaStatus, setOllamaStatus] = useState<OllamaStatus | null>(null);
  const [setupDismissed, setSetupDismissed] = useState(false);
  const [micTest, setMicTest] = useState(false);
  const [micLevel, setMicLevel] = useState(0);
  const [stats, setStats] = useState<UsageStats | null>(null);
  const [version, setVersion] = useState("");
  const [aboutOpen, setAboutOpen] = useState(false);

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

  const runHistorySearch = async (query: string) => {
    setHistoryQuery(query);
    if (!query.trim()) {
      setSearchResults(null);
      return;
    }
    try {
      setSearchResults(await invoke<HistoryEntry[]>("search_history", { query, n: 50 }));
    } catch {
      setSearchResults(null);
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

  const refresh = async () => {
    setSettings(await invoke<Settings>("get_settings"));
    setHistory(await invoke<HistoryEntry[]>("get_history", { n: 20 }));
    setModelReady(await invoke<boolean>("model_is_downloaded"));
    invoke<UsageStats>("usage_stats").then(setStats).catch(() => {});
  };

  useEffect(() => {
    refresh();
    loadOllamaModels();
    invoke<string[]>("list_input_devices").then(setInputDevices).catch(() => {});
    invoke<string[]>("get_default_prompts").then(setDefaultPrompts).catch(() => {});
    checkOllama();
    const unlisten = listen<string>("pipeline-status", (e) => {
      setStatus(e.payload);
      if (e.payload === "idle") refresh();
    });
    return () => {
      unlisten.then((f) => f());
    };
  }, []);

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

  if (!settings) return <main className="loading">Loading…</main>;

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

  const save = async (next: Settings) => {
    const serverChanged = next.ollama_url !== settings.ollama_url;
    setSettings(next);
    try {
      await invoke("set_settings", { settings: next });
      setBusy("");
      setModelReady(await invoke<boolean>("model_is_downloaded"));
      if (serverChanged) loadOllamaModels();
    } catch (e) {
      setBusy(String(e));
    }
  };

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

  const clearHistory = async () => {
    if (!confirmClear) {
      setConfirmClear(true);
      setTimeout(() => setConfirmClear(false), 3000);
      return;
    }
    setConfirmClear(false);
    try {
      await invoke("clear_history");
      setHistory([]);
    } catch (e) {
      setBusy(String(e));
    }
  };

  const state = status.split(":")[0]; // idle | recording | processing | error
  const pushToTalk = settings.push_to_talk;
  const pillLabel =
    state === "recording"
      ? pushToTalk ? "● Recording — release to stop" : "■ Recording — click to stop"
      : state === "processing" ? "Transcribing…"
      : state === "error" ? status
      : !modelReady ? "Model not downloaded yet"
      : pillHover ? (pushToTalk ? "● Dictate — hold" : "● Dictate") : "Ready";

  const pillDown = () => {
    if (state === "idle" || state === "recording") invoke("trigger_dictation", { pressed: true });
  };
  const pillUp = () => {
    invoke("trigger_dictation", { pressed: false });
  };

  return (
    <main>
      <header className="masthead">
        <div className="brand">
          <span className={`daruma ${state}`} aria-hidden="true" />
          <h1>Sussurro</h1>
        </div>
        <p className="tagline">Local dictation. Your voice never leaves this machine.</p>
        <button
          type="button"
          className={`status status-${state} pill-btn`}
          onMouseEnter={() => setPillHover(true)}
          onMouseLeave={() => {
            setPillHover(false);
            // Dragging off the button while holding must stop push-to-talk.
            if (state === "recording" && pushToTalk) pillUp();
          }}
          onMouseDown={pillDown}
          onMouseUp={pillUp}
          aria-label="Dictate: hold (push-to-talk) or click (toggle) to record"
        >
          {pillLabel}
        </button>
        {busy && <p className="busy" role="alert">{busy}</p>}
      </header>

      {!setupDismissed &&
        (ollamaStatus !== null &&
          (!ollamaStatus.running || !ollamaStatus.has_model || !modelReady)) && (
        <div className="setup-banner" role="alert">
          <div className="setup-head">
            <strong>Setup</strong>
            <span className="summary-right">
              <button className="btn-ghost" onClick={checkOllama}>Re-check</button>
              <button
                className="btn-ghost"
                onClick={() => setSetupDismissed(true)}
                aria-label="Dismiss setup banner"
              >
                ×
              </button>
            </span>
          </div>
          <ul>
            {!ollamaStatus.installed && (
              <li>
                <span className="setup-bad">✗</span> Ollama is not installed — cleanup
                and translation need it (dictation still works, raw only).
                <button
                  className="btn-ghost"
                  onClick={() => openUrl("https://ollama.com/download")}
                >
                  Get Ollama
                </button>
              </li>
            )}
            {ollamaStatus.installed && !ollamaStatus.running && (
              <li>
                <span className="setup-bad">✗</span> Ollama is installed but not
                running — start the Ollama app (or run <code>ollama serve</code>),
                then Re-check.
              </li>
            )}
            {ollamaStatus.running && !ollamaStatus.has_model && (
              <li>
                <span className="setup-bad">✗</span> Model “{settings.ollama_model}”
                is not on your Ollama server.
                <button
                  className="btn-ghost"
                  disabled={pullingModel}
                  onClick={async () => {
                    setPullingModel(true);
                    setBusy(`Pulling ${settings.ollama_model}… (can take minutes)`);
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
                  }}
                >
                  {pullingModel && <span className="btn-spinner" aria-hidden="true" />}
                  {pullingModel ? "Pulling…" : "Pull it"}
                </button>
              </li>
            )}
            {!modelReady && (
              <li>
                <span className="setup-bad">✗</span> Speech model not downloaded yet.
                <button className="btn-ghost" onClick={downloadModel} disabled={downloadingModel}>
                  {downloadingModel && <span className="btn-spinner" aria-hidden="true" />}
                  {downloadingModel ? "Downloading…" : "Download"}
                </button>
              </li>
            )}
          </ul>
        </div>
      )}

      <CollapsibleCard storageKey="dictationOpen" title="Dictation" defaultOpen>
        <div className="field">
          <div className="field-label">
            <span>Shortcut <Tip text="The system-wide key combination that triggers dictation in any app. Click the field, then press the keys you want (Esc cancels)." /></span>
            <small>{settings.push_to_talk ? "hold to record" : "tap to start / stop"}</small>
          </div>
          <HotkeyRecorder
            value={settings.hotkey}
            onChange={(combo) => save({ ...settings, hotkey: combo })}
          />
        </div>

        <div className="field">
          <div className="field-label">
            <span>Microphone <Tip text="Which input device to record from. Default follows the system microphone; pick a specific one if you have several (headset, webcam, desk mic). If the chosen device is unplugged, Sussurro falls back to the default." /></span>
            <small>capture device</small>
          </div>
          <div className="mic-col">
            <div className="mic-row">
              <select
                value={settings.input_device}
                onChange={(e) => save({ ...settings, input_device: e.target.value })}
              >
                <option value="">System default</option>
                {inputDevices.map((d) => (
                  <option key={d} value={d}>{d}</option>
                ))}
                {settings.input_device && !inputDevices.includes(settings.input_device) && (
                  <option value={settings.input_device}>
                    {settings.input_device} (unavailable)
                  </option>
                )}
              </select>
              <button
                className="btn-ghost"
                onClick={toggleMicTest}
                title="Listen to the input level to check the right microphone is picked up"
              >
                {micTest ? "Stop" : "Test"}
              </button>
            </div>
            {(micTest || recordingNow) && (
              <div
                className="vu"
                role="meter"
                aria-label="Input level"
                aria-valuemin={0}
                aria-valuemax={100}
                aria-valuenow={Math.round(vuPct)}
              >
                <div className="vu-fill" style={{ width: `${vuPct}%` }} />
              </div>
            )}
          </div>
        </div>

        <div className="field">
          <div className="field-label">
            <span>Command shortcut <Tip text="Command mode: select text anywhere, hold this shortcut and SPEAK AN INSTRUCTION ('make it shorter', 'translate to English', 'fix the grammar') — the LLM applies it to the selection and the result replaces it." /></span>
            <small>speak an instruction, applied to the selection</small>
          </div>
          <HotkeyRecorder
            value={settings.command_hotkey}
            onChange={(combo) => save({ ...settings, command_hotkey: combo })}
          />
        </div>

        <div className="field">
          <div className="field-label">
            <span>Push-to-talk <Tip text="On: recording lasts while you hold the shortcut or the Dictate button, like a walkie-talkie. Off: one tap/click starts recording, a second one stops it. Applies to both the keyboard shortcut and the Dictate button in the header." /></span>
            <small>off = toggle mode</small>
          </div>
          <label className="switch">
            <input
              type="checkbox"
              checked={settings.push_to_talk}
              onChange={(e) => save({ ...settings, push_to_talk: e.target.checked })}
            />
            <span className="slider" />
          </label>
        </div>
      </CollapsibleCard>

      <CollapsibleCard
        storageKey="speechOpen"
        title={<>Speech recognition <span className="via">engine & models</span></>}
      >

        <div className="field">
          <div className="field-label">
            <span>Engine <Tip text="Whisper: GPU-accelerated, any language, choose the model size below. Parakeet: NVIDIA's CPU-optimized model — roughly 10x faster than Whisper without a GPU, auto-detects 25 European languages, one fixed 456 MB model." /></span>
            <small>{settings.engine === "whisper" ? "whisper.cpp · GPU" : "Parakeet TDT v3 · CPU"}</small>
          </div>
          <div className="segmented" role="radiogroup" aria-label="STT engine">
            {(["whisper", "parakeet"] as const).map((e) => (
              <button
                key={e}
                role="radio"
                aria-checked={settings.engine === e}
                className={settings.engine === e ? "on" : ""}
                onClick={() => save({ ...settings, engine: e })}
              >
                {e === "whisper" ? "Whisper" : "Parakeet"}
              </button>
            ))}
          </div>
        </div>

        {settings.engine === "whisper" && (
          <div className="field">
            <div className="field-label">
              <span>Language <Tip text="Tell Whisper which language you dictate in. A fixed language is more accurate and slightly faster than auto-detect — especially on smaller models. Note: the English-only models ignore this." /></span>
              <small>hint for the transcriber</small>
            </div>
            <select
              value={settings.language}
              onChange={(e) => save({ ...settings, language: e.target.value })}
            >
              {LANGUAGES.map(([code, label]) => (
                <option key={code} value={code}>{label}</option>
              ))}
            </select>
          </div>
        )}

        <div className="field">
          <div className="field-label">
            <span>Model <Tip text="Whisper: bigger = more accurate but slower; 'English' variants are faster for English-only dictation. Parakeet has a single fixed model (int8, 456 MB)." /></span>
            <small>speech-to-text, fully offline</small>
          </div>
          <div className="model-row">
            {settings.engine === "whisper" ? (
              <select
                value={settings.whisper_model}
                onChange={(e) => save({ ...settings, whisper_model: e.target.value })}
              >
                {MODELS.map((m) => (
                  <option key={m.file} value={m.file}>{m.label}</option>
                ))}
              </select>
            ) : (
              <span className="fixed-model">Parakeet TDT 0.6B v3 · int8 · 456 MB</span>
            )}
            {!modelReady && (
              <button
                className="btn-primary btn-icon"
                onClick={downloadModel}
                disabled={downloadingModel}
                title={downloadingModel ? "Downloading — this can take a while…" : "Download the selected model"}
                aria-label={downloadingModel ? "Downloading model" : "Download the selected model"}
              >
                {downloadingModel ? (
                  <span className="btn-spinner" aria-hidden="true" />
                ) : (
                  <svg
                    width="18"
                    height="18"
                    viewBox="0 0 24 24"
                    fill="none"
                    stroke="currentColor"
                    strokeWidth="2.5"
                    strokeLinecap="round"
                    strokeLinejoin="round"
                    aria-hidden="true"
                  >
                    <path d="M12 3v12" />
                    <path d="m7 10 5 5 5-5" />
                    <path d="M5 21h14" />
                  </svg>
                )}
              </button>
            )}
          </div>
        </div>

        <AdvancedGroup>
          <div className="field">
            <div className="field-label">
              <span>Models folder <Tip text="Where downloaded STT models are stored (up to a few GB). Leave empty for the default app-data folder, or point it at a roomier disk. Already-downloaded models must be moved there manually." /></span>
              <small>empty = app data default</small>
            </div>
            <input
              value={settings.models_dir}
              placeholder="F:\claude\models"
              onChange={(e) => setSettings({ ...settings, models_dir: e.target.value })}
              onBlur={() => save(settings)}
              spellCheck={false}
            />
          </div>

          <div className="field">
            <div className="field-label">
              <span>Whisper mode <Tip text="For dictating quietly (open office, late night): boosts microphone gain 3x and lowers the silence gate so soft speech still registers." /></span>
              <small>for quiet speech</small>
            </div>
            <label className="switch">
              <input
                type="checkbox"
                checked={settings.whisper_mode}
                onChange={(e) => save({ ...settings, whisper_mode: e.target.checked })}
              />
              <span className="slider" />
            </label>
          </div>
        </AdvancedGroup>
      </CollapsibleCard>

      <CollapsibleCard
        storageKey="cleanupOpen"
        title={<>Cleanup <span className="via">via Ollama</span></>}
      >
        <div className="field">
          <div className="field-label">
            <span>Level <Tip text="How much the local LLM edits your transcript. None: exactly what you said, mistakes included. Light: removes 'um/uh' and fixes grammar. Medium: also tightens for clarity and conciseness. High: rewrites for brevity and polish. If Ollama is unreachable you always get the raw transcript." /></span>
            <small>{CLEANUP_LEVELS.find((l) => l.value === settings.cleanup_level)?.hint}</small>
          </div>
          <div className="segmented" role="radiogroup" aria-label="Cleanup level">
            {CLEANUP_LEVELS.map((l) => (
              <button
                key={l.value}
                role="radio"
                aria-checked={settings.cleanup_level === l.value}
                className={settings.cleanup_level === l.value ? "on" : ""}
                onClick={() => save({ ...settings, cleanup_level: l.value })}
              >
                {l.label}
              </button>
            ))}
          </div>
        </div>

        <div className="field">
          <div className="field-label">
            <span>Translate to <Tip text="Dictate in one language and get the cleaned text in another — the LLM translates while cleaning. 'Keep language' disables translation. Works even with Cleanup None (translate-only). Something Wispr Flow can't do locally." /></span>
            <small>output language</small>
          </div>
          <select
            value={settings.output_language}
            onChange={(e) => save({ ...settings, output_language: e.target.value })}
          >
            <option value="">Keep language</option>
            {LANGUAGES.filter(([code]) => code !== "auto").map(([code, label]) => (
              <option key={code} value={code}>{label}</option>
            ))}
          </select>
        </div>

        <div className="field">
          <div className="field-label"><span>Server <Tip text="Address of your Ollama server — the local service that runs the cleanup LLM. The default http://localhost:11434 is correct if Ollama runs on this machine; change it only if Ollama runs elsewhere (e.g. another PC on your network)." /></span></div>
          <input
            value={settings.ollama_url}
            onChange={(e) => setSettings({ ...settings, ollama_url: e.target.value })}
            onBlur={() => save(settings)}
            spellCheck={false}
          />
        </div>

        <div className="field">
          <div className="field-label">
            <span>LLM model <Tip text="The Ollama model that cleans up the transcript (filler removal, grammar, rewriting). Any small instruct model works — llama3.2:3b is a good default. The list shows what is installed on your Ollama server; add more with 'ollama pull'." /></span>
            {ollamaModels === null && <small>server unreachable — type the name</small>}
          </div>
          {ollamaModels ? (
            <select
              value={settings.ollama_model}
              onChange={(e) => save({ ...settings, ollama_model: e.target.value })}
            >
              {!ollamaModels.includes(settings.ollama_model) && (
                <option value={settings.ollama_model}>
                  {settings.ollama_model} (not installed)
                </option>
              )}
              {ollamaModels.map((m) => (
                <option key={m} value={m}>{m}</option>
              ))}
            </select>
          ) : (
            <input
              value={settings.ollama_model}
              onChange={(e) => setSettings({ ...settings, ollama_model: e.target.value })}
              onBlur={() => save(settings)}
              spellCheck={false}
            />
          )}
        </div>

        <AdvancedGroup>
          <div className="field field-col">
            <div className="field-label">
              <span>Cleanup prompts <Tip text="Rewrite the per-level instructions sent to the LLM. Leave a box empty to use the built-in default (shown as placeholder). Power-user territory: a bad prompt makes every dictation worse." /></span>
              <small>empty = built-in default</small>
            </div>
            {(["light", "medium", "high"] as const).map((lvl, i) => (
              <div key={lvl} className="prompt-override">
                <span className="prompt-level">{lvl}</span>
                <textarea
                  rows={2}
                  value={settings.prompt_overrides[lvl]}
                  placeholder={defaultPrompts[i]}
                  onChange={(e) =>
                    setSettings({
                      ...settings,
                      prompt_overrides: { ...settings.prompt_overrides, [lvl]: e.target.value },
                    })
                  }
                  onBlur={() => save(settings)}
                  spellCheck={false}
                />
              </div>
            ))}
          </div>
        </AdvancedGroup>
      </CollapsibleCard>

      <CollapsibleCard
        storageKey="snippetsOpen"
        title={<>Personalization <span className="via">dictionary · styles · snippets</span></>}
      >
        <div className="field field-col">
          <div className="field-label">
            <span>Personal dictionary <Tip text="Names, brands and jargon the models tend to misspell (e.g. Sussurro, Tauri). One per line. They are fed to Whisper as recognition hints and to the LLM as preferred spellings." /></span>
            <small>names & jargon, one per line — biases both Whisper and the LLM</small>
          </div>
          <textarea
            rows={3}
            value={settings.dictionary.join("\n")}
            onChange={(e) =>
              setSettings({
                ...settings,
                dictionary: e.target.value.split("\n").map((w) => w.trim()).filter(Boolean),
              })
            }
            onBlur={() => save(settings)}
            spellCheck={false}
            placeholder="Sussurro&#10;Tauri"
          />
        </div>

        <div className="field field-col">
          <div className="field-label">
            <span>App styles <Tip text="Per-application rules: when you dictate into an app whose name contains the match (e.g. 'slack'), the tone instruction is added to the cleanup prompt, and the rule's output language (if set) overrides the global 'Translate to'. Example: slack → casual + English; whatsapp → informal + Italiano." /></span>
            <small>tone and output language per app</small>
          </div>
          {settings.app_styles.map((s, i) => (
            <div className="snippet-row" key={i}>
              <input
                placeholder="app name contains…"
                value={s.app_match}
                onChange={(e) => {
                  const app_styles = settings.app_styles.slice();
                  app_styles[i] = { ...s, app_match: e.target.value };
                  setSettings({ ...settings, app_styles });
                }}
                onBlur={() => save(settings)}
                spellCheck={false}
              />
              <textarea
                placeholder="tone instruction for the LLM"
                rows={2}
                value={s.style}
                onChange={(e) => {
                  const app_styles = settings.app_styles.slice();
                  app_styles[i] = { ...s, style: e.target.value };
                  setSettings({ ...settings, app_styles });
                }}
                onBlur={() => save(settings)}
              />
              <button
                className="btn-ghost"
                onClick={() =>
                  save({ ...settings, app_styles: settings.app_styles.filter((_, j) => j !== i) })
                }
              >
                Remove
              </button>
              <select
                className="style-lang"
                title="Output language when dictating into this app — overrides the global 'Translate to'"
                value={s.language ?? ""}
                onChange={(e) => {
                  const app_styles = settings.app_styles.slice();
                  app_styles[i] = { ...s, language: e.target.value };
                  save({ ...settings, app_styles });
                }}
              >
                <option value="">Output: global language</option>
                {LANGUAGES.filter(([code]) => code !== "auto").map(([code, label]) => (
                  <option key={code} value={code}>Output: {label}</option>
                ))}
              </select>
            </div>
          ))}
          <button
            className="btn-ghost"
            onClick={() =>
              setSettings({
                ...settings,
                app_styles: [...settings.app_styles, { app_match: "", style: "", language: "" }],
              })
            }
          >
            + Add app style
          </button>
        </div>

        <div className="field field-col">
          <div className="field-label">
            <span>Snippets <Tip text="Example: cue 'firma email' → pastes your full signature. Matching ignores case and punctuation, and skips the AI cleanup entirely." /></span>
            <small>say a cue exactly — Sussurro pastes the full text instead of transcribing</small>
          </div>
          {settings.snippets.map((s, i) => (
          <div className="snippet-row" key={i}>
            <input
              placeholder="cue (what you say)"
              value={s.cue}
              onChange={(e) => {
                const snippets = settings.snippets.slice();
                snippets[i] = { ...s, cue: e.target.value };
                setSettings({ ...settings, snippets });
              }}
              onBlur={() => save(settings)}
              spellCheck={false}
            />
            <textarea
              placeholder="text to paste"
              rows={2}
              value={s.text}
              onChange={(e) => {
                const snippets = settings.snippets.slice();
                snippets[i] = { ...s, text: e.target.value };
                setSettings({ ...settings, snippets });
              }}
              onBlur={() => save(settings)}
              spellCheck={false}
            />
            <button
              className="btn-ghost"
              onClick={() =>
                save({ ...settings, snippets: settings.snippets.filter((_, j) => j !== i) })
              }
            >
              Remove
            </button>
          </div>
        ))}
          <button
            className="btn-ghost"
            onClick={() =>
              setSettings({ ...settings, snippets: [...settings.snippets, { cue: "", text: "" }] })
            }
          >
            + Add snippet
          </button>
        </div>

        <div className="field">
          <div className="field-label">
            <span>Portable config <Tip text="Export your dictionary, snippets and app styles to a JSON file, or import one — to move your setup between machines (sync it with a file/Git/Syncthing, no cloud account). Import merges without duplicates; machine-specific settings like hotkeys and models folder are not included." /></span>
            <small>dictionary + snippets + styles</small>
          </div>
          <div className="model-row">
            <button
              className="btn-ghost"
              onClick={async () => {
                const path = await saveDialog({
                  defaultPath: "sussurro-config.json",
                  filters: [{ name: "JSON", extensions: ["json"] }],
                });
                if (!path) return;
                try {
                  await invoke("export_config", { path });
                  setBusy("Config exported.");
                  setTimeout(() => setBusy(""), 3000);
                } catch (e) {
                  setBusy(String(e));
                }
              }}
            >
              Export
            </button>
            <button
              className="btn-ghost"
              onClick={async () => {
                const path = await openDialog({
                  multiple: false,
                  filters: [{ name: "JSON", extensions: ["json"] }],
                });
                if (!path || typeof path !== "string") return;
                try {
                  const msg = await invoke<string>("import_config", { path });
                  setBusy(msg);
                  refresh();
                } catch (e) {
                  setBusy(String(e));
                }
              }}
            >
              Import
            </button>
          </div>
        </div>
      </CollapsibleCard>

      <CollapsibleCard
        storageKey="behaviorOpen"
        title={<>Behavior <span className="via">feedback & extras</span></>}
      >
        <div className="field">
          <div className="field-label">
            <span>Live preview <Tip text="While you speak, the overlay shows a rolling partial transcript (re-transcribed every ~1.2s). Costs extra GPU/CPU during recording; the pasted text always comes from the final, full-quality pass." /></span>
            <small>partial transcript in the overlay</small>
          </div>
          <label className="switch">
            <input
              type="checkbox"
              checked={settings.live_preview}
              onChange={(e) => save({ ...settings, live_preview: e.target.checked })}
            />
            <span className="slider" />
          </label>
        </div>

        <div className="field">
          <div className="field-label">
            <span>Sound feedback <Tip text="A short rising tick when recording starts and a falling one when it stops — so you know the trigger worked without looking at this window." /></span>
            <small>tick on start / stop</small>
          </div>
          <label className="switch">
            <input
              type="checkbox"
              checked={settings.sound_feedback}
              onChange={(e) => save({ ...settings, sound_feedback: e.target.checked })}
            />
            <span className="slider" />
          </label>
        </div>

        <div className="field">
          <div className="field-label">
            <span>Voice commands <Tip text="Interpret spoken editing commands instead of transcribing them. Always work: 'a capo'/'new line', 'nuovo paragrafo'/'new paragraph', 'punto e a capo'/'period new line', 'punto elenco'/'new bullet' (bulleted list). With cleanup on, also 'scratch that'/'cancella quello' (deletes the previous phrase) and 'quote … end quote'." /></span>
            <small>a capo, punto elenco, scratch that…</small>
          </div>
          <label className="switch">
            <input
              type="checkbox"
              checked={settings.voice_commands}
              onChange={(e) => save({ ...settings, voice_commands: e.target.checked })}
            />
            <span className="slider" />
          </label>
        </div>

        <div className="field">
          <div className="field-label">
            <span>Streaming typing <Tip text="EXPERIMENTAL: types the text into the app WHILE you speak. With Cleanup None it streams word by word (holding back the last 2); with cleanup on it streams sentence by sentence, each one LLM-cleaned before being typed. The final pass completes the tail when you release." /></span>
            <small>experimental · word or sentence streaming</small>
          </div>
          <label className="switch">
            <input
              type="checkbox"
              checked={settings.stream_injection}
              onChange={(e) => save({ ...settings, stream_injection: e.target.checked })}
            />
            <span className="slider" />
          </label>
        </div>

        <div className="field">
          <div className="field-label">
            <span>Launch at login <Tip text="Start Sussurro automatically when you log in. It starts hidden in the tray — click the tray icon or press your shortcut to use it." /></span>
            <small>starts hidden in the tray</small>
          </div>
          <label className="switch">
            <input
              type="checkbox"
              checked={settings.autostart}
              onChange={(e) => save({ ...settings, autostart: e.target.checked })}
            />
            <span className="slider" />
          </label>
        </div>

        <div className="field">
          <div className="field-label">
            <span>Dictate to file <Tip text="Note-taking mode: every dictation is APPENDED to this file (e.g. an Obsidian note) instead of being pasted into the focused app. Clear the path to go back to normal pasting. Streaming typing is suspended while this is active." /></span>
            <small>{settings.output_file.trim() ? "active — nothing is pasted" : "off — text is pasted normally"}</small>
          </div>
          <div className="model-row">
            {settings.output_file.trim() ? (
              <>
                <span className="output-file-path" title={settings.output_file}>
                  {settings.output_file}
                </span>
                <button
                  className="btn-ghost"
                  onClick={() => save({ ...settings, output_file: "" })}
                >
                  Stop
                </button>
              </>
            ) : (
              <button
                className="btn-ghost"
                onClick={async () => {
                  const path = await saveDialog({
                    defaultPath: "sussurro-notes.md",
                    filters: [
                      { name: "Markdown", extensions: ["md"] },
                      { name: "Text", extensions: ["txt"] },
                    ],
                  });
                  if (path) save({ ...settings, output_file: path });
                }}
              >
                Choose file…
              </button>
            )}
          </div>
        </div>

        <AdvancedGroup>
          <div className="field">
            <div className="field-label">
              <span>Local API <Tip text="HTTP API on 127.0.0.1 for scripting: POST /clean (text → cleaned), POST /transcribe?ext=wav (audio file → transcript), GET /history?q=. Loopback only — but any local process can call it. Applied at app restart. curl examples in the README." /></span>
              <small>for scripts & automation · restart required</small>
            </div>
            <div className="model-row">
              <label className="switch">
                <input
                  type="checkbox"
                  checked={settings.api_enabled}
                  onChange={(e) => save({ ...settings, api_enabled: e.target.checked })}
                />
                <span className="slider" />
              </label>
              <input
                className="port-input"
                type="number"
                min={1024}
                max={65535}
                value={settings.api_port}
                onChange={(e) => setSettings({ ...settings, api_port: Number(e.target.value) })}
                onBlur={() => save(settings)}
                disabled={!settings.api_enabled}
                title="Port"
              />
            </div>
          </div>
        </AdvancedGroup>
      </CollapsibleCard>


      <CollapsibleCard
        storageKey="historyOpen"
        className="card history"
        title="History"
        headerExtra={
          history.length > 0 ? (
            <>
              <button
                className="btn-ghost"
                title="Export the whole history to Markdown or JSON"
                onClick={async (e) => {
                  // Inside <summary>: don't let the click toggle the accordion.
                  e.preventDefault();
                  e.stopPropagation();
                  const path = await saveDialog({
                    defaultPath: "sussurro-history.md",
                    filters: [
                      { name: "Markdown", extensions: ["md"] },
                      { name: "JSON", extensions: ["json"] },
                    ],
                  });
                  if (!path) return;
                  try {
                    const msg = await invoke<string>("export_history", { path });
                    setBusy(msg);
                    setTimeout(() => setBusy(""), 3000);
                  } catch (err) {
                    setBusy(String(err));
                  }
                }}
              >
                Export
              </button>
              <button
                className={`btn-ghost${confirmClear ? " danger" : ""}`}
                onClick={(e) => {
                  e.preventDefault();
                  e.stopPropagation();
                  clearHistory();
                }}
              >
                {confirmClear ? "Click again to delete" : "Clear"}
              </button>
            </>
          ) : undefined
        }
      >
        {stats && stats.total_dictations > 0 && (
          <p
            className="stats-row"
            title="Counted since install — pruning or clearing the history doesn't reset these numbers"
          >
            <strong>{fmtCount(stats.total_dictations)}</strong> dictations ·{" "}
            <strong>{fmtCount(stats.total_words)}</strong> words
            {" — today "}
            <strong>{fmtCount(stats.today_words)}</strong>
            {" · last 7 days "}
            <strong>{fmtCount(stats.week_words)}</strong>
          </p>
        )}
        <div className="field">
          <div className="field-label">
            <span>Search & retention <Tip text="Search the WHOLE history (raw and cleaned text, case-insensitive). Retention auto-deletes entries older than the chosen window — a privacy tool: what you dictated last month doesn't need to live on disk forever." /></span>
            <small>{searchResults !== null ? `${searchResults.length} matches` : "full-text, whole history"}</small>
          </div>
          <div className="model-row">
            <input
              type="search"
              placeholder="search dictations…"
              value={historyQuery}
              onChange={(e) => runHistorySearch(e.target.value)}
              spellCheck={false}
            />
            <select
              value={settings.history_retention_days}
              onChange={(e) =>
                save({ ...settings, history_retention_days: Number(e.target.value) })
              }
              title="Auto-delete entries older than"
            >
              <option value={0}>Keep forever</option>
              <option value={7}>7 days</option>
              <option value={30}>30 days</option>
              <option value={90}>90 days</option>
            </select>
          </div>
        </div>
        {history.length === 0 && searchResults === null && (
          <p className="empty">Nothing yet. Hold the shortcut and speak.</p>
        )}
        {searchResults !== null && searchResults.length === 0 && (
          <p className="empty">No matches for “{historyQuery}”.</p>
        )}
        <ol>
          {(searchResults ?? history).map((h) => (
            <li key={h.timestamp}>
              <div className="entry-head">
                <time>{new Date(h.timestamp).toLocaleTimeString()}</time>
                <span className="entry-actions">
                  <button
                    className="btn-ghost"
                    onClick={async () => {
                      await invoke("copy_text", { text: h.cleaned });
                    }}
                  >
                    Copy
                  </button>
                  <button
                    className="btn-ghost"
                    title="Clean the raw transcript again with the current level"
                    onClick={async () => {
                      setBusy("Re-cleaning…");
                      try {
                        await invoke("reclean", { raw: h.raw });
                        setBusy("");
                        refresh();
                      } catch (e) {
                        setBusy(String(e));
                      }
                    }}
                  >
                    Re-clean
                  </button>
                  <select
                    className="btn-ghost translate-select"
                    value=""
                    title="Translate this entry into another language (adds a new entry)"
                    onChange={async (e) => {
                      const lang = e.target.value;
                      e.target.value = "";
                      if (!lang) return;
                      setBusy("Translating…");
                      try {
                        await invoke("translate_entry", { raw: h.raw, lang });
                        setBusy("");
                        refresh();
                      } catch (err) {
                        setBusy(String(err));
                      }
                    }}
                  >
                    <option value="" disabled>
                      Translate…
                    </option>
                    {LANGUAGES.filter(([code]) => code !== "auto").map(([code, label]) => (
                      <option key={code} value={code}>{label}</option>
                    ))}
                  </select>
                  <button
                    className="btn-ghost"
                    title="Correct the text — new words are added to your dictionary"
                    onClick={() => setEditing({ ts: h.timestamp, draft: h.cleaned })}
                  >
                    Edit
                  </button>
                </span>
              </div>
              {editing?.ts === h.timestamp ? (
                <div className="edit-box">
                  <textarea
                    rows={3}
                    value={editing.draft}
                    onChange={(e) => setEditing({ ts: h.timestamp, draft: e.target.value })}
                    autoFocus
                  />
                  <div className="edit-actions">
                    <button
                      className="btn-primary"
                      onClick={async () => {
                        try {
                          const learned = await invoke<string[]>("learn_correction", {
                            raw: h.raw,
                            original: h.cleaned,
                            corrected: editing.draft,
                          });
                          setEditing(null);
                          setBusy(
                            learned.length > 0
                              ? `Learned: ${learned.join(", ")} → added to your dictionary`
                              : ""
                          );
                          refresh();
                        } catch (e) {
                          setBusy(String(e));
                        }
                      }}
                    >
                      Save correction
                    </button>
                    <button className="btn-ghost" onClick={() => setEditing(null)}>
                      Cancel
                    </button>
                  </div>
                </div>
              ) : (
                <p className="cleaned">{h.cleaned}</p>
              )}
              {h.cleaned !== h.raw && <p className="raw">{h.raw}</p>}
            </li>
          ))}
        </ol>
      </CollapsibleCard>

      <CollapsibleCard
        storageKey="fileOpen"
        title={<>Audio file <span className="via">transcribe a recording</span></>}
      >
        <p className="card-hint">
          Transcribe a .wav / .mp3 / .m4a file with the current engine and cleanup — no dictation needed.
          <Tip text="Something Wispr Flow doesn't do: it's dictation-only. The result is cleaned with your current settings and added to History (not injected anywhere)." />
        </p>
        <input
          type="file"
          accept="audio/*,.wav,.mp3,.m4a,.aac,.flac,.ogg"
          onChange={async (e) => {
            const file = e.target.files?.[0];
            e.target.value = "";
            if (!file) return;
            setBusy(`Transcribing ${file.name}…`);
            try {
              const bytes = Array.from(new Uint8Array(await file.arrayBuffer()));
              const ext = file.name.split(".").pop() ?? "";
              await invoke<HistoryEntry>("transcribe_audio_file", { bytes, ext });
              setBusy("");
              refresh();
            } catch (err) {
              setBusy(String(err));
            }
          }}
        />
      </CollapsibleCard>

      <footer>
        <div className="footer-top">
        <span>すべてローカル — everything stays local</span>
        <span className="footer-actions">
          <button
            className="btn-ghost"
            title="Copy version, OS and configuration to the clipboard — paste it into a bug report (no personal content included)"
            onClick={async () => {
              try {
                const report = await invoke<string>("diagnostics");
                await invoke("copy_text", { text: report });
                setBusy("Diagnostics copied to clipboard.");
                setTimeout(() => setBusy(""), 3000);
              } catch (e) {
                setBusy(String(e));
              }
            }}
          >
            Copy diagnostics
          </button>
          <button
            className="btn-ghost"
            onClick={async () => {
              setBusy("Checking for updates…");
              try {
                const update = await check();
                if (update) {
                  setBusy(`Updating to ${update.version}…`);
                  await update.downloadAndInstall();
                  await relaunch();
                } else {
                  setBusy("You're on the latest version.");
                  setTimeout(() => setBusy(""), 3000);
                }
              } catch (e) {
                setBusy(`Update check failed: ${e}`);
              }
            }}
          >
            Check for updates
          </button>
        </span>
        </div>
        <div className="footer-credit">
          <span className="daruma-mini" aria-hidden="true" />
          Sussurro {version && `${version} `}by{" "}
          <a
            href="https://darumahq.it"
            onClick={(e) => {
              e.preventDefault();
              openUrl("https://darumahq.it/");
            }}
          >
            DarumaHQ.it
          </a>
          {" · "}
          <button
            type="button"
            className="link-btn"
            onClick={() => setAboutOpen(true)}
          >
            About
          </button>
        </div>
      </footer>

      {aboutOpen && (
        <AboutDialog version={version} onClose={() => setAboutOpen(false)} />
      )}
    </main>
  );
}
