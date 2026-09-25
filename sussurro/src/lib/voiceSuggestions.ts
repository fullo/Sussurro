/* Voice suggestions in the speaker panel (#242, plan P12): the pure logic
   behind the "Sounds like Anna · Link · Not Anna" chip. The backend
   (`voice_suggestions`) does the matching and remembers the *Not X*
   answers per document; this decides what the panel shows between two
   fetches and when to ask again. A suggestion is never applied: Link goes
   through the speaker panel's usual link path. */

import type { DocSpeaker, Item, Person, Settings, VoiceSuggestion } from "./types";

/** Whether the panel asks for suggestions for this item: not for notes,
 *  not while it records or when it can't be edited (nothing to act on),
 *  only with voice data, and only with the setting on (absent = on). */
export function suggestionsWanted(
  item: Pick<Item, "meta" | "recording" | "edited_externally" | "embedded_segments">,
  settings: Pick<Settings, "voice_suggestions">,
): boolean {
  return (
    item.meta.type !== "note" &&
    !item.recording &&
    !item.edited_externally &&
    (item.embedded_segments ?? 0) > 0 &&
    settings.voice_suggestions !== false
  );
}

/** What the suggestions depend on: the item, its voice data, its speakers
 *  and their links (a link, unlink or Re-detect asks again) and the People
 *  registry (a person added or removed). Same key = no refetch. */
export function suggestionsKey(
  item: Pick<Item, "id" | "embedded_segments" | "segments">,
  people: Pick<Person, "id">[],
): string {
  const speakers = item.segments.speakers.map((s) => `${s.id}=${s.person_id ?? ""}`).join(",");
  const ids = people.map((p) => p.id).sort().join(",");
  return `${item.id}#${item.embedded_segments ?? 0}#${speakers}#${ids}`;
}

/** Key of a *Not X* answer: voice + person, within one document. */
export function dismissKey(speakerId: string, personId: string): string {
  return `${speakerId}|${personId}`;
}

/** Add a *Not X* answer to the ones the panel hides at once (the backend
 *  remembers it too; this keeps the chip from coming back before the
 *  next fetch). Returns a new set. */
export function withDismissal(dismissed: ReadonlySet<string>, speakerId: string, personId: string): Set<string> {
  const next = new Set(dismissed);
  next.add(dismissKey(speakerId, personId));
  return next;
}

/** The chip per speaker: speaker id → the person it sounds like. Only for
 *  a speaker that is still in the document and still unlinked, a person
 *  still in People, and an answer not dismissed since the fetch. */
export function shownSuggestions(
  speakers: DocSpeaker[],
  suggestions: VoiceSuggestion[],
  people: Person[],
  dismissed: ReadonlySet<string> = new Set(),
): Map<string, Person> {
  const out = new Map<string, Person>();
  for (const s of suggestions) {
    const speaker = speakers.find((sp) => sp.id === s.speaker_id);
    if (!speaker || speaker.person_id) continue;
    const person = people.find((p) => p.id === s.person_id);
    if (!person || dismissed.has(dismissKey(s.speaker_id, s.person_id))) continue;
    if (!out.has(s.speaker_id)) out.set(s.speaker_id, person);
  }
  return out;
}

/** "Anna" for "Anna Rossi": the *Not X* button stays short. */
export function shortName(name: string): string {
  return name.trim().split(/\s+/)[0] || name;
}
