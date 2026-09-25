/* Read aloud — Models → Voices and Settings → Experimental (#255, P18,
   P24): pure helpers. The backend owns the models (`tts_status`,
   `tts_download`, `tts_delete*`, `tts_preview`); downloads start only from
   a click here while the module is on, after the size and licence were
   shown. */

import { formatBytes } from "./format";
import type { TtsDownloadProgress, TtsLanguage, TtsStatus, TtsVoice } from "./types";

/** The label every read-aloud surface carries (P24). */
export const EXPERIMENTAL = "Experimental";

/** What the module is, for Settings → Experimental. */
export const TTS_ABOUT =
  "Reads documents aloud with Pocket TTS by Kyutai, on this computer. Nothing is downloaded until you ask: turn it on here, then pick the languages and voices in Models → Voices, where each download shows its size and licence first.";

/** The voice selected for a language, if any. */
export function selectedVoice(lang: TtsLanguage): TtsVoice | undefined {
  return lang.voices.find((v) => v.selected) ?? lang.voices[0];
}

/** Bytes a "Download" of the language fetches: the model if missing plus
 *  the selected voice if missing. */
export function downloadBytes(lang: TtsLanguage, voice?: TtsVoice): number {
  const v = voice ?? selectedVoice(lang);
  return (lang.model_downloaded ? 0 : lang.model_bytes) + (v && !v.downloaded ? v.bytes : 0);
}

/** One line under the language's name. */
export function languageStatus(lang: TtsLanguage): string {
  if (!lang.model_downloaded) return `Not downloaded — ${formatBytes(lang.model_bytes)} for the model`;
  const ready = lang.voices.filter((v) => v.downloaded).length;
  if (ready === 0) return "Model downloaded — download a voice to use it";
  return `Downloaded, ${ready} voice${ready === 1 ? "" : "s"}`;
}

/** The confirmation shown before a download (P24: size + licence first). */
export function downloadPrompt(status: TtsStatus, lang: TtsLanguage, voice?: TtsVoice): string {
  const v = voice ?? selectedVoice(lang);
  const parts: string[] = [];
  if (!lang.model_downloaded) parts.push(`the ${lang.label} model (${lang.variant}, ${formatBytes(lang.model_bytes)})`);
  if (v && !v.downloaded) parts.push(`the voice ${v.label} (${formatBytes(v.bytes)})`);
  const what = parts.length ? parts.join(" and ") : "nothing new";
  return `Download ${what} from huggingface.co? Total ${formatBytes(downloadBytes(lang, v))}. Licence: ${status.licence} (${status.attribution}).`;
}

/** "42%" of a running download, with the file. */
export function progressLabel(p: TtsDownloadProgress): string {
  const pct = p.total_bytes > 0 ? Math.floor((p.done_bytes / p.total_bytes) * 100) : 0;
  return `${pct}% · ${formatBytes(p.done_bytes)} of ${formatBytes(p.total_bytes)}${p.file ? ` · ${p.file}` : ""}`;
}

export function progressFraction(p: TtsDownloadProgress | null | undefined): number {
  if (!p || p.total_bytes <= 0) return 0;
  return Math.min(1, Math.max(0, p.done_bytes / p.total_bytes));
}

/** Turning the module off with models on disk offers to delete them. */
export function offPrompt(bytesOnDisk: number): string | null {
  if (bytesOnDisk <= 0) return null;
  return `Read aloud is off. Delete its downloaded models and voices too (${formatBytes(bytesOnDisk)})? You can download them again later.`;
}

/** Whether Preview can run for this voice. */
export function canPreview(lang: TtsLanguage, voice: TtsVoice): boolean {
  return lang.model_downloaded && voice.downloaded;
}

/** The `tts_voices` setting with `code` → `voice`. */
export function withVoice(voices: Record<string, string> | undefined, code: string, voice: string): Record<string, string> {
  return { ...(voices ?? {}), [code]: voice };
}
