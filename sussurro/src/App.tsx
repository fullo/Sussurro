import { useEffect, useRef, useState, type ReactNode } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import "./App.css";

type CleanupLevel = "none" | "light" | "medium" | "high";

interface Settings {
  hotkey: string;
  push_to_talk: boolean;
  whisper_model: string;
  ollama_url: string;
  ollama_model: string;
  cleanup_level: CleanupLevel;
  dictionary: string[];
  autostart: boolean;
}

interface HistoryEntry {
  timestamp: string;
  raw: string;
  cleaned: string;
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
  children,
}: {
  storageKey: string;
  title: ReactNode;
  className?: string;
  headerExtra?: ReactNode;
  children: ReactNode;
}) {
  const [open, setOpen] = useState(() => localStorage.getItem(storageKey) !== "0");
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
  const [busy, setBusy] = useState("");
  const [confirmClear, setConfirmClear] = useState(false);
  /** null = Ollama unreachable → free-text fallback */
  const [ollamaModels, setOllamaModels] = useState<string[] | null>(null);

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
  };

  useEffect(() => {
    refresh();
    loadOllamaModels();
    const unlisten = listen<string>("pipeline-status", (e) => {
      setStatus(e.payload);
      if (e.payload === "idle") refresh();
    });
    return () => {
      unlisten.then((f) => f());
    };
  }, []);

  if (!settings) return <main className="loading">Loading…</main>;

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
    setBusy("Downloading model — this can take a while…");
    try {
      await invoke("download_model");
      setBusy("");
      setModelReady(true);
    } catch (e) {
      setBusy(String(e));
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
  const statusLabel =
    state === "recording" ? "Recording — speak now"
    : state === "processing" ? "Transcribing…"
    : state === "error" ? status
    : modelReady ? "Ready" : "Model not downloaded yet";

  return (
    <main>
      <header className="masthead">
        <div className="brand">
          <span className={`daruma ${state}`} aria-hidden="true" />
          <h1>Sussurro</h1>
        </div>
        <p className="tagline">Local dictation. Your voice never leaves this machine.</p>
        <p className={`status status-${state}`} role="status">{statusLabel}</p>
      </header>

      <CollapsibleCard storageKey="dictationOpen" title="Dictation">
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
            <span>Push-to-talk <Tip text="On: recording lasts only while you hold the shortcut, like a walkie-talkie. Off: one tap starts recording, a second tap stops it." /></span>
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
            <span>Whisper model <Tip text="The local speech-to-text model (whisper.cpp) that turns your voice into text. Bigger = more accurate but slower to load and run. 'English' models are faster if you only dictate in English; multilingual models handle 90+ languages." /></span>
            <small>speech-to-text, fully offline</small>
          </div>
          <div className="model-row">
            <select
              value={settings.whisper_model}
              onChange={(e) => save({ ...settings, whisper_model: e.target.value })}
            >
              {MODELS.map((m) => (
                <option key={m.file} value={m.file}>{m.label}</option>
              ))}
            </select>
            {!modelReady && (
              <button className="btn-primary" onClick={downloadModel}>
                Download
              </button>
            )}
          </div>
        </div>
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
      </CollapsibleCard>

      {busy && <p className="busy" role="alert">{busy}</p>}

      <CollapsibleCard
        storageKey="historyOpen"
        className="history"
        title="History"
        headerExtra={
          history.length > 0 ? (
            <button
              className={`btn-ghost${confirmClear ? " danger" : ""}`}
              onClick={(e) => {
                // Inside <summary>: don't let the click toggle the accordion.
                e.preventDefault();
                e.stopPropagation();
                clearHistory();
              }}
            >
              {confirmClear ? "Click again to delete" : "Clear"}
            </button>
          ) : undefined
        }
      >
        {history.length === 0 && (
          <p className="empty">Nothing yet. Hold the shortcut and speak.</p>
        )}
        <ol>
          {history.map((h) => (
            <li key={h.timestamp}>
              <time>{new Date(h.timestamp).toLocaleTimeString()}</time>
              <p className="cleaned">{h.cleaned}</p>
              {h.cleaned !== h.raw && <p className="raw">{h.raw}</p>}
            </li>
          ))}
        </ol>
      </CollapsibleCard>

      <footer>
        <span>すべてローカル — everything stays local</span>
      </footer>
    </main>
  );
}
