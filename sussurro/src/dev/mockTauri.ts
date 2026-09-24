/* DEV ONLY — a fake Tauri backend so the UI can be opened with `npm run dev`
   in a normal browser (visual checks, screenshots). main.tsx loads this file
   only when `import.meta.env.DEV` is true AND the page is not running inside
   Tauri, through a dynamic import that production builds drop entirely.

   URL switches: ?ui=legacy (classic window), ?empty=1 (empty archive). */

import { mockIPC, mockWindows } from "@tauri-apps/api/mocks";
import { emit } from "@tauri-apps/api/event";
import type { Item, ItemMeta, ItemSummary, LlmProfile, Segment, Settings } from "../lib/types";

const params = new URLSearchParams(window.location.search);

const settings: Settings = {
  hotkey: "CommandOrControl+Shift+Space",
  push_to_talk: true,
  whisper_model: "ggml-large-v3-turbo-q5_0.bin",
  engine: "whisper",
  llm_profiles: [
    { id: "local", name: "Local", api: "ollama", base_url: "http://localhost:11434", api_key: "", model: "llama3.2:3b", external: false },
    { id: "lm-studio", name: "LM Studio", api: "openai", base_url: "http://localhost:1234/v1", api_key: "", model: "qwen2.5-7b-instruct", external: false },
    { id: "work", name: "Work", api: "openai", base_url: "https://llm.example.com/v1", api_key: "sk-demo", model: "gpt-4o-mini", external: true },
  ],
  cleanup_profile: "local",
  cleanup_level: "light",
  output_language: "",
  dictionary: ["Sussurro", "Tauri", "DarumaHQ"],
  autostart: false,
  sound_feedback: true,
  language: "it",
  snippets: [{ cue: "firma email", text: "Francesco Fullone\nDarumaHQ" }],
  live_preview: true,
  app_styles: [],
  models_dir: "",
  input_device: "",
  whisper_mode: false,
  stream_injection: false,
  voice_commands: true,
  prompt_overrides: { light: "", medium: "", high: "" },
  history_retention_days: 0,
  api_enabled: false,
  api_port: 4525,
  output_file: "",
  archive_dir: "",
  ui_v2: params.get("ui") !== "legacy",
};

const ARCHIVE = "/Users/demo/Documents/Sussurro";

interface Stored {
  id: string;
  meta: ItemMeta;
  segments: Segment[];
  edited_externally?: boolean;
  interrupted?: boolean;
  recording?: boolean;
}

const at = (daysAgo: number, h: number, m: number) => {
  const d = new Date();
  d.setDate(d.getDate() - daysAgo);
  d.setHours(h, m, 0, 0);
  return d.toISOString();
};

function segs(lines: string[], stepMs = 7000): Segment[] {
  return lines.map((text, i) => ({ id: i, start_ms: i * stepMs + 4000, end_ms: i * stepMs + 4000 + stepMs - 800, raw: text, text }));
}

function meta(title: string, type: ItemMeta["type"], date: string, duration: string, source: string, over: Partial<ItemMeta> = {}): ItemMeta {
  return { type, title, date, duration, source, language: "it", engine: "whisper-large-v3-turbo-q5_0", tags: [], categories: [], participants: [], ...over };
}

