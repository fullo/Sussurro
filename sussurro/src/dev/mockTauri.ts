/* DEV ONLY — a fake Tauri backend so the UI can be opened with `npm run dev`
   in a normal browser (visual checks, screenshots). main.tsx loads this file
   only when `import.meta.env.DEV` is true AND the page is not running inside
   Tauri, through a dynamic import that production builds drop entirely.

   URL switches: ?onboarding=welcome (first-run setup) or =whats_new (the
   upgrade screen), ?perms=ask (microphone not asked yet, Accessibility
   denied — the setup's permission step), ?empty=1 (empty archive),
   ?ytdlp=0 (yt-dlp not installed, for the Link tab), ?keychain=0 (no OS
   credential store: the profile editor's clear-text key warning). */

import { mockIPC, mockWindows } from "@tauri-apps/api/mocks";
import { version as pkgVersion } from "../../package.json";
import { emit } from "@tauri-apps/api/event";
import { linkEmail, mergePreview, nameKey, parseAliases, personFor, personProblems } from "../lib/people";
import { DATE_BUCKETS, localToday, type DateBucket, type Facets, type FacetValue } from "../lib/facets";
import type { AudioFile, CompanionDoc, DocSpeaker, Item, ItemMeta, ItemSummary, LlmProfile, Person, Recipe, Segment, Settings } from "../lib/types";

const params = new URLSearchParams(window.location.search);

/** `?perms=ask`: the microphone counts as asked once a mic test ran. */
let micAsked = false;

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
  api_scripting: false,
  output_file: "",
  archive_dir: "",
  // #115: the preview opens on the workspace; `?onboarding=welcome` or
  // `=whats_new` shows the first-run setup or the upgrade screen.
  onboarding: ((o) => (o === "welcome" || o === "whats_new" ? o : "done"))(params.get("onboarding")),
  subtitles: "on_request",
  extension_token: "",
  save_audio: false,
  // #136: `?notice=seen` skips the recording notice.
  meeting_notice_seen: params.get("notice") === "seen",
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
  /** Speakers of the document (#130). */
  speakers?: DocSpeaker[];
  /** Voice each line "really" has, standing in for the stored embeddings:
   *  what "Re-detect speakers" gives back. */
  voiceOf?: Record<number, string>;
  /** The original file of a transcription, as `source-files.json` knows it
   *  (#134): absent = not recorded (made before #134, or a link). */
  sourceFile?: "available" | "missing" | "changed";
  /** Saved audio in the item folder (#141). */
  audio?: AudioFile[];
}

/** A run's saved audio (#141): 16 kHz 16-bit mono, 32 000 bytes/s. */
const mockAudio = (seconds: number, names = ["audio.wav"]): AudioFile[] =>
  names.map((name) => ({ name, bytes: 44 + Math.round(seconds) * 32_000 }));

/** Whether a run saves its audio: New's choice, else the per-app default. */
const runSavesAudio = (a: Args): boolean => (a.saveAudio as boolean | undefined) ?? !!settings.save_audio;

const VOICE_COLORS = ["#0f766e", "#7e22ce", "#1f6feb", "#c2410c", "#be185d", "#4d7c0f", "#0369a1", "#9a3412"];
const voice = (n: number): DocSpeaker => ({ id: `voice:${n}`, label: `Voice ${n}`, color: VOICE_COLORS[(n - 1) % VOICE_COLORS.length] });

/** The preview's stand-in for the tracker (#134): two voices taking turns. */
const mockVoiceOf = (segmentId: number) => `voice:${(segmentId % 2) + 1}`;

/** "Identify voices" on a stored transcription, as engine::identify does. */
function labelVoices(s: Stored) {
  const truth: Record<number, string> = {};
  for (const x of s.segments) if (!x.stt_error && x.text.trim()) truth[x.id] = mockVoiceOf(x.id);
  s.segments = s.segments.map((x) => (truth[x.id] ? { ...x, speaker_id: truth[x.id] } : x));
  const used = [...new Set(Object.values(truth))].sort();
  s.speakers = used.map((id) => voice(Number(id.split(":")[1])));
  s.voiceOf = truth;
}

