/* DEV ONLY — a fake Tauri backend so the UI can be opened with `npm run dev`
   in a normal browser (visual checks, screenshots). main.tsx loads this file
   only when `import.meta.env.DEV` is true AND the page is not running inside
   Tauri, through a dynamic import that production builds drop entirely.

   URL switches: ?ui=legacy (classic window), ?empty=1 (empty archive),
   ?ytdlp=0 (yt-dlp not installed, for the Link tab), ?keychain=0 (no OS
   credential store: the profile editor's clear-text key warning). */

import { mockIPC, mockWindows } from "@tauri-apps/api/mocks";
import { emit } from "@tauri-apps/api/event";
import type { CompanionDoc, Item, ItemMeta, ItemSummary, LlmProfile, Recipe, Segment, Settings } from "../lib/types";

const params = new URLSearchParams(window.location.search);

const settings: Settings = {
  hotkey: "CommandOrControl+Shift+Space",
  push_to_talk: true,
  whisper_model: "ggml-large-v3-turbo-q5_0.bin",
  engine: "whisper",
  llm_profiles: [
    { id: "local", name: "Local", api: "ollama", base_url: "http://localhost:11434", api_key: "", model: "llama3.2:3b", external: false },
    { id: "lm-studio", name: "LM Studio", api: "openai", base_url: "http://localhost:1234/v1", api_key: "", model: "qwen2.5-7b-instruct", external: false, context_tokens: 32768 },
    { id: "work", name: "Work", api: "openai", base_url: "https://llm.example.com/v1", api_key: "", model: "gpt-4o-mini", external: true },
  ],
  cleanup_profile: "local",
  recipes: [
    {
      id: "domande-aperte",
      name: "Domande aperte",
      prompt: "Elenca le domande rimaste aperte, con chi le ha sollevate.",
      target: "companion_document",
      builtin: false,
    },
    {
      id: "chi-ha-detto-cosa",
      name: "Chi ha detto cosa",
      prompt: "Per ogni partecipante, riassumi in una riga cosa ha detto.",
      target: "answer",
      builtin: false,
    },
  ],
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
  subtitles: "on_request",
  meetings_enabled: false,
  extension_token: "",
};

/** A fake pairing token (#126): 64 hex characters, like the backend's. */
function fakeToken(): string {
  return Array.from({ length: 64 }, () => "0123456789abcdef"[Math.floor(Math.random() * 16)]).join("");
}

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
        meta: meta("Podcast Daruma, ep. 12 — intervista", "transcription", at(1, 17, 5), "00:48:10", "file:podcast-ep12.mp3", {
          tags: ["podcast"],
          participants: [{ name: "Francesco Fullone", email: "francesco@example.com" }, { name: "Ospite" }],
        }),
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
        meta: meta("Call con Studio Verdi", "meeting", at(2, 11, 0), "00:38:02", "mic", {
          categories: ["clienti"],
          participants: [{ name: "Anna Rossi", email: "anna@example.com" }, { name: "Marco Bianchi", email: "marco@example.com" }, { name: "Voice 1" }],
        }),
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

/** Items whose transcript.srt the preview "wrote" (#133). */
const srtWritten = new Set<string>();

/** External sends per item id (#122): what `.sussurro/external-log.json`
 *  records — hosts only here. */
const externalSends: Record<string, string[]> = {};

/** The item's "sent externally" hosts: its log plus external companions. */
function hostsOf(id: string): string[] {
  const fromDocs = (docs[id] ?? []).filter((d) => d.meta.external).map((d) => d.meta.host || "an unknown host");
  return [...new Set([...(externalSends[id] ?? []), ...fromDocs])].sort();
}

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
    external_hosts: hostsOf(s.id),
  };
}

