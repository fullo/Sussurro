/* Shapes shared with the Rust backend (commands.rs, settings.rs, archive/,
   engine/). Keep in step with the serde definitions there. */

export type CleanupLevel = "none" | "light" | "medium" | "high";

export interface Snippet {
  cue: string;
  text: string;
}

export interface AppStyle {
  app_match: string;
  style: string;
  /** Per-app output language (ISO code); "" = follow the global setting. */
  language: string;
}

/** Chat API an LLM profile speaks (P6): Ollama native or OpenAI-compatible. */
export type LlmApi = "ollama" | "openai";

/** Where a profile's API key lives (llm::KeyStorage). */
export type KeyStorage = "none" | "keychain" | "file" | "unreadable";

/** The OS credential store as the profile editor sees it (secrets::StoreStatus). */
export interface CredentialStoreStatus {
  available: boolean;
  /** "the macOS Keychain", "Windows Credential Manager", … */
  name: string;
  error: string;
}

/** A named LLM connection (llm/profile.rs). */
export interface LlmProfile {
  id: string;
  name: string;
  api: LlmApi;
  base_url: string;
  /** Bearer token, OpenAI-compatible only; "" = none. */
  api_key: string;
  /** Where the key is kept (#159), set by the backend and ignored when
   *  sent back: the OS credential store, settings.json as a fallback when no
   *  store works, or a store entry that couldn't be read this session.
   *  Absent = no key (or not placed yet). */
  api_key_storage?: KeyStorage;
  model: string;
  /** Text sent to this profile leaves the machine (inferred from the URL,
   *  overridable). Drives the privacy warning. */
  external: boolean;
  /** Model context window in tokens; 0 / absent = unknown (recipes then
   *  plan for 4096). */
  context_tokens?: number;
  /** Cleanup opt-in on an external profile (#122): the host the user agreed
   *  to send dictations and transcriptions to. Absent/"" = not agreed, and
   *  cleanup keeps the raw text. Bound to the host. */
  cleanup_opt_in?: string;
  /** The built-in "Local (bundled)" profile (#118): Sussurro's own
   *  llama-server with a small model. Its fields are fixed by the backend
   *  (never external); absent/false on every other profile. */
  bundled?: boolean;
}

/** The bundled LLM (`bundled_llm_status`, #118). */
export interface BundledLlmStatus {
  /** This build ships the llama-server sidecar. */
  available: boolean;
  /** The model file is in the models folder. */
  downloaded: boolean;
  /** Its server runs right now. */
  running: boolean;
  /** "Qwen3 1.7B" */
  model: string;
  download_bytes: number;
}

/** What a run on an external profile would send, and where
 *  (`external_run_preview`, #122). */
export interface ExternalRunPreview {
  item_id: string;
  item_title: string;
  recipe_id: string;
  recipe_name: string;
  question: string | null;
  profile_id: string;
  profile_name: string;
  host: string;
  base_url: string;
  model: string;
  chars: number;
  approx_tokens: number;
  external: boolean;
  /** Speaker names sent with the transcript (#143). */
  speakers?: string[];
  /** Participants whose names are sent. */
  participants?: number;
  /** Participant emails the item has, and how many this run sends (0
   *  unless the user ticked them for this run). */
  emails_available?: number;
  emails_sent?: number;
}

/** `prepare_external_run`: the one-time token a confirmed run needs. */
export interface ExternalRunConsent {
  token: string;
  expires_in_secs: number;
}

/** Where a recipe's output goes (recipes/mod.rs). */
export type RecipeTarget = "companion_document" | "answer";

/** A named prompt with a target (#120). Built-ins come from the backend
 *  (`recipes_list`); only user recipes live in `Settings.recipes`. */
export interface Recipe {
  id: string;
  name: string;
  prompt: string;
  target: RecipeTarget;
  builtin: boolean;
  /** Only for meetings and transcriptions whose transcript names its
   *  speakers (#143: Meeting minutes, Who said what). */
  speakers_only?: boolean;
}