/** Mirrors engine::identify::availability (#134). */
function voiceSource(s: Stored) {
  const file = s.meta.source.startsWith("file:") ? s.meta.source.slice(5) : "";
  const no = (reason: string) => ({ available: false, reason, file_name: file });
  if (s.meta.type === "note") return no("Notes are your own voice: they have no speakers.");
  if (s.meta.type === "meeting") return no("Meetings get their voices while they are recorded.");
  if (s.recording) return no("Available when the recording ends.");
  if (s.edited_externally) return no("The transcript was edited outside Sussurro.");
  if (s.voiceOf) return no("This transcription already has voice data: use Re-detect speakers.");
  if (s.meta.source.startsWith("url:"))
    return no("Voices are found in the audio, and a link's download is deleted once it is transcribed (downloading it again is not supported yet). Transcribe the link again with Identify voices on.");
  if (!file) return no("The original audio of this transcription is not available.");
  switch (s.sourceFile) {
    case "available":
      return { available: true, reason: "", file_name: file };
    case "missing":
      return no(`The original file “${file}” is no longer where it was transcribed from. Transcribe it again with Identify voices on.`);
    case "changed":
      return no(`The file “${file}” changed since it was transcribed, so its voices would not match the lines. Transcribe it again with Identify voices on.`);
    default:
      return no(`Sussurro doesn't know where “${file}” is: it was transcribed before voices could be identified later, or on another computer. Transcribe it again with Identify voices on.`);
  }
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
        meta: meta("Idee per l'onboarding", "note", at(0, 8, 40), "00:03:12", "mic", { tags: ["onboarding", "idee", "release"], categories: ["prodotto"] }),
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
          tags: ["podcast", "privacy"], categories: ["daruma"],
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
        // Its file is still on disk: the speaker panel offers "Identify voices".
        sourceFile: "available",
        // Saved with "Save audio" on (#141): the audio bar and Library marker.
        audio: mockAudio(48 * 60 + 10),
      },
      {
        id: "2026/09/call-con-studio-verdi",
        meta: meta("Call con Studio Verdi", "meeting", at(2, 11, 0), "00:38:02", "mic", {
          tags: ["Release", "roadmap"], categories: ["clienti"],
          participants: [{ name: "Anna Rossi", email: "anna@example.com" }, { name: "Marco Bianchi", email: "marco@example.com" }, { name: "Voice 1" }],
        }),
        segments: segs([
          "Ok, partiamo dalla release 0.7: archivio e schermata New.",
          "La cartella in Documenti funziona su tutti e tre i sistemi, manca il test con OneDrive.",
          "Ho importato un file da un'ora: la memoria resta piatta, nessun picco.",
        ]),
        edited_externally: true,
      },
      (() => {
        const lines = segs([
          "Allora, siamo tutti? Partiamo dalla roadmap della 0.9.",
          "Io vorrei chiudere prima l'estensione del browser, il resto dipende da lì.",
          "D'accordo, ma le etichette delle voci servono anche per le riunioni in sala.",
          "Giusto: con un solo portatile al centro del tavolo non abbiamo i nomi di Meet.",
          "Quindi Voice 1, Voice 2, e poi si rinominano nel documento.",
          "Esatto. E se il raggruppamento sbaglia, si sposta la riga a mano.",
        ]);
        const truth = ["voice:1", "voice:2", "voice:3", "voice:1", "voice:2", "voice:3"];
        return {
          id: "2026/09/riunione-in-sala-roadmap-0-9",
          meta: meta("Riunione in sala — roadmap 0.9", "meeting", at(1, 10, 0), "00:12:40", "mic", { categories: ["team"] }),
          segments: lines.map((l, i) => ({ ...l, speaker_id: i === 5 ? "voice:2" : truth[i] })),
          speakers: [voice(1), { ...voice(2), label: "Anna" }, voice(3)],
          voiceOf: Object.fromEntries(truth.map((v, i) => [i, v])),
          // One channel, saved (#141): the Audio tab's "Play only" (#142).
          audio: mockAudio(12 * 60 + 40),
        } as Stored;
      })(),
      (() => {
        // A browser meeting with both channels saved (#141/#142): "You" on
        // the mic file, Meet names and a voice on the remote one, with
        // word timings and one overlap across channels.
        const turns: [string, "mic" | "remote", number, number, string][] = [
          ["you", "mic", 1000, 5200, "Buongiorno a tutti, iniziamo con lo stato della release."],
          ["meet:Anna Rossi", "remote", 5600, 10_400, "La build per macOS è pronta, manca solo la firma."],
          ["meet:Anna Rossi", "remote", 10_600, 13_800, "Windows invece è ancora in coda."],
          ["you", "mic", 13_500, 16_000, "Perfetto, grazie Anna."],
          ["voice:1", "remote", 17_000, 22_500, "Io ho una domanda sul formato dei file audio salvati."],
          ["you", "mic", 23_000, 28_400, "Sono WAV mono a sedici kilohertz, uno per canale."],
          ["meet:Anna Rossi", "remote", 29_000, 33_000, "E si possono riascoltare per persona?"],
          ["you", "mic", 33_500, 38_000, "Sì, dalla scheda Audio, scegliendo chi ascoltare."],
        ];
        const timed = (text: string, start: number, end: number) => {
          const ws = text.split(/\s+/);
          const step = (end - start) / ws.length;
          return ws.map((w, i) => ({ w, start_ms: Math.round(start + i * step), end_ms: Math.round(start + (i + 0.85) * step) }));
        };
        return {
          id: "2026/09/weekly-sync-release",
          meta: meta("Weekly sync — release", "meeting", at(0, 9, 30), "00:00:40", "browser:meet.google.com", {
            categories: ["team"],
            participants: [{ name: "Anna Rossi", email: "anna@example.com" }, { name: "Voice 1" }],
          }),
          segments: turns.map(([speaker_id, channel, start_ms, end_ms, text], id) => ({
            id, channel, start_ms, end_ms, speaker_id, raw: text, text,
            // Line 3 was rewritten by cleanup: no word highlighting there.
            ...(id === 3 ? { text: "Perfetto, grazie mille Anna." } : {}),
            words: timed(text, start_ms, end_ms),
          })),
          speakers: [
            { id: "you", label: "You", color: "#1a1a1a" },
            { id: "meet:Anna Rossi", label: "Anna Rossi", color: VOICE_COLORS[VOICE_COLORS.length - 1] },
            voice(1),
          ],
          audio: mockAudio(40, ["audio-mic.wav", "audio-remote.wav"]),
        } as Stored;
      })(),
      {
        id: "2026/09/lezione-diritto-d-autore-e-ia",
        meta: meta("Lezione: diritto d'autore e IA — una lezione molto lunga con un titolo lunghissimo", "transcription", at(5, 9, 30), "01:12:40", "file:lezione.m4a", { tags: ["copyright", "privacy"], categories: ["studio"], participants: [{ name: "Prof. Neri" }] }),
        segments: segs(["Oggi vediamo come il diritto d'autore si applica ai modelli generativi.", "Partiamo dalla direttiva europea sul copyright nel mercato unico digitale."], 30000),
        interrupted: true,
      },
      {
        id: "2026/09/appunti-treno-per-bologna",
        meta: meta("Appunti treno per Bologna", "note", at(6, 7, 55), "00:02:04", "mic", { tags: ["idee"], categories: ["prodotto"] }),
        segments: segs(["Ricordarsi di prenotare il ritorno.", "Scrivere il post sul blog per la 0.7 durante il viaggio."]),
      },
      {
        id: "2025/12/lista-regali",
        meta: meta("Lista regali di Natale", "note", "2025-12-11T19:20:00+01:00", "00:01:02", "mic", { tags: ["personale"] }),
        segments: segs(["Un libro per papà, i colori per Giulia, e qualcosa per il gatto."]),
      },
    ];

