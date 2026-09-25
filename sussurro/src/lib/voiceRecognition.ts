/* *Recognise this voice* (People, #242, plan P12/P13) and Settings →
   Privacy → Voices: pure helpers for the per-person toggle, its status
   line and the explanation of what is stored. The backend owns the
   profiles (`voice_status`, `voice_set_enabled`, `voice_forget`,
   `voices_forget_all`); the UI only ever sees `VoiceStatus`. */

import type { VoiceStatus } from "./types";

/** 45_000 → "45 s", 180_000 → "3 min", 95_000 → "1 min 35 s". */
export function formatSpeech(ms: number): string {
  const s = Math.max(0, Math.round(ms / 1000));
  if (s < 60) return `${s} s`;
  const m = Math.floor(s / 60);
  const rest = s % 60;
  return rest ? `${m} min ${rest} s` : `${m} min`;
}

const docs = (n: number) => `${n} document${n === 1 ? "" : "s"}`;

/** The status line under the toggle. */
export function voiceStatusLabel(s: VoiceStatus | null | undefined): string {
  if (!s || !s.enabled) return "Off: this person's voice is not recognised.";
  if (s.ready) {
    return `Ready: ${formatSpeech(s.speech_ms)} of confirmed speech from ${docs(s.documents)}. Suggestions are on.`;
  }
  if (s.speech_ms === 0) {
    return `Learning: no confirmed speech yet. Link this person to a voice in a meeting or transcription; suggestions start at ${formatSpeech(s.min_speech_ms)} from ${docs(s.min_documents)}.`;
  }
  return `Learning: ${formatSpeech(s.speech_ms)} of ${formatSpeech(s.min_speech_ms)} of confirmed speech, from ${s.documents} of ${docs(s.min_documents)}. No suggestions until then.`;
}

/** The small marker in the People list ("" when off). */
export function voiceBadge(s: VoiceStatus | null | undefined): string {
  if (!s || !s.enabled) return "";
  return s.ready ? "voice recognised" : "learning voice";
}

/** What flipping the toggle does: turning it on first shows the sheet
 *  (what is stored, where, how to delete it, tell the person) and asks to
 *  confirm; turning it off deletes the profile at once. */
export function toggleAction(enabled: boolean, next: boolean): "confirm" | "off" | "none" {
  if (next === enabled) return "none";
  return next ? "confirm" : "off";
}

/** Statuses by person id, for the People list. */
export function statusesById(list: VoiceStatus[]): Record<string, VoiceStatus> {
  return Object.fromEntries(list.map((s) => [s.person_id, s]));
}

/** The first-use sheet and Settings → Privacy share this wording (P13). */
export const VOICE_STORED =
  "A voice profile is a summary of how this person sounds (a list of numbers, not a recording), built only from the lines you linked to them in meetings and transcriptions.";
export const VOICE_WHERE =
  "It stays on this computer, in Sussurro's app data folder — never in the archive folder, never in exports, the local API or a sync. Another computer starts again from your links.";
export const VOICE_USE =
  "Sussurro uses it only to suggest “Voice 2 sounds like …” in the speaker panel. Nothing is linked until you click Link.";
export const VOICE_DELETE =
  "Turn it off, click Forget this voice, delete the person, or use Settings → Privacy → Forget all voices: the profile is deleted at once, not moved to the trash.";
export const VOICE_TELL =
  "A voice profile that identifies someone is biometric data under the GDPR (art. 9): tell the person and ask for their consent before turning this on, unless you use it only for yourself.";

/** Confirmation text of *Forget all voices*. */
export function forgetAllPrompt(count: number): string {
  if (count === 0) return "There are no voice profiles. Forget any “Not this person” answers too?";
  return `Delete ${count === 1 ? "the voice profile" : `all ${count} voice profiles`} on this computer? Recognition turns off for everyone; your links in the archive stay, so you can turn it on again later.`;
}
