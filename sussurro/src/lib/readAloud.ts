/* Read aloud of an item (#256, #327, P17/P21/P24): pure helpers for the
   Audio tab's "Generated speech" section — one button per document: Create
   when there is no speech yet, Listen (playing the saved file, no
   generation) once there is. The backend owns the job (`read_aloud_start`,
   `read_aloud_cancel`, `read_aloud_job`) and the files (`read_aloud_files`,
   `read_aloud_delete`). #327 removed the temporary Listen path of #256
   (`save: false`, `listen-N.opus`, `read_aloud_discard`) — every job now
   saves. Nothing here downloads: a missing model or voice sends the user
   to Models → Voices. */

import { formatBytes } from "./format";
import type { ReadAloudJob, SpeechStatus, TtsLanguage, TtsStatus } from "./types";
import { selectedVoice } from "./tts";

/** The transcript's document name (what `document` says for it). */
export const TRANSCRIPT_DOC = "transcript.md";

/** The *Podcast script* recipe's companion document (0.13 stretch goal,
 *  #259, P17) — the one document read aloud speaks with two distinct
 *  built-in voices instead of one narrator. Pinned to match the backend's
 *  `tts::podcast::SCRIPT_FILE` (a Rust test keeps the two in step). */
export const PODCAST_SCRIPT_DOC = "podcast-script.md";

/** Said wherever generated speech is shown (P21): never a recording. */
export const SYNTHETIC_NOTE = "Synthetic voice made by Sussurro on this computer — not a recording of anyone.";

/** A document the user can pick: the transcript or a companion. */
export interface ReadableDoc {
  file: string;
  label: string;
}

/** The transcript first, then the companion documents. */
export function readableDocs(companions: { file: string; meta?: { title?: string } }[]): ReadableDoc[] {
  return [
    { file: TRANSCRIPT_DOC, label: "Transcript" },
    ...companions.map((d) => ({ file: d.file, label: d.meta?.title?.trim() || d.file })),
  ];
}

/** "Transcript", a companion's title, or the file name. */
export function documentLabel(document: string, docs: ReadableDoc[]): string {
  if (!document) return "Unknown document";
  return docs.find((d) => d.file === document)?.label ?? document;
}

/** Where read aloud stands for one language. */
export type Readiness =
  | { state: "off" }
  | { state: "no-language" }
  | { state: "missing"; lang: TtsLanguage; what: string }
  | { state: "ready"; lang: TtsLanguage; voice: string };

/** Whether `code` can be read now: the module on, a model for it, its
 *  model and selected voice downloaded. */
export function readiness(status: TtsStatus | null, code: string): Readiness {
  if (!status || !status.enabled) return { state: "off" };
  const lang = status.languages.find((l) => l.code === baseCode(code));
  if (!lang) return { state: "no-language" };
  const voice = selectedVoice(lang);
  if (!lang.model_downloaded) return { state: "missing", lang, what: `the ${lang.label} model` };
  if (!voice?.downloaded) return { state: "missing", lang, what: `the voice ${voice?.label ?? ""}`.trim() };
  return { state: "ready", lang, voice: voice.label };
}

/** `it-IT` → `it`. */
export function baseCode(code: string): string {
  return (code || "").trim().toLowerCase().split(/[-_]/)[0] ?? "";
}

/** The language to offer first: the item's when read aloud has it, else the
 *  first language whose model is downloaded, else the first. */
export function defaultLanguage(status: TtsStatus | null, itemLanguage: string): string {
  const langs = status?.languages ?? [];
  const own = langs.find((l) => l.code === baseCode(itemLanguage));
  if (own) return own.code;
  return (langs.find((l) => l.model_downloaded) ?? langs[0])?.code ?? "";
}

/** "Making the speech file… 12 of 40 passages". */
export function jobLabel(job: ReadAloudJob): string {
  const what = "Making the speech file";
  return job.total > 0 ? `${what}… ${Math.min(job.done, job.total)} of ${job.total} passages` : `${what}…`;
}

export function jobFraction(job: ReadAloudJob | null): number {
  if (!job || job.total <= 0) return 0;
  return Math.min(1, Math.max(0, job.done / job.total));
}

/** The job belongs to this item. */
export function jobIsFor(job: ReadAloudJob | null, itemId: string): boolean {
  return !!job && job.item_id === itemId;
}

/** One line under a speech file: voice(s), language, size, date. A
 *  two-voice podcast script (#259) names both hosts' voices. */
export function speechFacts(s: SpeechStatus, languages: TtsLanguage[] = []): string {
  const lang = languages.find((l) => l.code === s.language)?.label ?? s.language;
  const date = s.date ? s.date.slice(0, 10) : "";
  const voice = s.voice_b ? `voices ${s.voice} (Host A), ${s.voice_b} (Host B)` : s.voice && `voice ${s.voice}`;
  return [voice, lang, s.engine, formatBytes(s.bytes), date].filter(Boolean).join(" · ");
}

/** Why a speech file may no longer match its document, or null. */
export function staleNote(s: SpeechStatus): string | null {
  if (s.source_missing) return "The document it was read from has been deleted.";
  if (s.stale) return "Out of date: the text changed after this speech was made. Create it again to match.";
  if (!s.recorded) return "Sussurro has no record of what this file was read from.";
  return null;
}

/** Why a speech file carries no signature (#257 part 2), when the
 *  frontmatter recorded one; null when it is signed or predates signing. */
export function speechSignatureNote(s: SpeechStatus): string | null {
  if (s.signed || !s.unsigned) return null;
  return `Not signed: ${s.unsigned}. The watermark and the tags still mark it as synthetic.`;
}

/** The speech file already made from `document`, if any. */
export function speechFor(files: SpeechStatus[], document: string): SpeechStatus | undefined {
  return files.find((f) => f.document === document);
}

/** The Audio tab's single button per document (#327): "Create" when there
 *  is no speech yet, "Create again" once there is but it's stale (a
 *  secondary action next to Listen), or `null` when the existing file is
 *  current — Listen alone is enough then. A missing source document
 *  doesn't change this (Listen still plays what was made; the note about
 *  it is separate, see `staleNote`). */
export function createLabel(existing: SpeechStatus | undefined): "Create" | "Create again" | null {
  if (!existing) return "Create";
  return existing.stale ? "Create again" : null;
}

/** Listen (playing the file already made for `document`) is disabled only
 *  while a job is about to replace that exact file — it may be gone by the
 *  time playback would start. */
export function listenDisabled(job: ReadAloudJob | null, itemId: string, document: string): boolean {
  return jobIsFor(job, itemId) && job?.document === document;
}

/** A rough "about N min" for a document of `chars` characters on this
 *  engine (Italian ~1.3× real time, English ~0.4× on an M1, #255), so a
 *  long transcript doesn't surprise anyone. */
export function estimateMinutes(chars: number, lang: string): number {
  const speechSeconds = chars / 15;
  const rtf = baseCode(lang) === "en" ? 0.5 : 1.4;
  return Math.max(1, Math.round((speechSeconds * rtf) / 60));
}