function toSummary(s: Stored, snippet?: string): ItemSummary {
  return {
    id: s.id,
    meta: { ...s.meta },
    edited_externally: !!s.edited_externally,
    recording: !!s.recording,
    interrupted: !!s.interrupted,
    external_hosts: hostsOf(s.id),
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
      const people = s.meta.participants.map((p) => `${p.name} ${p.email ?? ""}`).join(" ");
      const hay = `${s.meta.title} ${text} ${s.meta.tags.join(" ")} ${s.meta.categories.join(" ")} ${people}`.toLowerCase();
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
  if (linkRun && linkRun.id === id) {
    linkRun.cancelled = true;
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

/* ---------- links (#123) ---------- */

const YT_DLP = params.get("ytdlp") !== "0";
const PLATFORMS = ["youtube.com", "youtu.be", "vimeo.com", "soundcloud.com", "dailymotion.com", "twitch.tv"];
const MEDIA = /\.(mp3|wav|m4a|aac|mp4|mov|flac|ogg|opus|webm|mkv)$/i;

/** A rough copy of `sources/url` classification, for the preview only. */
function linkInspect(input: string) {
  let u: URL;
  try {
    u = new URL(input.trim());
  } catch {
    return { kind: null, error: "not a valid link — it should start with https://", local: false, label: "" };
  }
  if (u.protocol !== "http:" && u.protocol !== "https:")
    return { kind: null, error: `only http and https links are supported, not ${u.protocol}`, local: false, label: "" };
  if (u.username || u.password)
    return { kind: null, error: "links with a user name or password are not supported (the link is saved in the item)", local: false, label: "" };
  const host = u.hostname.replace(/^www\./, "");
  const platform = !MEDIA.test(u.pathname) && PLATFORMS.some((d) => host === d || host.endsWith(`.${d}`));
  const local = /^(localhost|127\.|10\.|192\.168\.|169\.254\.|\[::1\])/.test(u.hostname);
  return { kind: platform ? "platform" : "direct", error: null, local, label: `${host}${u.pathname.replace(/\/$/, "")}${u.search}`.slice(0, 60) };
}

let linkRun: { id: number; cancelled: boolean; label: string } | null = null;

function startLink(url: string, title: string | null, language: string): number {
  if (linkRun) throw "a link is already being transcribed";
  const info = linkInspect(url);
  if (info.error) throw info.error;
  if (info.kind === "platform" && !YT_DLP)
    throw "links to video sites need yt-dlp, which was not found. Install yt-dlp with Homebrew: `brew install yt-dlp` (or `pipx install yt-dlp`).";
  const id = nextSession++;
  const run = { id, cancelled: false, label: info.label };
  linkRun = run;
  const via = info.kind === "platform" ? "yt-dlp" : "direct";
  const platformTitle = info.kind === "platform" ? "Local-first software: a conversation" : null;
  const total = 18 * 1024 * 1024;
  (async () => {
    const fail = () => {
      linkRun = null;
      ev("engine-error", { session_id: id, error: "cancelled" });
    };
    for (let i = 0; i <= 10; i++) {
      await new Promise((r) => setTimeout(r, 300));
      if (run.cancelled) return fail();
      ev("engine-download", { session_id: id, via, downloaded_bytes: (total * i) / 10, total_bytes: via === "direct" && i === 0 ? null : total, title: i > 1 ? platformTitle : null });
    }
    const name = title || platformTitle || decodeURIComponent(new URL(url).pathname.split("/").filter(Boolean).pop() ?? "link").replace(MEDIA, "");
    const itemId = `2026/09/${name.toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-|-$/g, "")}`;
    items.push({ id: itemId, meta: meta(name, "transcription", new Date().toISOString(), "", `url:${url}`, { language }), segments: [], recording: true });
    ev("engine-started", { session_id: id, item_id: itemId, item_type: "transcription", title: name, source: `url:${url}` });
    for (let i = 0; i < 6; i++) {
      await new Promise((r) => setTimeout(r, 600));
      if (run.cancelled) {
        items = items.filter((x) => x.id !== itemId);
        return fail();
      }
      const seg: Segment = { id: i, start_ms: i * 24000, end_ms: i * 24000 + 23000, raw: FAKE_LINES[i % 5], text: FAKE_LINES[i % 5] };
      find(itemId)?.segments.push(seg);
      ev("engine-segment", { session_id: id, segment: seg });
      ev("engine-progress", { session_id: id, processed_s: (i + 1) * 24, ingested_s: Math.min(144, (i + 2) * 24), total_s: 144, backlog_s: 24, queue_len: 1, segments_done: i + 1 });
    }
    const s = find(itemId)!;
    s.recording = false;
    s.meta.duration = "00:02:24";
    linkRun = null;
    ev("engine-done", { session_id: id, item_id: itemId, item_type: "transcription", title: name, text: "", segments: 6, duration_s: 144 });
  })();
  return id;
}

/* ---------- LLM profiles (#119) ---------- */

/** Fake model listing per profile: Ollama and localhost servers answer,
 *  "*.example.com" hosts behave as unreachable (to preview the error). */
function listModels(p: LlmProfile | undefined): string[] {
  if (!p) throw "no profile";
  if (/example\.com/.test(p.base_url)) throw "OpenAI-compatible server not reachable";
  return p.api === "ollama" ? ["llama3.2:3b", "qwen2.5:3b"] : ["qwen2.5-7b-instruct", "gemma-3-4b-it"];
}

/* ---------- recipes (#120) ---------- */

const BUILTIN_RECIPES: Recipe[] = [
  { id: "formatted-document", name: "Formatted document", prompt: "Turn the transcript into a well-structured written document in markdown: a **tl;dr:** line, a # title, ## to ###### headings, tables where the content is tabular.", target: "companion_document", builtin: true },
  { id: "summary", name: "Summary", prompt: "Summarise the transcript in markdown: a short paragraph with the gist, then the key points as a bullet list.", target: "companion_document", builtin: true },
  { id: "action-items", name: "Action items", prompt: "List every action item in the transcript as a markdown task list (`- [ ] …`), with owner and deadline when said.", target: "companion_document", builtin: true },
  { id: "decisions", name: "Decisions", prompt: "List the decisions taken in the transcript as a markdown bullet list, each with its reason.", target: "companion_document", builtin: true },
];

const allRecipes = (): Recipe[] => [...BUILTIN_RECIPES, ...settings.recipes];

function companionFile(r: Recipe): string {
  if (r.id === "formatted-document") return "document.md";
  const s = r.name.toLowerCase().normalize("NFKD").replace(/[̀-ͯ]/g, "").replace(/[^a-z0-9]+/g, "-").replace(/^-+|-+$/g, "") || "untitled";
  return s === "transcript" || s === "document" ? `recipe-${s}.md` : `${s}.md`;
}

function companion(item: Stored, r: Recipe, p: LlmProfile, body: string, date = new Date().toISOString()): CompanionDoc {
  return {
    file: companionFile(r),
    meta: {
      title: `${item.meta.title} — ${r.name}`,
      generated_by: `${r.name} / ${p.name} / ${p.model}`,
      recipe: r.id,
      profile: p.name,
      model: p.model,
      external: p.external,
      ...(p.external ? { host: hostOf(p) } : {}),
      date,
      transcript: "transcript.md",
    },
    body,
    edited_externally: false,
  };
}

const ONBOARDING_DOC = `**tl;dr:** Tre idee per un primo avvio più semplice: chiedere l'accesso a Documenti durante l'onboarding, riusare un modello già installato e proporre una nota di prova.

# Idee per l'onboarding

## Primo avvio

### Permessi

Su macOS la richiesta per la cartella Documenti va fatta **subito**, mai durante una registrazione.

### Modelli

Se Ollama ha già un modello, lo usiamo e lo diciamo all'utente invece di chiedere un download.

## Priorità

| Idea | Impegno | Release |
|---|:---:|---|
| Accesso a Documenti all'onboarding | Basso | 0.7 |
| Riuso del modello installato | Medio | 0.7 |
| Nota di prova guidata | Medio | 0.8 |

## Prossimi passi

- [ ] Discuterne con Anna nella weekly di giovedì
- [x] Scrivere le idee in una nota`;

/** Companion documents per item id. */
const docs: Record<string, CompanionDoc[]> = {};
if (!params.get("empty")) {
  const onboarding = "2026/09/idee-per-l-onboarding";
  const local = settings.llm_profiles[0];
  docs[onboarding] = [
    companion({ id: onboarding, meta: meta("Idee per l'onboarding", "note", at(0, 8, 40), "", "mic"), segments: [] }, BUILTIN_RECIPES[0], { ...local, model: "qwen3:1.7b" }, ONBOARDING_DOC, at(0, 8, 50)),
  ];
  // The mock's Library marker (#122): a summary made on the external profile.
  const podcast = "2026/09/podcast-daruma-ep-12-intervista";
  const work = settings.llm_profiles.find((p) => p.external)!;
  const podcastItem = items.find((i) => i.id === podcast);
  if (podcastItem) {
    docs[podcast] = [
      companion(podcastItem, BUILTIN_RECIPES[1], work, "La voce è un dato personale: il podcast spiega perché Sussurro trascrive in locale.\n\n- Un portatile recente trascrive un'ora in pochi minuti.\n- La pulizia con un modello locale è sorprendentemente buona.\n- Licenza AGPL per proteggere il lavoro della comunità.", at(1, 18, 0)),
    ];
    externalSends[podcast] = [hostOf(work)];
  }
}

/* ---------- privacy gate (#122) ---------- */

function hostOf(p: LlmProfile): string {
  const s = p.base_url.trim();
  const rest = s.includes("://") ? s.slice(s.indexOf("://") + 3) : s;
  return (rest.split(/[/?#]/)[0] ?? "").replace(/^.*@/, "").replace(/:\d+$/, "").toLowerCase();
}

/** One-time confirmation tokens: what each allows, and when it was issued. */
const consents: Record<string, { key: string; at: number }> = {};
let nextConsent = 1;

function consentKey(id: string, r: Recipe, question: string | null, p: LlmProfile): string {
  return [id, r.id, question ?? "", p.id, hostOf(p), p.model].join("\u0000");
}

function resolveRun(a: Args): { item: Stored; recipe: Recipe; question: string | null; profile: LlmProfile } {
  const item = find(String(a.id));
  if (!item) throw `no archive item '${a.id}'`;
  let question = a.question == null ? null : String(a.question).split(/\s+/).filter(Boolean).join(" ");
  if (question === "") throw "write a question first";
  const recipe = question !== null ? QUESTION : allRecipes().find((x) => x.id === a.recipeId);
  if (!recipe) throw `no recipe '${a.recipeId}'`;
  const profile = settings.llm_profiles.find((x) => x.id === a.profileId);
  if (!profile) throw `no LLM profile '${a.profileId}'`;
  return { item, recipe, question, profile };
}

function preview(a: Args) {
  const { item, recipe, question, profile } = resolveRun(a);
  const chars = item.segments.reduce((n, s) => n + s.text.length + 12, 0) + recipe.prompt.length;
  return {
    item_id: item.id,
    item_title: item.meta.title,
    recipe_id: recipe.id,
    recipe_name: recipe.name,
    question,
    profile_id: profile.id,
    profile_name: profile.name,
    host: hostOf(profile),
    base_url: profile.base_url,
    model: profile.model,
    chars,
    approx_tokens: Math.ceil(chars / 4),
    external: profile.external,
  };
}

function prepare(a: Args) {
  const { item, recipe, question, profile } = resolveRun(a);
  if (!profile.external) throw `“${profile.name}” is a local profile: its runs need no confirmation`;
  const token = `mock-consent-${nextConsent++}`;
  consents[token] = { key: consentKey(item.id, recipe, question, profile), at: Date.now() };
  return { token, expires_in_secs: 120 };
}

/** The backend's gate: an external run needs a fresh, matching, unused token. */
function consume(token: unknown, key: string, p: LlmProfile) {
  if (!p.external) return;
  if (typeof token !== "string" || !consents[token])
    throw `“${p.name}” is an external profile: the transcript would go to ${hostOf(p)}. Confirm the run first — nothing was sent.`;
  const c = consents[token];
  delete consents[token];
  if (Date.now() - c.at > 120_000) throw "this confirmation expired — confirm the run again";
  if (c.key !== key) throw "this confirmation was given for another run — confirm the run again";
}

function fakeResult(r: Recipe, item: Stored): string {
  const text = item.segments.map((s) => s.text).filter(Boolean);
  switch (r.id) {
    case "formatted-document":
      return `**tl;dr:** ${text[0] ?? ""}\n\n# ${item.meta.title}\n\n## Contenuto\n\n${text.slice(1).join(" ")}\n\n| Punto | Dettaglio |\n|---|---|\n${text.slice(0, 3).map((t, i) => `| ${i + 1} | ${t} |`).join("\n")}`;
    case "action-items":
      return text.slice(0, 2).map((t) => `- [ ] ${t}`).join("\n");
    default:
      return `${text[0] ?? ""}\n\n${text.slice(1, 4).map((t) => `- ${t}`).join("\n")}`;
  }
}

/** Recipe runs in flight, by item id. */
const recipeRuns: Record<string, { recipe: Recipe; question: string | null; cancelled: boolean; step: { phase: string; done: number; total: number } | null }> = {};

/** Ask panel answers not saved yet (#121), by answer id. */
interface PendingAnswer {
  itemId: string;
  recipe: Recipe;
  question: string | null;
  profile: LlmProfile;
  text: string;
  date: string;
}
const answers: Record<number, PendingAnswer> = {};
let nextAnswer = 1;

function fakeAnswer(question: string | null, item: Stored): string {
  const text = item.segments.map((s) => s.text).filter(Boolean);
  if (!question) return text.slice(0, 3).map((t) => `- ${t}`).join("\n") || "Nessun contenuto.";
  if (/chi|who/i.test(question)) return `Secondo la trascrizione: **${text[0] ?? "nessuno lo dice"}**\n\n- ${text[1] ?? "—"}`;
  return `${text[0] ?? "La trascrizione non ne parla."}\n\n${text.slice(1, 3).map((t) => `- ${t}`).join("\n")}`;
}

function answerFile(a: PendingAnswer): string {
  const s = slugOf(a.question ?? a.recipe.name) || "untitled";
  return s === "transcript" || s === "document" ? `${a.question ? "question" : "recipe"}-${s}.md` : `${s}.md`;
}

function slugOf(s: string): string {
  return s.toLowerCase().normalize("NFKD").replace(/[̀-ͯ]/g, "").replace(/[^a-z0-9]+/g, "-").replace(/^-+|-+$/g, "").slice(0, 60).replace(/-+$/, "");
}

function saveAnswer(id: string, answerId: number): string {
  const a = answers[answerId];
  if (!a || a.itemId !== id) throw "this answer is no longer available — ask again to save it";
  const item = find(id)!;
  const list = (docs[id] ??= []);
  const base = answerFile(a);
  let file = base;
  for (let n = 2; list.some((d) => d.file === file); n++) file = base.replace(/\.md$/, `-${n}.md`);
  const doc = companion(item, a.recipe, a.profile, a.question ? `> **Question:** ${a.question}\n\n${a.text}` : a.text, a.date);
  doc.file = file;
  doc.meta.title = `${item.meta.title} — ${a.question ?? a.recipe.name}`;
  doc.meta.kind = "answer";
  if (a.question) doc.meta.question = a.question;
  list.push(doc);
  list.sort((x, y) => Number(x.file !== "document.md") - Number(y.file !== "document.md") || x.file.localeCompare(y.file));
  delete answers[answerId];
  return file;
}

const QUESTION: Recipe = { id: "question", name: "Question", prompt: "", target: "answer", builtin: true };

async function runRecipe(id: string, recipeId: string, profileId: string | null, question: string | null = null, consent: unknown = null) {
  const item = find(id);
  if (!item) throw `no archive item '${id}'`;
  const r = question !== null ? QUESTION : allRecipes().find((x) => x.id === recipeId);
  if (!r) throw `no recipe '${recipeId}'`;
  if (question !== null) {
    question = question.split(/\s+/).filter(Boolean).join(" ");
    if (!question) throw "write a question first";
  }
  // No profile named: a local one, never an external fallback (#122).
  const p = settings.llm_profiles.find((x) => x.id === profileId) ?? settings.llm_profiles.find((x) => !x.external);
  if (!p) throw "no local LLM profile — pick a profile for this run (an external one asks for a confirmation first)";
  consume(consent, consentKey(id, r, question, p), p);
  if (item.recording) throw `'${id}' is still being recorded — run recipes when the session ends`;
  if (recipeRuns[id]) throw `“${recipeRuns[id].recipe.name}” is already running on this item — wait for it or cancel it`;
  if (p.external) externalSends[id] = [...new Set([...(externalSends[id] ?? []), hostOf(p)])];
  const run = { recipe: r, question, cancelled: false, step: null as { phase: string; done: number; total: number } | null };
  recipeRuns[id] = run;
  const base = { item_id: id, recipe_id: r.id, recipe_name: r.name, question };
  const prov = { profile: p.name, model: p.model, external: p.external, host: p.external ? hostOf(p) : "", answer_id: null as number | null };
  // Long items (transcriptions, meetings) go through map-reduce.
  const parts = item.meta.type === "note" ? 0 : Math.max(2, Math.ceil(item.segments.length / 3));
  const steps = parts ? [...Array.from({ length: parts + 1 }, (_, i) => ({ phase: "map", done: i, total: parts })), { phase: "reduce", done: 0, total: 1 }, { phase: "reduce", done: 1, total: 1 }] : [{ phase: "single", done: 0, total: 1 }, { phase: "single", done: 1, total: 1 }];
  let finished;
  for (const s of steps) {
    if (run.cancelled) break;
    run.step = s;
    ev("recipe-progress", { ...base, ...s });
    await new Promise((res) => setTimeout(res, 900));
  }
  delete recipeRuns[id];
  if (run.cancelled) {
    finished = { ...base, ...prov, file: null, answer: null, error: null, cancelled: true };
  } else if (r.target === "answer") {
    const text = fakeAnswer(question, item);
    const answerId = nextAnswer++;
    answers[answerId] = { itemId: id, recipe: r, question, profile: p, text, date: new Date().toISOString() };
    finished = { ...base, ...prov, answer_id: answerId, file: null, answer: text, error: null, cancelled: false };
  } else {
    const doc = companion(item, r, p, fakeResult(r, item));
    const list = (docs[id] ??= []);
    const idx = list.findIndex((d) => d.file === doc.file);
    if (idx >= 0) list[idx] = doc;
    else list.push(doc);
    list.sort((a, b) => Number(a.file !== "document.md") - Number(b.file !== "document.md") || a.file.localeCompare(b.file));
    finished = { ...base, ...prov, file: doc.file, answer: null, error: null, cancelled: false };
  }
  ev("recipe-finished", finished);
  return finished;
}

/* ---------- command table ---------- */

type Args = Record<string, unknown>;

function handle(cmd: string, a: Args): unknown {
  switch (cmd) {
    case "get_settings":
      return { ...settings };
    case "set_settings": {
      // Keys "go to the keychain" as with the backend's secrets::sync_keys
      // (#159). The mock never ships a key of its own: fake data holds no secret.
      const next = a.settings as Settings;
      Object.assign(settings, {
        ...next,
        // Only the token commands change it, as in the backend.
        extension_token: settings.extension_token,
        llm_profiles: next.llm_profiles.map((p) => ({ ...p, api_key_storage: p.api_key ? "keychain" : "none" })),
      });
      return { ...settings };
    }
    case "credential_store_status":
      return params.get("keychain") === "0"
        ? { available: false, name: "the Secret Service keyring", error: "no D-Bus session bus" }
        : { available: true, name: "the macOS Keychain", error: "" };
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
      const next = a.meta as ItemMeta;
      // Mirrors store::update_meta: notes never get participants (P10, #124).
      const participants = (next.participants ?? [])
        .map((p) => ({ name: p.name.trim(), ...(p.email?.trim() ? { email: p.email.trim() } : {}) }))
        .filter((p) => p.name);
      if (next.type === "note" && participants.length && JSON.stringify(participants) !== JSON.stringify(s.meta.participants))
        throw "notes have no participants — participants belong to meetings and transcriptions. Remove them, or change the item's type first.";
      s.meta = { ...next, participants };
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
    case "archive_export": {
      // Mirrors archive::export: subtitles are refused for notes (P10).
      const s = find(String(a.id));
      if (!s) throw `no archive item '${a.id}'`;
      const format = String(a.format);
      if ((format === "srt" || format === "vtt") && s.meta.type === "note")
        throw "notes have no subtitles — subtitles belong to meetings and transcriptions";
      const path = String(a.path);
      return path.toLowerCase().endsWith(`.${format}`) ? path : `${path}.${format}`;
    }
    case "archive_subtitles_status":
    case "archive_create_subtitles": {
      const s = find(String(a.id));
      if (!s) throw `no archive item '${a.id}'`;
      const applicable = s.meta.type !== "note";
      if (cmd === "archive_create_subtitles") {
        if (!applicable) throw "notes have no subtitles — subtitles belong to meetings and transcriptions";
        if (s.recording) throw `'${s.id}' is still being recorded — create subtitles when the session ends`;
        srtWritten.add(s.id);
      }
      return { file: "transcript.srt", applicable, exists: srtWritten.has(s.id), edited_externally: false };
    }
    case "export_history":
      return "Entries exported.";
    case "extension_token_get":
      if (!settings.extension_token) settings.extension_token = fakeToken();
      return settings.extension_token;
    case "extension_token_regenerate":
      settings.extension_token = fakeToken();
      return settings.extension_token;
    case "engine_status":
      return {
        active: (mic ? 1 : 0) + (fileRun ? 1 : 0) + (linkRun ? 1 : 0),
        mic_session: mic?.id ?? null,
        meeting_session: null,
        file_sessions: fileRun ? [{ session_id: fileRun.id, label: "mock.wav" }] : [],
        link_sessions: linkRun ? [{ session_id: linkRun.id, label: linkRun.label }] : [],
      };
    case "link_inspect":
      return linkInspect(String(a.url ?? ""));
    case "yt_dlp_status":
      return YT_DLP
        ? { found: true, path: "/opt/homebrew/bin/yt-dlp", version: "2025.09.26", install_help: "" }
        : { found: false, path: null, version: null, install_help: "Install yt-dlp with Homebrew: `brew install yt-dlp` (or `pipx install yt-dlp`). It is not bundled with Sussurro: video sites change often and yt-dlp is updated to follow them." };
    case "engine_start_link":
      return startLink(String(a.url), (a.title as string | null) ?? null, runLanguage(a));
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
    case "recipes_list":
      return allRecipes();
    case "recipe_documents":
      if (!find(String(a.id))) throw `no archive item '${a.id}'`;
      return (docs[String(a.id)] ?? []).map((d) => ({ ...d, meta: { ...d.meta } }));
    case "recipe_run":
      return runRecipe(String(a.id), String(a.recipeId), (a.profileId as string | null) ?? null, null, a.consent);
    case "recipe_ask":
      return runRecipe(String(a.id), "question", (a.profileId as string | null) ?? null, String(a.question ?? ""), a.consent);
    case "external_run_preview":
      return preview(a);
    case "prepare_external_run":
      return prepare(a);
    case "recipe_save_answer":
      return saveAnswer(String(a.id), Number(a.answerId));
    case "recipe_dismiss_answer": {
      const known = !!answers[Number(a.answerId)];
      delete answers[Number(a.answerId)];
      return known;
    }
    case "recipe_cancel": {
      const run = recipeRuns[String(a.id)];
      if (run) run.cancelled = true;
      return !!run;
    }
    case "recipe_status":
      return Object.entries(recipeRuns).map(([item_id, r]) => ({ item_id, recipe_id: r.recipe.id, recipe_name: r.recipe.name, question: r.question, progress: r.step }));
    case "recipe_reveal_document":
      console.info("[mock] reveal document", a.id, a.file);
      return null;
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
    case "plugin:dialog|save": {
      const o = (a.options ?? {}) as { defaultPath?: string };
      return `/Users/demo/Desktop/${o.defaultPath ?? "export"}`;
    }
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
