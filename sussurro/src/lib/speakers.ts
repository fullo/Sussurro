/* Speakers of a document (#130): the pure logic behind the speaker panel
   and the line chips. Components only wire these to invoke(). */

import { canAddToPeople, isGenericSpeaker, matchPerson } from "./people";
import type { DocSpeaker, Item, Person, Segment, VoiceSource } from "./types";

/** Line-move target that opens a new "Voice N" (speakers/doc.rs NEW_VOICE). */
export const NEW_VOICE = "voice:new";
/** Longest speaker name (speakers/doc.rs MAX_LABEL_CHARS). */
export const MAX_LABEL_CHARS = 60;

/** Whether an item shows the speaker panel (and, once it has speakers, the
 *  line chips and "Move to speaker"): every item but a note (P10: a note
 *  is the user's own voice). Transcriptions always (P11, #134 —
 *  "Identify voices" lives in the panel); meetings always (#138). */
export function speakersEnabled(item: Pick<Item, "meta">): boolean {
  return item.meta.type !== "note";
}

/** The run's "Identify voices" argument (#134): only transcriptions are
 *  labelled; a note never is, whatever the checkbox said before the type
 *  changed. */
export function identifyVoicesArg(itemType: string, checked: boolean): boolean {
  return itemType === "transcription" && checked;
}

/** What the speaker panel offers for a transcription without voice data
 *  (#134): "identify" = the button; "explain" = why not (the backend's
 *  reason); "none" = nothing to offer (it has voice data — Re-detect — or
 *  is not a transcription). */
export function identifyOffer(
  item: Pick<Item, "meta" | "embedded_segments">,
  source: VoiceSource | null,
): "identify" | "explain" | "none" {
  if (item.meta.type !== "transcription" || (item.embedded_segments ?? 0) > 0 || !source) return "none";
  return source.available ? "identify" : "explain";
}

/** Where a speaker's name comes from, for the panel's small print. */
export function speakerSource(id: string): string {
  if (id === "you") return "mic channel";
  // Names from the meeting page, per platform (#131, #245, #246).
  if (id.startsWith("meet:")) return "from Meet";
  if (id.startsWith("teams:")) return "from Teams";
  if (id.startsWith("zoom:")) return "from Zoom";
  if (id.startsWith("voice:")) return "by voice";
  return "";
}

export function isVoice(id: string): boolean {
  return /^voice:[1-9]\d*$/.test(id);
}

export interface SpeakerShare {
  speaker: DocSpeaker;
  /** Speech attributed to the speaker, ms. */
  ms: number;
  /** Lines attributed to the speaker. */
  lines: number;
  /** Share of all attributed speech, 0–100, rounded. */
  percent: number;
}

const lineMs = (s: Segment) => Math.max(0, s.end_ms - s.start_ms);

/** Each speaker of the document with its share of speech (lines with text
 *  only), in the document's speaker order. Percentages are rounded to
 *  whole numbers; a speaker with no line left shows 0 %. */
export function speakerShares(item: Pick<Item, "segments">): SpeakerShare[] {
  const { speakers, segments } = item.segments;
  const ms = new Map<string, number>();
  const lines = new Map<string, number>();
  let total = 0;
  for (const s of segments) {
    if (!s.speaker_id || !s.text.trim()) continue;
    const d = lineMs(s);
    ms.set(s.speaker_id, (ms.get(s.speaker_id) ?? 0) + d);
    lines.set(s.speaker_id, (lines.get(s.speaker_id) ?? 0) + 1);
    total += d;
  }
  return speakers.map((speaker) => {
    const m = ms.get(speaker.id) ?? 0;
    return {
      speaker,
      ms: m,
      lines: lines.get(speaker.id) ?? 0,
      percent: total > 0 ? Math.round((m / total) * 100) : 0,
    };
  });
}

/** Speaker of each line id, for the transcript chips. */
export function speakerOfLine(item: Pick<Item, "segments">): Map<number, DocSpeaker> {
  const byId = new Map(item.segments.speakers.map((s) => [s.id, s]));
  const out = new Map<number, DocSpeaker>();
  for (const s of item.segments.segments) {
    const sp = s.speaker_id ? byId.get(s.speaker_id) : undefined;
    if (sp) out.set(s.id, sp);
  }
  return out;
}

/** Why a speaker name can't be saved, or "" when it can. An empty name is
 *  fine for a voice (it gets its "Voice N" name back). */
export function labelProblem(id: string, label: string): string {
  const l = label.trim().replace(/\s+/g, " ");
  if (!l && !isVoice(id)) return "A speaker needs a name.";
  if ([...l].length > MAX_LABEL_CHARS) return `At most ${MAX_LABEL_CHARS} characters.`;
  return "";
}

/** Why "Re-detect speakers" is not available on this item, or "". */
export function redetectBlocked(item: Pick<Item, "recording" | "edited_externally" | "embedded_segments">): string {
  if (item.recording) return "Available when the recording ends.";
  if (item.edited_externally) return "The transcript was edited outside Sussurro.";
  if (!item.embedded_segments) return "This recording has no voice data: speakers can only be detected on recordings made with speaker labels on.";
  return "";
}

/* ---------- People links (#132) ---------- */

/** The registry entry a speaker is linked to, if it is still there. */
export function linkedPerson(speaker: DocSpeaker, people: Person[]): Person | null {
  return speaker.person_id ? (people.find((p) => p.id === speaker.person_id) ?? null) : null;
}

/** The person to suggest for an unlinked speaker: its label (a Meet name,
 *  or a name the user typed) matches exactly one person. Generic labels
 *  ("Voice 2", "You") never suggest anyone. */
export function linkSuggestion(speaker: DocSpeaker, people: Person[]): Person | null {
  if (speaker.person_id || isGenericSpeaker(speaker.label)) return null;
  return matchPerson(people, speaker.label);
}

/** "Add to People" is offered for an unlinked speaker the user named
 *  (not a generic "Voice N") who is nobody in the registry yet. */
export function canAddSpeakerToPeople(speaker: DocSpeaker, people: Person[]): boolean {
  return !speaker.person_id && canAddToPeople(people, { name: speaker.label });
}

/** People offered by "Link to person…", sorted by name; the suggested one
 *  (if any) first. */
export function linkChoices(speaker: DocSpeaker, people: Person[]): Person[] {
  const first = linkSuggestion(speaker, people);
  const rest = people
    .filter((p) => p.id !== first?.id)
    .slice()
    .sort((a, b) => a.name.localeCompare(b.name));
  return first ? [first, ...rest] : rest;
}
