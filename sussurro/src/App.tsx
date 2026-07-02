import { useEffect, useState } from "react";
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
}

interface HistoryEntry {
  timestamp: string;
  raw: string;
  cleaned: string;
}

const MODELS = [
  "ggml-base.en.bin",
  "ggml-small.bin",
  "ggml-medium.bin",
  "ggml-large-v3-turbo-q5_0.bin",
];

export default function App() {
  const [settings, setSettings] = useState<Settings | null>(null);
  const [history, setHistory] = useState<HistoryEntry[]>([]);
  const [status, setStatus] = useState("idle");
  const [modelReady, setModelReady] = useState(true);
  const [busy, setBusy] = useState("");

  const refresh = async () => {
    setSettings(await invoke<Settings>("get_settings"));
    setHistory(await invoke<HistoryEntry[]>("get_history", { n: 20 }));
    setModelReady(await invoke<boolean>("model_is_downloaded"));
  };

  useEffect(() => {
    refresh();
    const unlisten = listen<string>("pipeline-status", (e) => {
      setStatus(e.payload);
      if (e.payload === "idle") refresh();
    });
    return () => {
      unlisten.then((f) => f());
    };
  }, []);

  if (!settings) return <main>Loading…</main>;

  const save = async (next: Settings) => {
    setSettings(next);
    try {
      await invoke("set_settings", { settings: next });
      setBusy("");
      setModelReady(await invoke<boolean>("model_is_downloaded"));
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

  return (
    <main>
      <h1>Sussurro</h1>
      <p className={`status status-${status.split(":")[0]}`}>
        {status === "recording" ? "● Recording" : status === "processing" ? "… Processing" : status}
      </p>

      <section>
        <h2>Dictation</h2>
        <label>
          Hotkey{" "}
          <input
            value={settings.hotkey}
            onChange={(e) => setSettings({ ...settings, hotkey: e.target.value })}
            onBlur={() => save(settings)}
          />
        </label>
        <label>
          <input
            type="checkbox"
            checked={settings.push_to_talk}
            onChange={(e) => save({ ...settings, push_to_talk: e.target.checked })}
          />{" "}
          Push-to-talk (hold to record; unchecked = tap to toggle)
        </label>
        <label>
          Whisper model{" "}
          <select
            value={settings.whisper_model}
            onChange={(e) => save({ ...settings, whisper_model: e.target.value })}
          >
            {MODELS.map((m) => (
              <option key={m}>{m}</option>
            ))}
          </select>
        </label>
        {!modelReady && (
          <button onClick={downloadModel}>Download model</button>
        )}
      </section>

      <section>
        <h2>Cleanup (Ollama)</h2>
        <label>
          Level{" "}
          <select
            value={settings.cleanup_level}
            onChange={(e) =>
              save({ ...settings, cleanup_level: e.target.value as CleanupLevel })
            }
          >
            <option value="none">None (raw transcript)</option>
            <option value="light">Light (fillers + grammar)</option>
            <option value="medium">Medium (clarity + conciseness)</option>
            <option value="high">High (rewrite + polish)</option>
          </select>
        </label>
        <label>
          Ollama URL{" "}
          <input
            value={settings.ollama_url}
            onChange={(e) => setSettings({ ...settings, ollama_url: e.target.value })}
            onBlur={() => save(settings)}
          />
        </label>
        <label>
          Ollama model{" "}
          <input
            value={settings.ollama_model}
            onChange={(e) => setSettings({ ...settings, ollama_model: e.target.value })}
            onBlur={() => save(settings)}
          />
        </label>
        <label>
          Personal dictionary (one term per line)
          <textarea
            rows={4}
            value={settings.dictionary.join("\n")}
            onChange={(e) =>
              setSettings({
                ...settings,
                dictionary: e.target.value.split("\n").map((w) => w.trim()).filter(Boolean),
              })
            }
            onBlur={() => save(settings)}
          />
        </label>
      </section>

      {busy && <p className="busy">{busy}</p>}

      <section>
        <h2>History</h2>
        {history.length === 0 && <p>No dictations yet.</p>}
        <ul>
          {history.map((h) => (
            <li key={h.timestamp}>
              <span className="ts">{new Date(h.timestamp).toLocaleTimeString()}</span>{" "}
              {h.cleaned}
              {h.cleaned !== h.raw && <div className="raw">raw: {h.raw}</div>}
            </li>
          ))}
        </ul>
      </section>
    </main>
  );
}