const find = (id: string) => items.find((i) => i.id === id);

/* ---------- People registry (#132) ---------- */

let people: Person[] = params.get("empty")
  ? []
  : [
      { id: "p-anna", name: "Anna Rossi", email: "anna@example.com", aliases: ["Anna R.", "Annie"] },
      { id: "p-marco", name: "Marco Bianchi", email: "marco@example.com", aliases: [] },
      { id: "p-francesco", name: "Francesco Fullone", email: "francesco@example.com", aliases: ["Fullo"] },
      { id: "p-giulia", name: "Giulia Verdi", aliases: [] },
      // A likely duplicate of Anna, to show the merge hint.
      { id: "p-anna2", name: "Anna R.", email: "a.rossi@studio.example", aliases: [] },
    ];
let personSeq = 0;

function cleanPerson(p: Person): Person {
  const name = p.name.trim().replace(/\s+/g, " ");
  const email = p.email?.trim() || undefined;
  const problems = personProblems({ ...p, name, email }, people);
  if (problems.length) throw problems[0];
  return { id: p.id, name, ...(email ? { email } : {}), aliases: parseAliases(p.aliases.join("\n"), name) };
}

function sortedPeople(): Person[] {
  return people.slice().sort((a, b) => nameKey(a.name).localeCompare(nameKey(b.name)));
}

function peopleUsage(): Record<string, number> {
  const out: Record<string, number> = Object.fromEntries(people.map((p) => [p.id, 0]));
  for (const it of items)
    for (const id of new Set(it.meta.participants.map((pt) => personFor(people, pt)?.id).filter(Boolean) as string[])) out[id]++;
  return out;
}

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
    segments: { version: 1, speakers: (s.speakers ?? []).map((x) => ({ ...x })), segments: s.segments.map((x) => ({ ...x })) },
    body,
    edited_externally: !!s.edited_externally,
    recording: !!s.recording,
    interrupted: !!s.interrupted,
    external_hosts: hostsOf(s.id),
    embedded_segments: s.voiceOf ? Object.keys(s.voiceOf).length : 0,
    audio: (s.audio ?? []).map((f) => ({ ...f })),
    folder_bytes: 4_096 + body.length + (s.audio ?? []).reduce((n, f) => n + f.bytes, 0),
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
    audio_bytes: (s.audio ?? []).reduce((n, f) => n + f.bytes, 0),
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

/* ---------- Library facets (#135), mirroring archive/facets.rs ---------- */

interface MockFilters {
  type?: string;
  types?: string[];
  tags?: string[];
  categories?: string[];
  participants?: string[];
  date_bucket?: DateBucket;
  date_from?: string;
  date_to?: string;
  today?: string;
}

type FacetName = "type" | "tag" | "category" | "participant" | "date";

const valueKey = (v: string) => v.trim().split(/\s+/).join(" ").toLowerCase();
const participantKey = (p: { name: string; email?: string }) => {
  const who = personFor(people, p);
  return who ? `person:${who.id}` : `name:${nameKey(p.name)}`;
};
/** The item's local day (the real index uses the day written in the
 *  frontmatter, which is the local day where it was recorded). */
const dayOf = (s: Stored) => localToday(new Date(s.meta.date));

function inBucket(day: string, b: DateBucket, today: string): boolean {
  const [y, m, d] = today.split("-").map(Number);
  const monday = localToday(new Date(y, m - 1, d - ((new Date(y, m - 1, d).getDay() + 6) % 7)));
  const from = { today, week: monday, month: `${today.slice(0, 7)}-01`, year: `${today.slice(0, 4)}-01-01`, older: "" }[b];
  return b === "older" ? day < `${today.slice(0, 4)}-01-01` : day >= from && day <= today;
}

function passes(s: Stored, f: MockFilters, today: string, skip?: FacetName): boolean {
  const any = (list: string[] | undefined, have: string[]) => !list?.length || list.some((k) => have.includes(k));
  if (skip !== "type" && ((f.type && s.meta.type !== f.type) || !any(f.types, [s.meta.type]))) return false;
  if (skip !== "tag" && !any(f.tags?.map(valueKey), s.meta.tags.map(valueKey))) return false;
  if (skip !== "category" && !any(f.categories?.map(valueKey), s.meta.categories.map(valueKey))) return false;
  if (skip !== "participant" && !any(f.participants, s.meta.participants.map(participantKey))) return false;
  if (skip !== "date") {
    const day = dayOf(s);
    if (f.date_bucket && !inBucket(day, f.date_bucket, today)) return false;
    if (f.date_from && day < f.date_from) return false;
    if (f.date_to && day > f.date_to) return false;
  }
  return true;
}

/** Distinct items per value key, most first; label = min() of the spellings. */
function grouped(list: Stored[], values: (s: Stored) => { key: string; label: string }[]): FacetValue[] {
  const out = new Map<string, FacetValue>();
  for (const s of list) {
    const seen = new Set<string>();
    for (const v of values(s)) {
      if (seen.has(v.key)) continue;
      seen.add(v.key);
      const cur = out.get(v.key);
      if (!cur) out.set(v.key, { ...v, count: 1 });
      else {
        cur.count++;
        if (v.label < cur.label) cur.label = v.label;
      }
    }
  }
  return [...out.values()].sort((a, b) => b.count - a.count || nameKey(a.label).localeCompare(nameKey(b.label)));
}