let items: Stored[] = params.get("empty")
  ? []
  : [
      {
        id: "2026/09/idee-per-l-onboarding",
        meta: meta("Idee per l'onboarding", "note", at(0, 8, 40), "00:03:12", "mic", { tags: ["onboarding", "idee"], categories: ["prodotto"] }),
        segments: segs([
          "Tre idee per rendere il primo avvio più semplice.",
          "Primo: chiedere l'accesso alla cartella Documenti durante l'onboarding, mai durante una registrazione.",
          "Secondo: se Ollama ha già un modello installato lo usiamo e lo diciamo all'utente invece di chiedere un download.",
          "Terzo: proporre una nota di prova guidata, così si vede subito dove finiscono i file.",
          "",
          "Da discutere con Anna nella weekly di giovedì.",
        ]).map((s, i) => (i === 4 ? { ...s, text: "", raw: "", stt_error: "whisper: failed to decode" } : s)),
      },
      {
        id: "2026/09/podcast-daruma-ep-12-intervista",
        meta: meta("Podcast Daruma, ep. 12 — intervista", "transcription", at(1, 17, 5), "00:48:10", "file:podcast-ep12.mp3", { tags: ["podcast"] }),
        segments: segs([
          "Benvenuti a una nuova puntata del podcast di Daruma.",
          "Oggi parliamo di software locale e di privacy con un ospite speciale.",
          "La domanda che ci fanno più spesso è: perché non usare semplicemente il cloud?",
          "La risposta breve è che la voce è un dato personale, e non dovrebbe lasciare il tuo computer.",
          "Con i modelli di oggi un portatile recente trascrive un'ora di audio in pochi minuti.",
          "E la pulizia del testo con un piccolo modello locale è sorprendentemente buona.",
          "Parliamo anche di licenze: AGPL per proteggere il lavoro della comunità.",
          "Grazie a tutti per l'ascolto, alla prossima puntata.",
        ], 21000),
      },
      {
        id: "2026/09/call-con-studio-verdi",
        meta: meta("Call con Studio Verdi", "meeting", at(2, 11, 0), "00:38:02", "mic", { categories: ["clienti"] }),
        segments: segs([
          "Ok, partiamo dalla release 0.7: archivio e schermata New.",
          "La cartella in Documenti funziona su tutti e tre i sistemi, manca il test con OneDrive.",
          "Ho importato un file da un'ora: la memoria resta piatta, nessun picco.",
        ]),
        edited_externally: true,
      },
      {
        id: "2026/09/lezione-diritto-d-autore-e-ia",
        meta: meta("Lezione: diritto d'autore e IA — una lezione molto lunga con un titolo lunghissimo", "transcription", at(5, 9, 30), "01:12:40", "file:lezione.m4a"),
        segments: segs(["Oggi vediamo come il diritto d'autore si applica ai modelli generativi.", "Partiamo dalla direttiva europea sul copyright nel mercato unico digitale."], 30000),
        interrupted: true,
      },
      {
        id: "2026/09/appunti-treno-per-bologna",
        meta: meta("Appunti treno per Bologna", "note", at(6, 7, 55), "00:02:04", "mic"),
        segments: segs(["Ricordarsi di prenotare il ritorno.", "Scrivere il post sul blog per la 0.7 durante il viaggio."]),
      },
      {
        id: "2025/12/lista-regali",
        meta: meta("Lista regali di Natale", "note", "2025-12-11T19:20:00+01:00", "00:01:02", "mic", { tags: ["personale"] }),
        segments: segs(["Un libro per papà, i colori per Giulia, e qualcosa per il gatto."]),
      },
    ];

const find = (id: string) => items.find((i) => i.id === id);

function toItem(s: Stored): Item {
  const body = `\n# ${s.meta.title}\n\n${s.segments.map((x) => x.text).join(" ")}\n`;
  return {
    id: s.id,
    meta: { ...s.meta },
    segments: { version: 1, speakers: [], segments: s.segments.map((x) => ({ ...x })) },
    body,
    edited_externally: !!s.edited_externally,
    recording: !!s.recording,
    interrupted: !!s.interrupted,
  };
}

function toSummary(s: Stored, snippet?: string): ItemSummary {
  return {
    id: s.id,
    meta: { ...s.meta },
    edited_externally: !!s.edited_externally,
    recording: !!s.recording,
    interrupted: !!s.interrupted,
    ...(snippet ? { snippet } : {}),
  };
}

const newestFirst = (a: Stored, b: Stored) => b.meta.date.localeCompare(a.meta.date);

