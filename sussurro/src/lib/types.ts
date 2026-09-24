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

/** A named LLM connection (llm/profile.rs). */
export interface LlmProfile {
  id: string;
  name: string;
  api: LlmApi;
  base_url: string;
  /** Bearer token, OpenAI-compatible only; "" = none. */
  api_key: string;
  model: string;
  /** Text sent to this profile leaves the machine (inferred from the URL,
   *  overridable). Drives the privacy warning. */
  external: boolean;
}

export interface Settings {
  hotkey: string;
  push_to_talk: boolean;
  whisper_model: string;
  engine: "whisper" | "parakeet";
  /** LLM profiles (#119): cleanup (and later recipes, Ask) pick one. */
  llm_profiles: LlmProfile[];
  /** Id of the profile cleanup runs on. */
  cleanup_profile: string;
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
  /** Dictate-to-file: append dictations to this file instead of pasting. */
  output_file: string;
  /** Archive folder; "" = the default `<Documents>/Sussurro`. */
  archive_dir: string;
  /** Workspace preview (#114): the new shell instead of the classic window. */
  ui_v2: boolean;
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

export interface SegmentsFile {
  version: number;
  speakers: { id: string; label: string; color: string }[];
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
}

export interface ItemSummary {
  id: string;
  meta: ItemMeta;
  edited_externally: boolean;
  /** Search excerpt with `**match**` highlights (search results only). */
  snippet?: string;
  recording?: boolean;
  interrupted?: boolean;
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

export interface EngineStatus {
  active: number;
  mic_session: number | null;
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