function facetSearch(query: string, f: MockFilters): { items: ItemSummary[]; facets: Facets } {
  const today = f.today || localToday();
  const hits = new Map(search(query).map((h) => [h.id, h]));
  const matched = items.filter((s) => hits.has(s.id)).sort(newestFirst);
  const base = (skip?: FacetName) => matched.filter((s) => passes(s, f, today, skip));
  const found = base().map((s) => hits.get(s.id) as ItemSummary);
  const byType = base("type");
  const byDate = base("date");
  const personName = (v: FacetValue) => ({ ...v, label: people.find((p) => `person:${p.id}` === v.key)?.name ?? v.label });
  return {
    items: found,
    facets: {
      total: found.length,
      types: (["note", "meeting", "transcription"] as const).map((t) => ({ key: t, label: t, count: byType.filter((s) => s.meta.type === t).length })),
      tags: grouped(base("tag"), (s) => s.meta.tags.map((t) => ({ key: valueKey(t), label: t }))),
      categories: grouped(base("category"), (s) => s.meta.categories.map((c) => ({ key: valueKey(c), label: c }))),
      participants: grouped(base("participant"), (s) => s.meta.participants.map((p) => ({ key: participantKey(p), label: p.name }))).map(personName),
      dates: DATE_BUCKETS.map((b) => ({ key: b.value, label: b.label, count: byDate.filter((s) => inBucket(dayOf(s), b.value, today)).length })),
    },
  };
}

/* ---------- engine simulation ---------- */

let nextSession = 1;
/** The live capture: a mic session, or System audio + mic (#139) when
 *  `system` names the loopback device. */
let mic: { id: number; itemId: string; timer: number; n: number; started: number; system?: string; saveAudio?: boolean } | null = null;
const SYSTEM_DEVICES = {
  default_input: "MacBook Pro Microphone",
  devices: [
    { name: "BlackHole 2ch", loopback: true },
    { name: "MacBook Pro Microphone", loopback: false },
    { name: "USB Audio Device", loopback: false },
  ],
  // #140: a Mac on 14.2+ (process taps); `?native=off` shows the fallback
  // to the device picker (a Mac below 14.2).
  native:
    params.get("native") === "off"
      ? {
          available: false,
          backend: "coreaudio-tap",
          detail: null,
          reason:
            "recording the computer's sound directly needs macOS 14.2 or later (this Mac runs 13.6) — use a loopback device such as BlackHole",
          needs_permission: false,
        }
      : { available: true, backend: "coreaudio-tap", detail: "MacBook Pro Speakers", reason: null, needs_permission: true },
};
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
  if (mic.system) {
    // System audio + mic: "You" on the mic channel, voices on the system one.
    seg.channel = i % 2 === 0 ? "mic" : "system";
    seg.speaker_id = i % 2 === 0 ? "you" : `voice:${1 + ((i >> 1) % 2)}`;
  }
  const s = find(mic.itemId);
  if (s) s.segments.push(seg);
  ev("engine-segment", { session_id: mic.id, segment: seg });
  if (mic.system && i === 3)
    ev("engine-warning", {
      session_id: mic.id,
      message: "The microphone and the system audio device drifted 0.21 s apart — realigned at 0:18 (silence added to the system audio device channel).",
    });
  const t = (Date.now() - mic.started) / 1000;
  ev("engine-progress", { session_id: mic.id, processed_s: i * 6 + 5, ingested_s: t, total_s: t, backlog_s: Math.max(0, t - (i * 6 + 5)), queue_len: 0, segments_done: i + 1 });
}

/** A run's language (#157): the one chosen in New, else the settings'. The
 *  per-run cleanup level only changes the (fake) text, so it is just logged. */
function runLanguage(a: Args): string {
  if (a.cleanupLevel) console.info("[mock] run cleanup level", a.cleanupLevel);
  return (a.language as string | null) || settings.language;
}

function startMic(title: string | null, language: string, saveAudio = false): number {
  const id = nextSession++;
  const itemId = `2026/09/${new Date().toISOString().slice(0, 10)}-untitled`;
  items.push({ id: itemId, meta: meta(title || "Untitled", "note", new Date().toISOString(), "", "mic", { language }), segments: [], recording: true });
  mic = { id, itemId, n: 0, started: Date.now(), timer: window.setInterval(micTick, 2500), saveAudio };
  setTimeout(() => ev("engine-started", { session_id: id, item_id: itemId, item_type: "note", title: title ?? "", source: "mic" }), 50);
  return id;
}

/** New → System audio + mic (#139): a meeting from the mic and a loopback
 *  device. */
function startSystem(a: Args, language: string): number {
  if (mic) throw mic.system ? "a system audio session is already running" : "a microphone session is running — stop it first";
  const native = !!a.native;
  if (native && !SYSTEM_DEVICES.native.available)
    throw `This computer's sound (built-in) is unavailable: ${SYSTEM_DEVICES.native.reason}`;
  const device = native ? "This computer's sound (built-in)" : String(a.systemDevice ?? "");
  const micDevice = (a.micDevice as string | null) || settings.input_device || SYSTEM_DEVICES.default_input;
  if (!device) throw "choose the system audio device";
  if (device === micDevice) throw "the system audio device is the microphone — choose a different device for one of them";
  const title = (a.title as string | null) ?? null;
  const id = nextSession++;
  const itemId = `2026/09/${new Date().toISOString().slice(0, 10)}-untitled`;
  items.push({
    id: itemId,
    meta: meta(title || "Untitled", "meeting", new Date().toISOString(), "", "system", { language }),
    segments: [],
    speakers: [
      { id: "you", label: "You", color: "#1f6f5c" },
      { id: "voice:1", label: "Voice 1", color: "#8a5a00" },
      { id: "voice:2", label: "Voice 2", color: "#5b4a9e" },
    ],
    recording: true,
  });
  mic = { id, itemId, n: 0, started: Date.now(), timer: window.setInterval(micTick, 2500), system: device, saveAudio: runSavesAudio(a) };
  setTimeout(() => ev("engine-started", { session_id: id, item_id: itemId, item_type: "meeting", title: title ?? "", source: "system" }), 50);
  return id;
}

