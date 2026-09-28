/* Read aloud — Models → Voices and Settings → Experimental (#255, P18,
   P24): pure helpers. The backend owns the models (`tts_status`,
   `tts_download`, `tts_delete*`, `tts_preview`); downloads start only from
   a click here while the module is on, after the size and licence were
   shown. */

import { formatBytes } from "./format";
import type { TtsDownloadProgress, TtsLanguage, TtsStatus, TtsVoice, TtsWatermark, WatermarkCheck } from "./types";

/** The label every read-aloud surface carries (P24). */
export const EXPERIMENTAL = "Experimental";

/** What the module is, for Settings → Experimental. */
export const TTS_ABOUT =
  "Reads documents aloud with Pocket TTS by Kyutai, on this computer. Nothing is downloaded until you ask: turn it on here, then pick the languages and voices in Models → Voices, where each download shows its size and licence first.";

/** The voice selected for a language, if any. */
export function selectedVoice(lang: TtsLanguage): TtsVoice | undefined {
  return lang.voices.find((v) => v.selected) ?? lang.voices[0];
}

/** Bytes a "Download" of the language fetches: the model if missing, the
 *  selected voice if missing, and the watermark models if missing (#257:
 *  they come with the first download — nothing is spoken without them). */
export function downloadBytes(lang: TtsLanguage, voice?: TtsVoice, watermark?: TtsWatermark): number {
  const v = voice ?? selectedVoice(lang);
  const wm = watermark && !watermark.downloaded ? watermark.bytes : 0;
  return (lang.model_downloaded ? 0 : lang.model_bytes) + (v && !v.downloaded ? v.bytes : 0) + wm;
}

/** One line under the language's name. */
export function languageStatus(lang: TtsLanguage): string {
  if (!lang.model_downloaded) return `Not downloaded — ${formatBytes(lang.model_bytes)} for the model`;
  const ready = lang.voices.filter((v) => v.downloaded).length;
  if (ready === 0) return "Model downloaded — download a voice to use it";
  return `Downloaded, ${ready} voice${ready === 1 ? "" : "s"}`;
}

/** The watermark models as the confirmation names them. */
function watermarkPart(w: TtsWatermark): string {
  return `the watermark models that mark every file Sussurro speaks (${formatBytes(w.bytes)}, ${w.licence}, ${w.attribution})`;
}

/** The confirmation shown before a download (P24: size + licence first). */
export function downloadPrompt(status: TtsStatus, lang: TtsLanguage, voice?: TtsVoice): string {
  const v = voice ?? selectedVoice(lang);
  const parts: string[] = [];
  if (!lang.model_downloaded) parts.push(`the ${lang.label} model (${lang.variant}, ${formatBytes(lang.model_bytes)})`);
  if (v && !v.downloaded) parts.push(`the voice ${v.label} (${formatBytes(v.bytes)})`);
  const wm = status.watermark && !status.watermark.downloaded && parts.length ? status.watermark : undefined;
  if (wm) parts.push(watermarkPart(wm));
  const what = parts.length > 1 ? `${parts.slice(0, -1).join(", ")} and ${parts[parts.length - 1]}` : (parts[0] ?? "nothing new");
  return `Download ${what} from huggingface.co? Total ${formatBytes(downloadBytes(lang, v, wm))}. Licence: ${status.licence} (${status.attribution}).`;
}

/** The confirmation of a watermark-only download (TTS models from before
 *  #257 can't speak without it). */
export function watermarkPrompt(w: TtsWatermark): string {
  return `Download ${watermarkPart(w)} from huggingface.co? Total ${formatBytes(w.bytes)}.`;
}

/** Why generated speech can't be made while the watermark is missing, when
 *  some model is already there (null otherwise). */
export function watermarkMissing(status: TtsStatus): string | null {
  const w = status.watermark;
  if (!w || w.downloaded) return null;
  if (!status.languages.some((l) => l.model_downloaded)) return null;
  return "Every file Sussurro speaks carries an inaudible watermark, and its model is not downloaded yet: reading aloud and previews wait for it.";
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

/** Whether Preview can run for this voice (and, when the backend reports
 *  it, with the watermark model there, #257). */
export function canPreview(lang: TtsLanguage, voice: TtsVoice, watermark?: TtsWatermark): boolean {
  return lang.model_downloaded && voice.downloaded && (!watermark || watermark.downloaded);
}

/** The `tts_voices` setting with `code` → `voice`. */
export function withVoice(voices: Record<string, string> | undefined, code: string, voice: string): Record<string, string> {
  return { ...(voices ?? {}), [code]: voice };
}

// ---- Check a file (#257, E17) ------------------------------------------------
// Wording rules (E17 / plan 4.7): "found" only with Sussurro's code; a mark
// with another code is "inconclusive", never another tool's mark; nothing
// found is "no Sussurro mark found", never "made by a person".

/** The one-line answer. */
export function checkSummary(r: WatermarkCheck): string {
  switch (r.summary) {
    case "made_by_sussurro":
      return "Made by Sussurro: the audio carries Sussurro's watermark.";
    case "inconclusive":
      return "Inconclusive: something like a watermark is there, but not with Sussurro's code. Music and steady tones can look like this.";
    case "tags_only":
      return "No Sussurro watermark found. The file's tags say Sussurro made it, but tags can be copied or edited by anyone.";
    default:
      return "No Sussurro mark found. That doesn't say who or what made the audio.";
  }
}

const pct = (x: number) => `${Math.round(x * 100)}%`;

/** The watermark layer's line. */
export function watermarkLine(r: WatermarkCheck): string {
  const w = r.watermark;
  switch (w.verdict) {
    case "found":
      return `Watermark: found — ${pct(w.frames_marked)} of the audio marked, with Sussurro's code (${w.bit_errors} of 16 bits off).`;
    case "inconclusive":
      return `Watermark: inconclusive — ${pct(w.frames_marked)} of the audio looks marked, but ${w.bit_errors} of the 16 code bits differ from Sussurro's.`;
    default:
      return `Watermark: not found (${pct(w.frames_marked)} of the audio looked marked; at least half is needed).`;
  }
}

/** The metadata layer's line. */
export function metadataLine(r: WatermarkCheck): string {
  switch (r.metadata.status) {
    case "sussurro":
      return "Tags: synthetic speech generated by Sussurro. Tags are not signed: anyone can write or remove them.";
    case "synthetic":
      return "Tags: marked as AI-generated by other software.";
    case "none":
      return "Tags: no synthetic-speech tags.";
    default:
      return `Tags: not read for ${r.format} files.`;
  }
}

/** The signed-metadata layer's line (C2PA comes with #257's second part). */
export function signatureLine(r: WatermarkCheck): string {
  return r.signature.status === "not_checked" ? "Signed metadata (C2PA): not checked yet." : `Signed metadata: ${r.signature.status}.`;
}

/** Caveats worth a line. */
export function checkNotes(r: WatermarkCheck): string[] {
  const notes: string[] = [];
  if (r.short) notes.push("The audio is under 3 seconds: too short for the watermark to say much.");
  if (r.truncated) notes.push("Only the first hour was checked.");
  notes.push("Checked on this computer: nothing was uploaded.");
  return notes;
}
