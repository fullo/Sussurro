import type { DocSpeaker, Person } from "../lib/types";
import { shortName } from "../lib/voiceSuggestions";

/** "Sounds like Anna Rossi · Link · Not Anna" under an unlinked voice in
 *  the speaker panel (#242, P12): a suggestion from a known voice, never
 *  applied without the Link click, which takes the panel's usual link
 *  path (participants and emails follow as for any link). */
export function VoiceSuggestionChip({
  speaker,
  person,
  busy,
  onLink,
  onDismiss,
}: {
  speaker: DocSpeaker;
  person: Person;
  busy: boolean;
  onLink: () => void;
  onDismiss: () => void;
}) {
  const who = person.email ? `${person.name} <${person.email}>` : person.name;
  return (
    <span className="spk-voice-sugg" role="group" aria-label={`${speaker.label} sounds like ${person.name}`}>
      <span className="spk-voice-sugg-text" title="Suggested from a known voice (People → Recognise this voice). Nothing is linked until you click Link.">
        Sounds like <b>{person.name}</b>
      </span>
      <button type="button" className="link-btn" disabled={busy} onClick={onLink} title={`Link ${speaker.label} to ${who}`}>
        Link
      </button>
      <button
        type="button"
        className="link-btn"
        disabled={busy}
        onClick={onDismiss}
        title={`Don't suggest ${person.name} for ${speaker.label} in this document again`}
      >
        Not {shortName(person.name)}
      </button>
    </span>
  );
}
