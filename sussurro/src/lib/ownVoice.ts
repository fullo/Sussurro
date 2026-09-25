/* The user's own voice, "You" (#243, plan P14): the pure logic behind the
   enrolment dialog, Settings → Voices and the speaker panel's offer. The
   backend (speakers/own_voice.rs) builds and stores the profile and does
   the labelling; nothing here sees a vector. */

import type { Item, OwnVoiceStatus } from "./types";

export type EnrolLanguage = "it" | "en";

/** The paragraph read aloud for the enrolment: about 30 s at a meeting's
 *  pace, varied sounds, nothing personal. */
export const ENROL_PARAGRAPHS: Record<EnrolLanguage, string> = {
  en:
    "This is my voice, recorded on this computer so that Sussurro can recognise me in my own recordings. " +
    "I am reading slowly and clearly, at the pace I use in a meeting. On Monday morning we planned the " +
    "budget, checked the numbers twice and agreed on three questions for the next call. Afterwards, somebody " +
    "suggested a quick walk by the river, and even the quietest person in the room laughed. Now I will pause " +
    "for a moment, and then finish reading this paragraph.",
  it:
    "Questa è la mia voce, registrata su questo computer perché Sussurro possa riconoscermi nelle mie " +
    "registrazioni. Leggo con calma e in modo chiaro, con il ritmo che uso durante una riunione. Lunedì " +
    "mattina abbiamo preparato il bilancio, controllato due volte i numeri e deciso tre domande per la " +
    "prossima chiamata. Dopo, qualcuno ha proposto una breve passeggiata lungo il fiume, e perfino la persona " +
    "più silenziosa della stanza si è messa a ridere. Adesso faccio una piccola pausa, e poi finisco di leggere.",
};

/** The paragraph's language: the dictation language when it is Italian or
 *  English, else (auto-detect, another language) the system's language —
 *  Italian when it is Italian, English otherwise. */
export function enrolLanguage(dictationLanguage: string, systemLanguage: string): EnrolLanguage {
  const pick = (code: string): EnrolLanguage | null => {
    const c = code.trim().toLowerCase();
    if (c.startsWith("it")) return "it";
    if (c.startsWith("en")) return "en";
    return null;
  };
  return pick(dictationLanguage) ?? (pick(systemLanguage) === "it" ? "it" : "en");
}

/** Whether "You" can be told by voice in this item: a single channel. A
 *  browser meeting, or system audio with the mic on its own channel,
 *  already has "You" on the mic (speakers/own_voice.rs single_channel). */
export function singleChannel(item: Pick<Item, "meta" | "segments">): boolean {
  const source = item.meta.source ?? "";
  if (source.startsWith("browser:")) return false;
  return !item.segments.segments.some(
    (s) => s.speaker_id === "you" || (source === "system" && s.channel === "mic"),
  );
}

/** What the speaker panel offers about the user's voice:
 *  - "enrol": record your voice (not enrolled yet);
 *  - "find": look for your voice in this recording (enrolled, no "You" yet);
 *  - "matched": a voice was labelled "You" from your recorded voice;
 *  - "none": nothing (a note, a two-channel recording, no voice data,
 *    still recording or edited outside the app, status unknown). */
export function ownVoiceOffer(
  item: Pick<Item, "meta" | "segments" | "recording" | "edited_externally" | "embedded_segments">,
  status: OwnVoiceStatus | null,
): "enrol" | "find" | "matched" | "none" {
  if (item.meta.type === "note" || item.recording || item.edited_externally) return "none";
  if (!item.embedded_segments || !singleChannel(item)) return "none";
  if (item.segments.speakers.some((s) => s.own_voice === true)) return "matched";
  if (!status) return "none";
  return status.enrolled ? "find" : "enrol";
}

/** Whether the reading can be stopped and saved: most of the target
 *  length has gone by, and never before the minimum speech could have been
 *  said (pauses don't count, so the backend checks the speech itself). */
export function canFinishEnrolment(
  elapsedMs: number,
  status: Pick<OwnVoiceStatus, "min_speech_ms" | "target_ms">,
): boolean {
  return elapsedMs >= Math.max(status.min_speech_ms, status.target_ms * 0.8);
}

/** The recording reached the longest one used: the dialog saves it. */
export function enrolmentFull(elapsedMs: number, status: Pick<OwnVoiceStatus, "max_ms">): boolean {
  return elapsedMs >= status.max_ms;
}

/** The progress bar: share of the target length, 0–100. */
export function enrolmentPercent(elapsedMs: number, status: Pick<OwnVoiceStatus, "target_ms">): number {
  if (status.target_ms <= 0) return 100;
  return Math.max(0, Math.min(100, Math.round((elapsedMs / status.target_ms) * 100)));
}

/** "0:12 / 0:30" under the progress bar. */
export function enrolmentClock(elapsedMs: number, status: Pick<OwnVoiceStatus, "target_ms">): string {
  const fmt = (ms: number) => {
    const s = Math.floor(Math.max(0, ms) / 1000);
    return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, "0")}`;
  };
  return `${fmt(elapsedMs)} / ${fmt(status.target_ms)}`;
}

/** Settings → Voices summary line. */
export function ownVoiceSummary(status: OwnVoiceStatus | null): string {
  if (!status) return "";
  if (!status.enrolled) return "Not recorded. Sussurro doesn't know your voice.";
  const s = Math.round(status.speech_ms / 1000);
  const day = status.updated ? ` on ${status.updated.slice(0, 10)}` : "";
  return `Recorded${day} · ${s} s of speech`;
}