function search(query: string, type?: string): ItemSummary[] {
  const q = query.trim().toLowerCase();
  return items
    .filter((s) => !type || s.meta.type === type)
    .slice()
    .sort(newestFirst)
    .flatMap((s) => {
      if (!q) return [toSummary(s)];
      const text = s.segments.map((x) => x.text).join(" ");
      const hay = `${s.meta.title} ${text} ${s.meta.tags.join(" ")}`.toLowerCase();
      if (!q.split(/\s+/).every((w) => hay.includes(w))) return [];
      const at = text.toLowerCase().indexOf(q.split(/\s+/)[0]);
      if (at < 0) return [toSummary(s)];
      const from = Math.max(0, at - 40);
      const w = q.split(/\s+/)[0].length;
      const snippet = `${from > 0 ? "…" : ""}${text.slice(from, at)}**${text.slice(at, at + w)}**${text.slice(at + w, at + 80)}…`;
      return [toSummary(s, snippet)];
    });
}

/* ---------- engine simulation ---------- */

let nextSession = 1;
let mic: { id: number; itemId: string; timer: number; n: number; started: number } | null = null;
const FAKE_LINES = [
  "Allora, provo a registrare una nota lunga dal microfono.",
  "Il testo appare riga per riga mentre parlo, con il timestamp a sinistra.",
  "Quando premo stop la nota finisce nella Library con i tag di default.",
  "Posso poi correggere una riga o cancellarla dal pannello del documento.",
  "Tutto resta in locale, in una cartella di file markdown.",
];

const ev = (name: string, payload: unknown) => emit(name, payload);

function micTick() {
  if (!mic) return;
  const i = mic.n++;
  const seg: Segment = { id: i, start_ms: i * 6000, end_ms: i * 6000 + 5200, raw: FAKE_LINES[i % FAKE_LINES.length], text: FAKE_LINES[i % FAKE_LINES.length] };
  const s = find(mic.itemId);
  if (s) s.segments.push(seg);
  ev("engine-segment", { session_id: mic.id, segment: seg });
  const t = (Date.now() - mic.started) / 1000;
  ev("engine-progress", { session_id: mic.id, processed_s: i * 6 + 5, ingested_s: t, total_s: t, backlog_s: Math.max(0, t - (i * 6 + 5)), queue_len: 0, segments_done: i + 1 });
}

/** A run's language (#157): the one chosen in New, else the settings'. The
 *  per-run cleanup level only changes the (fake) text, so it is just logged. */
function runLanguage(a: Args): string {
  if (a.cleanupLevel) console.info("[mock] run cleanup level", a.cleanupLevel);
  return (a.language as string | null) || settings.language;
}

function startMic(title: string | null, language: string): number {
  const id = nextSession++;
  const itemId = `2026/09/${new Date().toISOString().slice(0, 10)}-untitled`;
  items.push({ id: itemId, meta: meta(title || "Untitled", "note", new Date().toISOString(), "", "mic", { language }), segments: [], recording: true });
  mic = { id, itemId, n: 0, started: Date.now(), timer: window.setInterval(micTick, 2500) };
  setTimeout(() => ev("engine-started", { session_id: id, item_id: itemId, item_type: "note", title: title ?? "", source: "mic" }), 50);
  return id;
}

function stopMic(): number {
  if (!mic) throw "no microphone session is running";
  const m = mic;
  clearInterval(m.timer);
  mic = null;
  setTimeout(() => {
    const s = find(m.itemId);
    if (!s) return;
    s.recording = false;
    const title = s.meta.title === "Untitled" ? "Allora, provo a registrare una nota lunga" : s.meta.title;
    s.meta.title = title;
    s.id = m.itemId.replace("untitled", "nota-dal-microfono");
    s.meta.duration = `00:00:${String(Math.round((Date.now() - m.started) / 1000) % 60).padStart(2, "0")}`;
    ev("engine-done", { session_id: m.id, item_id: s.id, item_type: "note", title, text: "", segments: s.segments.length, duration_s: (Date.now() - m.started) / 1000 });
  }, 1200);
  return m.id;
}

function cancel(id: number): boolean {
  if (mic && mic.id === id) {
    clearInterval(mic.timer);
    items = items.filter((i) => i.id !== mic!.itemId);
    mic = null;
    ev("engine-error", { session_id: id, error: "cancelled" });
    return true;
  }
  if (fileRun && fileRun.id === id) {
    fileRun.cancelled = true;
    return true;
  }
  return false;
}