export interface Settings {
  hotkey: string;
  push_to_talk: boolean;
  whisper_model: string;
  /** `qwen3_asr` (#117) needs the bundled llama-server sidecar. */
  engine: "whisper" | "parakeet" | "qwen3_asr";
  /** LLM profiles (#119): cleanup (and later recipes, Ask) pick one. */
  llm_profiles: LlmProfile[];
  /** Id of the profile cleanup runs on. */
  cleanup_profile: string;
  /** The user's own recipes (#120); built-ins are not stored. */
  recipes: Recipe[];
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
  whisper_mode: boolean;
  stream_injection: boolean;
  voice_commands: boolean;
  prompt_overrides: { light: string; medium: string; high: string };
  history_retention_days: number;
  api_enabled: boolean;
  api_port: number;
  /** The token-less scripting routes (`/clean`, `/transcribe`, `/history`)
   *  answer (#215). Off on a new install; applies at once. */
  api_scripting: boolean;
  /** The archive API (`/archive/…`, #249) answers — each request also needs
   *  an archive token. Off by default; applies at once. Absent before 0.11.
   *  The tokens themselves are not part of the settings the UI edits: they
   *  come from `archive_tokens_list` and change only through their own
   *  commands. */
  api_archive?: boolean;
  /** Dictate-to-file: append dictations to this file instead of pasting. */
  output_file: string;
  /** Archive folder; "" = the default `<Documents>/Sussurro`. */
  archive_dir: string;
  /** First-run onboarding (#115): the full setup on a fresh install, a
   *  "What's new" screen on an upgrade from 0.6.x, nothing once done. */
  onboarding: Onboarding;
  /** Subtitles setting (P7, #133): `transcript.srt` only when asked, or on
   *  every save. Meetings and transcriptions only (P10). */
  subtitles: SubtitlesMode;
  /** Browser-extension pairing token (#126); set only by the backend
   *  (`extension_token_get` / `extension_token_regenerate`). */
  extension_token: string;
  /** "Save audio" preselected in New (P9, #141). Off by default: WAV is
   *  saved only on request. Absent in settings from before 0.10. */
  save_audio?: boolean;
  /** "Saved audio format" (#247, P16): the format of the audio runs save
   *  from now on; items saved earlier keep theirs. Absent in settings from
   *  before 0.11 (= the backend default, WAV until #248). */
  saved_audio_format?: SavedAudioFormat;
  /** The notice before the first recording of other people (#136) was
   *  acknowledged with "Don't show this again". Absent or false (a fresh or
   *  cleared settings file) = show it. */
  meeting_notice_seen?: boolean;
}

/** Saved audio file format (#247): 16-bit WAV or Ogg Opus at 24 kb/s. */
export type SavedAudioFormat = "wav" | "opus";

export type SubtitlesMode = "on_request" | "always";

/** `Settings.onboarding` (settings.rs `Onboarding`). */
export type Onboarding = "welcome" | "whats_new" | "done";

/** `local_api_status` (#127): whether this run serves the local API. Its
 *  settings apply at startup, so this can differ from them until a restart. */
export type ListenState =
  | { state: "off" }
  | { state: "listening"; port: number }
  | { state: "failed"; port: number };

/** What an archive item can be exported as (#133). */
export type ExportFormat = "md" | "txt" | "srt" | "vtt";

/** Where an item's `transcript.srt` stands (`archive_subtitles_status`). */
export interface SubtitlesStatus {
  /** `transcript.srt`. */
  file: string;
  /** A meeting or a transcription: notes have no subtitles (P10). */
  applicable: boolean;
  exists: boolean;
  /** Changed since the app wrote it (or not the app's): never overwritten. */
  edited_externally: boolean;
}

export interface OllamaStatus {
  installed: boolean;
  running: boolean;
  has_model: boolean;
}

export type PermState = "granted" | "denied" | "unknown" | "not_applicable";
export interface Permissions {
  microphone: PermState;
  accessibility: PermState;
}

export interface HistoryEntry {
  timestamp: string;
  raw: string;
  cleaned: string;
}

export interface UsageStats {
  total_dictations: number;
  total_words: number;
  today_dictations: number;
  today_words: number;
  week_dictations: number;
  week_words: number;
}

/* ---------- Archive (archive/types.rs, archive/store.rs) ---------- */

export type ItemType = "note" | "meeting" | "transcription";

export interface Participant {
  name: string;
  email?: string;
}

/** People registry entry (archive/people.rs, #132), stored in
 *  `<archive>/.sussurro/people.json`. */
export interface Person {
  /** "" for a draft not saved yet. */
  id: string;
  name: string;
  email?: string;
  aliases: string[];
}

/** A person's voice profile as the UI sees it (#241, `voice_status`,
 *  `voices_status`): never the voiceprint itself, which stays in the app
 *  data dir. `enabled` = *Recognise this voice* is on; `ready` = enough
 *  confirmed speech (`min_speech_ms`) from enough documents
 *  (`min_documents`) to make suggestions. */
export interface VoiceStatus {
  person_id: string;
  enabled: boolean;
  speech_ms: number;
  documents: number;
  ready: boolean;
  min_speech_ms: number;
  min_documents: number;
  /** RFC 3339 time of the last build; "" when off. */
  updated: string;
}

/** Frontmatter of transcript.md. Unknown keys (Obsidian aliases…) ride along
 *  flattened and must be sent back untouched on update. */
export interface ItemMeta {
  type: ItemType;
  title: string;
  /** RFC 3339. */
  date: string;
  /** HH:MM:SS */
  duration?: string;
  source: string;
  language: string;
  engine: string;
  tags: string[];
  categories: string[];
  participants: Participant[];
  [extra: string]: unknown;
}