function stopMic(system = false): number {
  if (!mic || !!mic.system !== system) throw system ? "no system audio session is running" : "no microphone session is running";
  const m = mic;
  clearInterval(m.timer);
  mic = null;
  setTimeout(() => {
    const s = find(m.itemId);
    if (!s) return;
    s.recording = false;
    const title = s.meta.title === "Untitled" ? "Allora, provo a registrare una nota lunga" : s.meta.title;
    s.meta.title = title;
    s.id = m.itemId.replace("untitled", m.system ? "riunione-audio-di-sistema" : "nota-dal-microfono");
    s.meta.duration = `00:00:${String(Math.round((Date.now() - m.started) / 1000) % 60).padStart(2, "0")}`;
    if (m.saveAudio) s.audio = mockAudio((Date.now() - m.started) / 1000, m.system ? ["audio-mic.wav", "audio-system.wav"] : undefined);
    ev("engine-done", { session_id: m.id, item_id: s.id, item_type: s.meta.type, title, text: "", segments: s.segments.length, duration_s: (Date.now() - m.started) / 1000 });
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

async function transcribeFile(
  path: string,
  itemType: "note" | "transcription",
  title: string | null,
  language: string,
  identify: boolean,
  saveAudio = false,
) {
  // Notes never get voices (P10), whatever the toggle said (#134).
  const voices = identify && itemType === "transcription";
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
    const seg: Segment = { id: i, start_ms: i * 24000, end_ms: i * 24000 + 23000, raw: FAKE_LINES[i % 5], text: FAKE_LINES[i % 5], ...(voices ? { speaker_id: mockVoiceOf(i) } : {}) };
    find(itemId)?.segments.push(seg);
    ev("engine-segment", { session_id: id, segment: seg });
    ev("engine-progress", { session_id: id, processed_s: (i + 1) * 24, ingested_s: Math.min(total, (i + 2) * 24), total_s: total, backlog_s: 24, queue_len: 1, segments_done: i + 1 });
  }
  const s = find(itemId)!;
  s.recording = false;
  s.meta.duration = "00:03:12";
  if (saveAudio) s.audio = mockAudio(total);
  if (voices) labelVoices(s);
  // A transcription's file path is remembered on this machine (#134).
  if (itemType === "transcription") s.sourceFile = "available";
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

function startLink(url: string, title: string | null, language: string, identify: boolean, saveAudio = false): number {
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
      const seg: Segment = { id: i, start_ms: i * 24000, end_ms: i * 24000 + 23000, raw: FAKE_LINES[i % 5], text: FAKE_LINES[i % 5], ...(identify ? { speaker_id: mockVoiceOf(i) } : {}) };
      find(itemId)?.segments.push(seg);
      ev("engine-segment", { session_id: id, segment: seg });
      ev("engine-progress", { session_id: id, processed_s: (i + 1) * 24, ingested_s: Math.min(144, (i + 2) * 24), total_s: 144, backlog_s: 24, queue_len: 1, segments_done: i + 1 });
    }
    const s = find(itemId)!;
    s.recording = false;
    s.meta.duration = "00:02:24";
    if (saveAudio) s.audio = mockAudio(144);
    if (identify) labelVoices(s);
    linkRun = null;
    ev("engine-done", { session_id: id, item_id: itemId, item_type: "transcription", title: name, text: "", segments: 6, duration_s: 144 });
  })();
  return id;
}

/* ---------- LLM profiles (#119) ---------- */

/** Fake model listing per profile: Ollama and localhost servers answer,
 *  "*.example.com" hosts behave as unreachable (to preview the error). */
/* ---------- the bundled LLM (#118) ---------- */

/** `?bundled=missing`: its model isn't downloaded; `?sidecar=0`: a build
 *  without the llama-server sidecar; `?ollama=down`: the local Ollama
 *  can't be reached (the "Use the bundled model" offer). */
const bundled = {
  available: params.get("sidecar") !== "0",
  downloaded: params.get("bundled") !== "missing",
};
const BUNDLED_PROFILE: LlmProfile = {
  id: "bundled",
  name: "Local (bundled)",
  api: "openai",
  base_url: "http://127.0.0.1",
  api_key: "",
  model: "qwen3-1.7b",
  external: false,
  context_tokens: 8192,
  bundled: true,
};
if (bundled.available) settings.llm_profiles.push({ ...BUNDLED_PROFILE });
/** `?servers=none`: no LLM server answers at all (the onboarding's cleanup
 *  step then offers the bundled model). */
const noServers = params.get("servers") === "none";
const ollamaDown = noServers || params.get("ollama") === "down";

function bundledStatus() {
  return { ...bundled, running: false, model: "Qwen3 1.7B", download_bytes: 2_165_039_200 };
}

async function bundledDownload(): Promise<void> {
  if (!bundled.available) throw "this build has no bundled llama-server";
  if (!bundled.downloaded) await new Promise((r) => setTimeout(r, 1500));
  bundled.downloaded = true;
}

function listModels(p: LlmProfile | undefined): string[] {
  if (!p) throw "no profile";
  if (p.bundled) {
    if (!bundled.available) throw "this build has no bundled llama-server";
    if (!bundled.downloaded) throw "the bundled model is not downloaded yet";
    return ["qwen3-1.7b"];
  }
  if (ollamaDown && p.api === "ollama") throw "ollama not reachable";
  if (noServers) throw "server not reachable";
  if (/example\.com/.test(p.base_url)) throw "OpenAI-compatible server not reachable";
  // The onboarding's probe of llama.cpp-server's default port (#115): not running.
  if (/localhost:8080/.test(p.base_url)) throw "connection refused";
  return p.api === "ollama" ? ["llama3.2:3b", "qwen2.5:3b"] : ["qwen2.5-7b-instruct", "gemma-3-4b-it"];
}

