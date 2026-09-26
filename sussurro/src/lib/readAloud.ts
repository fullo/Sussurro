/* Read aloud of an item (#256, P17/P21/P24): pure helpers for the Audio
   tab's "Generated speech" section. The backend owns the job
   (`read_aloud_start`, `read_aloud_cancel`, `read_aloud_job`), the files
   (`read_aloud_files`, `read_aloud_delete`) and the temporary Listen file
   (`read_aloud_discard`). Nothing here downloads: a missing model or voice
   sends the user to Models → Voices. */

import { formatBytes } from "./format";
import type { ReadAloudJob, SpeechStatus, TtsLanguage, TtsStatus } from "./types";
import { selectedVoice } from "./tts";

/** The transcript's document name (what `document` says for it). */
export const TRANSCRIPT_DOC = "transcript.md";

/** Said wherever generated speech is shown (P21): never a recording. */
export const SYNTHETIC_NOTE = "Synthetic voice made by Sussurro on this computer — not a recording of anyone.";

/** Said next to a temporary Listen player. */
export const LISTEN_NOTE = "Temporary: not saved, and deleted when you close this document.";

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

/** "Reading aloud… 12 of 40". */
export function jobLabel(job: ReadAloudJob): string {
  const what = job.save ? "Making the speech file" : "Preparing to listen";
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

/** One line under a speech file: voice, language, size, date. */
export function speechFacts(s: SpeechStatus, languages: TtsLanguage[] = []): string {
  const lang = languages.find((l) => l.code === s.language)?.label ?? s.language;
  const date = s.date ? s.date.slice(0, 10) : "";
  return [s.voice && `voice ${s.voice}`, lang, s.engine, formatBytes(s.bytes), date].filter(Boolean).join(" · ");
}

/** Why a speech file may no longer match its document, or null. */
export function staleNote(s: SpeechStatus): string | null {
  if (s.source_missing) return "The document it was read from has been deleted.";
  if (s.stale) return "Out of date: the text changed after this speech was made. Make it again to match.";
  if (!s.recorded) return "Sussurro has no record of what this file was read from.";
  return null;
}

/** The speech file already made from `document`, if any. */
export function speechFor(files: SpeechStatus[], document: string): SpeechStatus | undefined {
  return files.find((f) => f.document === document);
}

/** A rough "about N min" for a document of `chars` characters on this
 *  engine (Italian ~1.3× real time, English ~0.4× on an M1, #255), so a
 *  long transcript doesn't surprise anyone. */
export function estimateMinutes(chars: number, lang: string): number {
  const speechSeconds = chars / 15;
  const rtf = baseCode(lang) === "en" ? 0.5 : 1.4;
  return Math.max(1, Math.round((speechSeconds * rtf) / 60));
}