export interface Word {
  w: string;
  start_ms: number;
  end_ms: number;
}

export interface Segment {
  id: number;
  channel?: "mic" | "remote" | "system" | "file";
  start_ms: number;
  end_ms: number;
  speaker_id?: string;
  raw: string;
  text: string;
  edited?: boolean;
  words?: Word[];
  /** STT failed on this stretch (#153): empty text, shown as "[not transcribed]". */
  stt_error?: string;
}

/** A speaker as known to one document (#130): `you`, `meet:<name>` or
 *  `voice:<n>`, with this document's label and colour. */
export interface DocSpeaker {
  id: string;
  label: string;
  color: string;
  /** Person of the People registry this speaker is (#132). */
  person_id?: string;
  /** Label before a link replaced it; unlinking gives it back. */
  label_before_link?: string;
}

export interface SegmentsFile {
  version: number;
  speakers: DocSpeaker[];
  segments: Segment[];
}

export interface Item {
  id: string;
  meta: ItemMeta;
  segments: SegmentsFile;
  body: string;
  edited_externally: boolean;
  /** A capture session is still writing this item (#153). */
  recording?: boolean;
  /** The app stopped before the session was finalized (#153). */
  interrupted?: boolean;
  /** Hosts its text was sent to by an external LLM profile (#122); empty =
   *  it never left the machine. */
  external_hosts?: string[];
  /** Lines with voice data (#130): "Re-detect speakers" needs some. The
   *  embeddings themselves stay in the backend. */
  embedded_segments?: number;
  /** Saved audio in the item folder (#141): `audio.wav`, or one file per
   *  channel (`audio-mic.wav`, `audio-remote.wav`…). Empty = none. */
  audio?: AudioFile[];
  /** Size of the item folder on disk, audio included (#141). */
  folder_bytes?: number;
}

/** One saved audio file of an item (#141). */
export interface AudioFile {
  name: string;
  bytes: number;
}

/** Whether "Identify voices" can run on a transcription (#134,
 *  `archive_voice_source`). */
export interface VoiceSource {
  available: boolean;
  /** Why not, for the speaker panel (empty when available). */
  reason: string;
  /** The original file's name (empty when unknown), or the saved audio
   *  file's when `saved_audio`. */
  file_name: string;
  /** The voices come from the audio saved with the item (#248): the
   *  original file isn't available. Absent from older backends. */
  saved_audio?: boolean;
}

/** One line on the Voice map (#144, `archive_voice_map`). */
export interface VoiceMapPoint {
  segment_id: number;
  /** The line's speaker when the map was computed. */
  speaker_id?: string | null;
  x: number;
  y: number;
}

/** The Voice map of an item (#144): its stored speaker embeddings projected
 *  to 2-D (PCA) by the backend. */
export interface VoiceMap {
  /** At most 5,000, in line order. */
  points: VoiceMapPoint[];
  /** Lines with voice data (more than `points` = downsampled for drawing). */
  total: number;
  /** Share of the voice prints' differences the two axes keep, 0–1. */
  explained: number;
}

export interface ItemSummary {
  id: string;
  meta: ItemMeta;
  edited_externally: boolean;
  /** Search excerpt with `**match**` highlights (search results only). */
  snippet?: string;
  recording?: boolean;
  interrupted?: boolean;
  /** See Item.external_hosts (the Library's "sent externally" marker). */
  external_hosts?: string[];
  /** Bytes of saved audio in the item folder (#141); 0 = none. */
  audio_bytes?: number;
}

/* ---------- Long-form engine (engine/mod.rs, commands.rs) ---------- */

/** `transcribe_file` result: the archive item written by the engine. */
export interface EngineResult {
  /** Present since #153. */
  session_id?: number;
  item_id: string;
  item_type: ItemType;
  title: string;
  text: string;
  segments: number;
  duration_s: number;
}

/** `engine-progress` payload. */
export interface EngineProgress {
  session_id: number;
  processed_s: number;
  ingested_s: number;
  total_s: number;
  backlog_s: number;
  queue_len: number;
  segments_done: number;
}

/** `engine-segment` payload. */
export interface EngineSegmentEvent {
  session_id: number;
  segment: Segment;
}

/** `engine-started` payload (#153): the run's item exists, marked
 *  recording. `item_id` is provisional for an untitled session: the folder
 *  is renamed after the final title and `engine-done` carries the final id. */
export interface EngineStarted {
  session_id: number;
  item_id: string;
  item_type: ItemType;
  title: string;
  source: string;
}

/** `engine-done` payload. */
export interface EngineDone extends EngineResult {
  session_id: number;
}

/** `engine-error` payload. `item_id` names an item kept as interrupted when
 *  some segments were transcribed before the failure (#153). */