/* ---------- recipes (#120) ---------- */

const BUILTIN_RECIPES: Recipe[] = [
  { id: "formatted-document", name: "Formatted document", prompt: "Turn the transcript into a well-structured written document in markdown: a **tl;dr:** line, a # title, ## to ###### headings, tables where the content is tabular.", target: "companion_document", builtin: true },
  { id: "summary", name: "Summary", prompt: "Summarise the transcript in markdown: a short paragraph with the gist, then the key points as a bullet list.", target: "companion_document", builtin: true },
  { id: "action-items", name: "Action items", prompt: "List every action item in the transcript as a markdown task list (`- [ ] …`), with owner and deadline when said.", target: "companion_document", builtin: true },
  { id: "decisions", name: "Decisions", prompt: "List the decisions taken in the transcript as a markdown bullet list, each with its reason.", target: "companion_document", builtin: true },
  // #143: only where the transcript names its speakers.
  { id: "meeting-minutes", name: "Meeting minutes", prompt: "Write the minutes of this meeting: Attendees, Agenda, Discussion, Decisions (who decided), Action items as a table Action | Owner | Due.", target: "companion_document", builtin: true, speakers_only: true },
  { id: "who-said-what", name: "Who said what", prompt: "For each speaker, a ## heading with their name, then what they said, with timestamps.", target: "companion_document", builtin: true, speakers_only: true },
];

/** Speaker labels the item's lines carry (#143), in order. */
function mockSpeakers(item: Stored): string[] {
  if (item.meta.type === "note") return [];
  const out: string[] = [];
  for (const seg of item.segments) {
    const label = (item.speakers ?? []).find((x) => x.id === seg.speaker_id)?.label.trim();
    if (label && seg.text.trim() && !out.includes(label)) out.push(label);
  }
  return out;
}

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