let fileRun: { id: number; cancelled: boolean } | null = null;

async function transcribeFile(path: string, itemType: "note" | "transcription", title: string | null, language: string) {
  const id = nextSession++;
  fileRun = { id, cancelled: false };
  const name = path.split("/").pop() ?? path;
  const itemId = `2026/09/${name.replace(/\.[^.]+$/, "")}`;
  const total = 192;
  items.push({ id: itemId, meta: meta(title || name.replace(/\.[^.]+$/, ""), itemType, new Date().toISOString(), "", `file:${name}`, { language }), segments: [], recording: true });
  await new Promise((r) => setTimeout(r, 30));
  ev("engine-started", { session_id: id, item_id: itemId, item_type: itemType, title: title ?? "", source: `file:${name}` });
  for (let i = 0; i < 8; i++) {
    await new Promise((r) => setTimeout(r, 700));
    if (fileRun.cancelled) {
      items = items.filter((x) => x.id !== itemId);
      fileRun = null;
      ev("engine-error", { session_id: id, error: "cancelled" });
      throw "cancelled";
    }
    const seg: Segment = { id: i, start_ms: i * 24000, end_ms: i * 24000 + 23000, raw: FAKE_LINES[i % 5], text: FAKE_LINES[i % 5] };
    find(itemId)?.segments.push(seg);
    ev("engine-segment", { session_id: id, segment: seg });
    ev("engine-progress", { session_id: id, processed_s: (i + 1) * 24, ingested_s: Math.min(total, (i + 2) * 24), total_s: total, backlog_s: 24, queue_len: 1, segments_done: i + 1 });
  }
  const s = find(itemId)!;
  s.recording = false;
  s.meta.duration = "00:03:12";
  fileRun = null;
  const result = { session_id: id, item_id: itemId, item_type: itemType, title: s.meta.title, text: "", segments: 8, duration_s: total };
  ev("engine-done", result);
  return result;
}

/* ---------- LLM profiles (#119) ---------- */

/** Fake model listing per profile: Ollama and localhost servers answer,
 *  "*.example.com" hosts behave as unreachable (to preview the error). */
function listModels(p: LlmProfile | undefined): string[] {
  if (!p) throw "no profile";
  if (/example\.com/.test(p.base_url)) throw "OpenAI-compatible server not reachable";
  return p.api === "ollama" ? ["llama3.2:3b", "qwen2.5:3b"] : ["qwen2.5-7b-instruct", "gemma-3-4b-it"];
}

/* ---------- command table ---------- */

type Args = Record<string, unknown>;