export interface EngineError {
  session_id: number;
  error: string;
  item_id?: string;
}

/** `engine-warning` payload (#139): the run goes on, but the user should
 *  know — a device of a System audio + mic session was lost, or the two
 *  device clocks were realigned. */
export interface EngineWarning {
  session_id: number;
  message: string;
}

/** An input device in New → System audio + mic (#139). */
export interface InputDeviceInfo {
  name: string;
  /** The name looks like a loopback/virtual device — a hint only. */
  loopback: boolean;
}

/** "This computer's sound (built-in)" (#140): the OS's own capture of the
 *  output — WASAPI loopback, a Core Audio process tap (macOS 14.2+), the
 *  default sink's monitor (PulseAudio/PipeWire). */
export interface NativeLoopback {
  available: boolean;
  backend: "wasapi" | "coreaudio-tap" | "pulse-monitor" | "none";
  /** What it records, when known: the output device, or the monitor. */
  detail: string | null;
  /** Why it is unavailable (the device picker is the fallback). */
  reason: string | null;
  /** The OS asks for a permission on first use (macOS). */
  needs_permission: boolean;
}

/** `list_system_audio_devices` (#139). */
export interface SystemAudioDevices {
  /** What an empty microphone choice records. */
  default_input: string | null;
  /** Every input device, loopback-looking ones first. */
  devices: InputDeviceInfo[];
  /** The native choice (#140); absent = not offered. */
  native?: NativeLoopback;
}

export interface EngineStatus {
  active: number;
  mic_session: number | null;
  /** The browser meeting being recorded (#126). */
  meeting_session?: number | null;
  /** The System audio + mic session (#139). */
  system_session?: number | null;
  /** Running file transcriptions, oldest first (#158). */
  file_sessions: { session_id: number; label: string }[];
  /** Running link transcriptions, oldest first (#123). */
  link_sessions?: { session_id: number; label: string }[];
}

/* ---------- Link source (#123, sources/url) ---------- */

export type LinkKind = "direct" | "platform";
export type LinkVia = "direct" | "yt-dlp";

/** `engine-download` payload: a link run fetching its audio, before
 *  `engine-started`. */
export interface EngineDownload {
  session_id: number;
  via: LinkVia;
  downloaded_bytes: number;
  /** Absent when the server does not say. */
  total_bytes: number | null;
  /** The platform's title (yt-dlp), once known. */
  title: string | null;
}

/** `link_inspect`: what the Link tab shows while the user types. */
export interface LinkInfo {
  kind: LinkKind | null;
  error: string | null;
  /** Visibly this computer or the local network: needs the opt-in. */
  local: boolean;
  label: string;
}

/** `yt_dlp_status`. */
export interface YtDlpStatus {
  found: boolean;
  path: string | null;
  version: string | null;
  install_help: string;
}

/* ---------- Recipes (recipes/, archive/companion.rs, #120) ---------- */

/** Frontmatter of a companion document: its provenance. */
export interface CompanionMeta {
  title: string;
  /** "<recipe> / <profile> / <model>" */
  generated_by: string;
  recipe: string;
  profile: string;
  model: string;
  external: boolean;
  /** Server the transcript went to, on an external profile (#122). */
  host?: string;
  date: string;
  transcript: string;
  [extra: string]: unknown;
}

/** A markdown file a recipe wrote next to the transcript. */
export interface CompanionDoc {
  /** File name in the item folder ("document.md"). */
  file: string;
  meta: CompanionMeta;
  body: string;
  /** Changed since the app wrote it: a regeneration writes a new copy. */
  edited_externally: boolean;
}

export type RecipePhase = "single" | "map" | "merge" | "reduce";

export interface RecipeStep {
  phase: RecipePhase;
  done: number;
  total: number;
}

/** `recipe-progress` payload. */
export interface RecipeProgress extends RecipeStep {
  item_id: string;
  recipe_id: string;
  recipe_name: string;
  /** The question, for a free question from the Ask panel (#121). */
  question?: string | null;
}

/** `recipe_status` entry: a run in flight. */
export interface RecipeRunStatus {
  item_id: string;
  recipe_id: string;
  recipe_name: string;
  question?: string | null;
  progress: RecipeStep | null;
}

/** `recipe-finished` payload and `recipe_run` / `recipe_ask` result. */
export interface RecipeFinished {
  item_id: string;
  recipe_id: string;
  recipe_name: string;
  file: string | null;
  answer: string | null;
  /** Handle for `recipe_save_answer` (the answer is kept in memory only). */
  answer_id?: number | null;
  question?: string | null;
  /** Profile the run used (its name), model and external flag. */
  profile?: string;
  model?: string;
  external?: boolean;
  /** Server the transcript went to, on an external profile (#122). */
  host?: string;
  error: string | null;
  cancelled: boolean;
}