function consentKey(id: string, r: Recipe, question: string | null, p: LlmProfile, emails = false): string {
  return [id, r.id, question ?? "", p.id, hostOf(p), p.model, emails ? "emails" : "names"].join("\u0000");
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
  const people = item.meta.participants.filter((x) => x.name.trim());
  const emails = people.filter((x) => (x.email ?? "").trim()).length;
  return {
    speakers: mockSpeakers(item),
    participants: people.length,
    emails_available: emails,
    emails_sent: a.includeEmails ? emails : 0,
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
  consents[token] = { key: consentKey(item.id, recipe, question, profile, !!a.includeEmails), at: Date.now() };
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

async function runRecipe(id: string, recipeId: string, profileId: string | null, question: string | null = null, consent: unknown = null, includeEmails = false) {
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
  consume(consent, consentKey(id, r, question, p, includeEmails), p);
  if (item.recording) throw `'${id}' is still being recorded — run recipes when the session ends`;
  if (r.speakers_only && !mockSpeakers(item).length)
    throw `“${r.name}” needs a meeting or transcription whose transcript names its speakers — this one has none`;
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
    case "stt_sidecar_available":
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
    case "ollama_status": {
      // As the backend: whether the cleanup profile's server lists its model.
      const p = settings.llm_profiles.find((x) => x.id === settings.cleanup_profile) ?? settings.llm_profiles[0];
      try {
        const models = listModels(p);
        return { installed: true, running: true, has_model: models.includes(p.model) || p.api === "ollama" };
      } catch {
        return { installed: !ollamaDown, running: false, has_model: false };
      }
    }
    case "bundled_llm_status":
      return bundledStatus();
    case "bundled_llm_download":
      return bundledDownload();
    case "bundled_llm_use":
      return bundledDownload().then(() => {
        if (!settings.llm_profiles.some((p) => p.bundled)) settings.llm_profiles.push({ ...BUNDLED_PROFILE });
        settings.cleanup_profile = "bundled";
        return { ...settings };
      });
    case "check_permissions":
      return params.get("perms") === "ask" && !micAsked
        ? { microphone: "unknown", accessibility: "denied" }
        : { microphone: "granted", accessibility: params.get("perms") === "ask" ? "denied" : "granted" };
    case "mic_level":
      return 0.02 + Math.random() * 0.05;
    case "start_mic_test":
      // The first mic access is what asks the OS (#115, ?perms=ask).
      micAsked = true;
      return null;
    case "stop_mic_test":
    case "trigger_dictation":
    case "copy_text":
    case "open_settings":
      return null;
    case "diagnostics":
      return "Sussurro dev preview";
    case "archive_dir":
      return settings.archive_dir || ARCHIVE;
    case "archive_prepare":
      // The real command creates the folder (the macOS Documents prompt, #115).
      return settings.archive_dir || ARCHIVE;
    case "archive_list":
      return items.slice().sort(newestFirst).map((s) => toSummary(s));
    case "archive_search":
      return facetSearch(String(a.query ?? ""), (a.filters ?? {}) as MockFilters).items;
    case "archive_facets":
      return facetSearch(String(a.query ?? ""), (a.filters ?? {}) as MockFilters);
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
      // Mirrors people::link_on_save (#132): new participants get the email.
      if (next.type !== "note") {
        const known = new Set(s.meta.participants.map((p) => nameKey(p.name)));
        for (const p of participants) {
          const email = known.has(nameKey(p.name)) ? null : linkEmail(people, p);
          if (email) Object.assign(p, { email });
        }
      }
      s.meta = { ...next, participants };
      return toItem(s);
    }
    case "people_list":
      return sortedPeople();
    case "people_usage":
      return peopleUsage();
    case "people_add": {
      const p = cleanPerson({ ...(a.person as Person), id: "" });
      p.id = `p-new${++personSeq}`;
      people.push(p);
      return p;
    }
    case "people_update": {
      const input = a.person as Person;
      const at = people.findIndex((p) => p.id === input.id);
      if (at < 0) throw "that person is no longer in People";
      people[at] = cleanPerson(input);
      return people[at];
    }
    case "people_delete": {
      if (!people.some((p) => p.id === a.id)) throw "that person is no longer in People";
      people = people.filter((p) => p.id !== a.id);
      return null;
    }
    case "people_merge": {
      const into = people.find((p) => p.id === a.into);
      const from = people.filter((p) => (a.from as string[]).includes(p.id) && p.id !== a.into);
      if (!into || !from.length) throw "that person is no longer in People";
      const merged = mergePreview(into, from);
      people = people.filter((p) => !from.includes(p)).map((p) => (p.id === into.id ? merged : p));
      return merged;
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
    case "archive_move_segment_speaker":
    case "archive_rename_speaker":
    case "archive_link_speaker":
    case "archive_unlink_speaker":
    case "archive_redetect_speakers": {
      // Mirrors archive::edit_speakers (#130), simplified.
      const s = find(String(a.id));
      if (!s) throw `no archive item '${a.id}'`;
      if (s.edited_externally) throw `'${s.id}' was edited outside Sussurro: its transcript.md is kept as is — edit it there`;
      if (s.recording) throw `'${s.id}' is still being recorded — edit it when the session ends`;
      const speakers = (s.speakers ??= []);
      if (cmd === "archive_move_segment_speaker") {
        let to = String(a.speakerId);
        if (to === "voice:new") {
          const max = Math.max(0, ...speakers.map((x) => Number(x.id.split(":")[1]) || 0));
          speakers.push(voice(max + 1));
          to = `voice:${max + 1}`;
        } else if (!speakers.some((x) => x.id === to)) throw `no speaker '${to}' in this document`;
        s.segments = s.segments.map((x) => (x.id === Number(a.segmentId) ? { ...x, speaker_id: to } : x));
      } else if (cmd === "archive_rename_speaker") {
        const sp = speakers.find((x) => x.id === a.speakerId);
        if (!sp) throw `no speaker '${a.speakerId}' in this document`;
        const label = String(a.label).trim().replace(/\s+/g, " ");
        sp.label = label || sp.id.replace("voice:", "Voice ");
        delete sp.label_before_link;
      } else if (cmd === "archive_link_speaker") {
        // Mirrors speakers::doc::link_speaker (#130), simplified.
        const sp = speakers.find((x) => x.id === a.speakerId);
        if (!sp) throw `no speaker '${a.speakerId}' in this document`;
        const person = people.find((x) => x.id === a.personId);
        if (!person) throw "that person is no longer in People";
        const old = sp.label;
        const generic = /^voice \d+$|^you$/i.test(old.trim());
        sp.person_id = person.id;
        if (generic) {
          sp.label_before_link = old;
          sp.label = person.name;
        }
        const list = s.meta.participants;
        const at = list.findIndex((pt) => nameKey(pt.name) === nameKey(person.name) || nameKey(pt.name) === nameKey(old));
        if (at < 0) list.push(person.email ? { name: person.name, email: person.email } : { name: person.name });
        else list[at] = { name: nameKey(list[at].name) === nameKey(old) ? person.name : list[at].name, email: list[at].email || person.email };
      } else if (cmd === "archive_unlink_speaker") {
        const sp = speakers.find((x) => x.id === a.speakerId);
        if (!sp) throw `no speaker '${a.speakerId}' in this document`;
        delete sp.person_id;
        if (sp.label_before_link) sp.label = sp.label_before_link;
        delete sp.label_before_link;
      } else {
        const truth = s.voiceOf;
        if (!truth) throw "this document has no voice data — speakers can only be detected on recordings made with speaker detection on";
        s.segments = s.segments.map((x) => (truth[x.id] ? { ...x, speaker_id: truth[x.id] } : x));
        const used = new Set(s.segments.map((x) => x.speaker_id));
        s.speakers = speakers.filter((x) => used.has(x.id));
      }
      return toItem(s);
    }
    case "archive_voice_map": {
      // Mirrors speakers::map::voice_map (#144), faked: each "true" voice is
      // a cloud around its own spot, with a fixed jitter per line.
      const s = find(String(a.id));
      if (!s) throw `no archive item '${a.id}'`;
      const truth = s.voiceOf ?? {};
      const ids = Object.keys(truth).map(Number);
      const voices = [...new Set(Object.values(truth))].sort();
      const jitter = (n: number) => ((Math.sin(n * 12.9898) * 43758.5453) % 1) * 0.35;
      const points = s.segments
        .filter((x) => ids.includes(x.id))
        .map((x) => {
          const k = voices.indexOf(truth[x.id]);
          const angle = (2 * Math.PI * k) / Math.max(1, voices.length) + 0.4;
          return {
            segment_id: x.id,
            speaker_id: x.speaker_id ?? null,
            x: Math.cos(angle) + jitter(x.id + 1),
            y: Math.sin(angle) + jitter(x.id + 101),
          };
        });
      return { points, total: points.length, explained: points.length > 1 ? 0.31 : 0 };
    }
    case "archive_voice_source": {
      const s = find(String(a.id));
      if (!s) throw `no archive item '${a.id}'`;
      return voiceSource(s);
    }
    case "archive_identify_voices": {
      // Mirrors engine::identify (#134), simplified.
      const s = find(String(a.id));
      if (!s) throw `no archive item '${a.id}'`;
      const v = voiceSource(s);
      if (!v.available) throw v.reason;
      return new Promise((r) =>
        setTimeout(() => {
          labelVoices(s);
          r(toItem(s));
        }, 1200),
      );
    }
    case "archive_delete":
      items = items.filter((s) => s.id !== a.id);
      return null;
    case "archive_delete_audio": {
      // "Delete audio, keep transcript" (#141): only the audio goes.
      const s = find(String(a.id));
      if (!s) throw `no archive item '${a.id}'`;
      if (s.recording) throw `'${s.id}' is still being recorded — stop the session before deleting its audio`;
      delete s.audio;
      return toItem(s);
    }
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
    case "local_api_status":
      // As if Sussurro started with the current settings.
      return settings.api_enabled ? { state: "listening", port: settings.api_port } : { state: "off" };
    case "engine_status":
      return {
        active: (mic ? 1 : 0) + (fileRun ? 1 : 0) + (linkRun ? 1 : 0),
        mic_session: mic && !mic.system ? mic.id : null,
        system_session: mic?.system ? mic.id : null,
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
      return startLink(String(a.url), (a.title as string | null) ?? null, runLanguage(a), !!a.identifyVoices, runSavesAudio(a));
    case "engine_start_mic":
      if (mic) throw "a microphone session is already running";
      return startMic((a.title as string | null) ?? null, runLanguage(a), runSavesAudio(a));
    case "engine_stop_mic":
      return stopMic();
    case "engine_start_system":
      return startSystem(a, runLanguage(a));
    case "engine_stop_system":
      return stopMic(true);
    case "list_system_audio_devices":
      return SYSTEM_DEVICES;
    case "engine_cancel":
      return cancel(Number(a.sessionId));
    case "transcribe_file":
      return transcribeFile(
        String(a.path),
        (a.itemType as "note" | "transcription") ?? "note",
        (a.title as string | null) ?? null,
        runLanguage(a),
        !!a.identifyVoices,
        runSavesAudio(a),
      );
    case "recipes_list":
      return allRecipes();
    case "recipe_documents":
      if (!find(String(a.id))) throw `no archive item '${a.id}'`;
      return (docs[String(a.id)] ?? []).map((d) => ({ ...d, meta: { ...d.meta } }));
    case "recipe_run":
      return runRecipe(String(a.id), String(a.recipeId), (a.profileId as string | null) ?? null, null, a.consent, !!a.includeEmails);
    case "recipe_ask":
      return runRecipe(String(a.id), "question", (a.profileId as string | null) ?? null, String(a.question ?? ""), a.consent, !!a.includeEmails);
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
      return `${pkgVersion}-dev`;
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

/* ---------- Saved audio for the Audio tab (#142) ---------- */

/** What the backend's sussurro-audio: scheme would serve: here a
 *  synthesized WAV (8 kHz, a hum per speaker pulsing like syllables on each
 *  of the channel's lines, silence between) as a blob: URL, cached. Only as
 *  long as the lines (max 5 min), whatever the frontmatter duration says. */
const audioUrls = new Map<string, string>();
function mockAudioUrl(path: string): string {
  const cached = audioUrls.get(path);
  if (cached) return cached;
  const cut = path.lastIndexOf("/");
  const s = find(path.slice(0, cut));
  const file = path.slice(cut + 1);
  const rate = 8000;
  const lines = (s?.segments ?? []).filter((x) => file === "audio.wav" || file === `audio-${x.channel ?? "mic"}.wav`);
  const endMs = Math.min(5 * 60_000, Math.max(2000, ...(s?.segments ?? []).map((x) => x.end_ms + 1000)));
  const n = Math.round((endMs / 1000) * rate);
  const buf = new DataView(new ArrayBuffer(44 + n * 2));
  const str = (o: number, t: string) => [...t].forEach((c, i) => buf.setUint8(o + i, c.charCodeAt(0)));
  str(0, "RIFF");
  buf.setUint32(4, 36 + n * 2, true);
  str(8, "WAVEfmt ");
  buf.setUint32(16, 16, true);
  buf.setUint16(20, 1, true);
  buf.setUint16(22, 1, true);
  buf.setUint32(24, rate, true);
  buf.setUint32(28, rate * 2, true);
  buf.setUint16(32, 2, true);
  buf.setUint16(34, 16, true);
  str(36, "data");
  buf.setUint32(40, n * 2, true);
  const pitch = (id?: string) => 140 + (([...(id ?? "")].reduce((a, c) => a + c.charCodeAt(0), 0) % 7) * 35);
  for (const l of lines) {
    const f = pitch(l.speaker_id);
    for (let i = Math.round((l.start_ms / 1000) * rate); i < Math.min(n, Math.round((l.end_ms / 1000) * rate)); i++) {
      const t = i / rate;
      const env = 0.5 + 0.5 * Math.sin(2 * Math.PI * 4 * t);
      const v = 0.25 * env * (Math.sin(2 * Math.PI * f * t) + 0.4 * Math.sin(4 * Math.PI * f * t));
      buf.setInt16(44 + i * 2, Math.max(-1, Math.min(1, v)) * 32767, true);
    }
  }
  const url = URL.createObjectURL(new Blob([buf.buffer], { type: "audio/wav" }));
  audioUrls.set(path, url);
  return url;
}

export function installMockTauri(): void {
  mockWindows("main");
  mockIPC((cmd, args) => handle(cmd, (args ?? {}) as Args), { shouldMockEvents: true });
  (window as unknown as { __TAURI_INTERNALS__: { convertFileSrc: (p: string, protocol?: string) => string } }).__TAURI_INTERNALS__.convertFileSrc = (
    p: string,
    protocol = "asset",
  ) => (protocol === "sussurro-audio" ? mockAudioUrl(p) : `${protocol}://localhost/${encodeURIComponent(p)}`);
  console.info("[dev] Tauri backend mocked — browser preview only");
}