function handle(cmd: string, a: Args): unknown {
  switch (cmd) {
    case "get_settings":
      return { ...settings };
    case "set_settings":
      Object.assign(settings, a.settings as Settings);
      return null;
    case "get_history":
      return [
        { timestamp: at(0, 10, 12), raw: "ehm allora mandami il file entro domani", cleaned: "Mandami il file entro domani." },
        { timestamp: at(0, 9, 3), raw: "ciao Anna ci vediamo alle tre", cleaned: "Ciao Anna, ci vediamo alle tre." },
      ];
    case "search_history":
      return [];
    case "usage_stats":
      return { total_dictations: 132, total_words: 5210, today_dictations: 4, today_words: 88, week_dictations: 31, week_words: 1240 };
    case "model_is_downloaded":
      return true;
    case "list_whisper_models":
      return ["ggml-large-v3-turbo-q5_0.bin", "ggml-small.bin"];
    case "list_ollama_models":
      return listModels(settings.llm_profiles.find((p) => p.id === settings.cleanup_profile) ?? settings.llm_profiles[0]);
    case "llm_list_models":
      return listModels(a.profile as LlmProfile);
    case "list_input_devices":
      return ["MacBook Pro Microphone", "USB Audio Device"];
    case "get_default_prompts":
      return ["Remove fillers, fix grammar.", "Also tighten for clarity.", "Rewrite for brevity."];
    case "ollama_status":
      return { installed: true, running: true, has_model: true };
    case "check_permissions":
      return { microphone: "granted", accessibility: "granted" };
    case "mic_level":
      return 0.02 + Math.random() * 0.05;
    case "start_mic_test":
    case "stop_mic_test":
    case "trigger_dictation":
    case "copy_text":
    case "open_settings":
      return null;
    case "diagnostics":
      return "Sussurro dev preview";
    case "archive_dir":
      return settings.archive_dir || ARCHIVE;
    case "archive_list":
      return items.slice().sort(newestFirst).map((s) => toSummary(s));
    case "archive_search": {
      const f = (a.filters ?? {}) as { type?: string };
      return search(String(a.query ?? ""), f.type);
    }
    case "archive_get": {
      const s = find(String(a.id));
      if (!s) throw `no archive item '${a.id}'`;
      return toItem(s);
    }
    case "archive_update_meta": {
      const s = find(String(a.id));
      if (!s) throw `no archive item '${a.id}'`;
      s.meta = { ...(a.meta as ItemMeta) };
      return toItem(s);
    }
    case "archive_update_segment":
    case "archive_delete_segment": {
      const s = find(String(a.id));
      if (!s) throw `no archive item '${a.id}'`;
      if (s.edited_externally) throw `'${s.id}' was edited outside Sussurro: its transcript.md is kept as is — edit it there`;
      const sid = Number(a.segmentId);
      if (cmd === "archive_delete_segment") s.segments = s.segments.filter((x) => x.id !== sid);
      else
        s.segments = s.segments.map((x) =>
          x.id === sid ? { ...x, text: String(a.text).trim(), edited: true, stt_error: undefined } : x,
        );
      return toItem(s);
    }
    case "archive_delete":
      items = items.filter((s) => s.id !== a.id);
      return null;
    case "archive_reveal":
      console.info("[mock] reveal", a.id ?? ARCHIVE);
      return null;
    case "archive_rebuild_index":
      return items.length;
    case "engine_status":
      return {
        active: (mic ? 1 : 0) + (fileRun ? 1 : 0),
        mic_session: mic?.id ?? null,
        file_sessions: fileRun ? [{ session_id: fileRun.id, label: "mock.wav" }] : [],
      };
    case "engine_start_mic":
      if (mic) throw "a microphone session is already running";
      return startMic((a.title as string | null) ?? null, runLanguage(a));
    case "engine_stop_mic":
      return stopMic();
    case "engine_cancel":
      return cancel(Number(a.sessionId));
    case "transcribe_file":
      return transcribeFile(
        String(a.path),
        (a.itemType as "note" | "transcription") ?? "note",
        (a.title as string | null) ?? null,
        runLanguage(a),
      );
    case "pick_import_file":
      // The real command opens the picker in Rust and returns {name, contents}
      // (null on cancel); the preview skips the dialog and returns a sample.
      return a.kind === "snippets"
        ? { name: "snippets.csv", contents: 'cue,text\nfirma,"Un saluto,\nFrancesco"\nindirizzo,Via Roma 1\n' }
        : { name: "dictionary.txt", contents: "Sussurro\nTauri\nwhisper.cpp\n" };
    case "plugin:app|version":
      return "0.7.0-dev";
    case "plugin:dialog|open": {
      const o = (a.options ?? {}) as { directory?: boolean };
      return o.directory ? "/Users/demo/Obsidian/Vault/Sussurro" : "/Users/demo/Recordings/memo-idee-onboarding.m4a";
    }
    case "plugin:dialog|save":
    case "plugin:opener|open_url":
    case "plugin:updater|check":
      return null;
    default:
      console.warn("[mock] unhandled command", cmd, a);
      return null;
  }
}

export function installMockTauri(): void {
  mockWindows("main");
  mockIPC((cmd, args) => handle(cmd, (args ?? {}) as Args), { shouldMockEvents: true });
  console.info("[dev] Tauri backend mocked — browser preview only");
}
