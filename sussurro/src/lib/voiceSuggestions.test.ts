import { describe, expect, it } from "vitest";
import {
  dismissKey,
  shortName,
  shownSuggestions,
  suggestionsKey,
  suggestionsWanted,
  withDismissal,
} from "./voiceSuggestions";
import type { DocSpeaker, Item, Person, VoiceSuggestion } from "./types";

const anna: Person = { id: "p-anna", name: "Anna Rossi", email: "anna@example.com", aliases: [] };
const bruno: Person = { id: "p-bruno", name: "Bruno", aliases: [] };
const people = [anna, bruno];
const sp = (id: string, person_id?: string): DocSpeaker => ({ id, label: id.replace("voice:", "Voice "), color: "", person_id });
const sugg = (speaker_id: string, person_id: string): VoiceSuggestion => ({ speaker_id, person_id });

const item = (over: Partial<Item> = {}, type = "meeting"): Item =>
  ({
    id: "2026/09/weekly",
    meta: { type },
    recording: false,
    edited_externally: false,
    embedded_segments: 12,
    segments: { speakers: [sp("voice:1"), sp("voice:2")], segments: [] },
    ...over,
  }) as unknown as Item;

describe("suggestionsWanted", () => {
  it("asks for meetings and transcriptions with voice data, setting on or absent", () => {
    expect(suggestionsWanted(item(), {})).toBe(true);
    expect(suggestionsWanted(item(), { voice_suggestions: true })).toBe(true);
    expect(suggestionsWanted(item({}, "transcription"), {})).toBe(true);
  });

  it("never for notes, recording or externally edited items, without voice data or with the setting off", () => {
    expect(suggestionsWanted(item({}, "note"), {})).toBe(false);
    expect(suggestionsWanted(item({ recording: true }), {})).toBe(false);
    expect(suggestionsWanted(item({ edited_externally: true }), {})).toBe(false);
    expect(suggestionsWanted(item({ embedded_segments: 0 }), {})).toBe(false);
    expect(suggestionsWanted(item(), { voice_suggestions: false })).toBe(false);
  });
});

describe("suggestionsKey", () => {
  const key = (i: Item, ps: Person[] = people) => suggestionsKey(i, ps);

  it("changes on a link, an unlink, a re-detect, new voice data or a People change", () => {
    const base = key(item());
    const linked = item({ segments: { speakers: [sp("voice:1", "p-anna"), sp("voice:2")], segments: [] } } as unknown as Partial<Item>);
    expect(key(linked)).not.toBe(base);
    const redetected = item({ segments: { speakers: [sp("voice:1"), sp("voice:2"), sp("voice:3")], segments: [] } } as unknown as Partial<Item>);
    expect(key(redetected)).not.toBe(base);
    expect(key(item({ embedded_segments: 13 }))).not.toBe(base);
    expect(key(item(), [anna])).not.toBe(base);
    expect(key(item({ id: "other" }))).not.toBe(base);
  });

  it("stays the same for the same state, whatever order People come in", () => {
    expect(key(item(), [bruno, anna])).toBe(key(item(), [anna, bruno]));
  });
});

describe("shownSuggestions", () => {
  const speakers = [sp("voice:1"), sp("voice:2"), sp("voice:3", "p-bruno")];

  it("maps each unlinked voice to its person", () => {
    const shown = shownSuggestions(speakers, [sugg("voice:1", "p-anna"), sugg("voice:2", "p-bruno")], people);
    expect([...shown.entries()].map(([k, p]) => [k, p.id])).toEqual([
      ["voice:1", "p-anna"],
      ["voice:2", "p-bruno"],
    ]);
  });

  it("drops a voice linked meanwhile, a speaker gone, a person gone", () => {
    const shown = shownSuggestions(
      speakers,
      [sugg("voice:3", "p-anna"), sugg("voice:9", "p-anna"), sugg("voice:1", "p-gone")],
      people,
    );
    expect(shown.size).toBe(0);
  });

  it("hides a dismissed answer at once, only for that voice and person", () => {
    const list = [sugg("voice:1", "p-anna"), sugg("voice:2", "p-anna")];
    const dismissed = withDismissal(new Set(), "voice:1", "p-anna");
    const shown = shownSuggestions(speakers, list, people, dismissed);
    expect([...shown.keys()]).toEqual(["voice:2"]);
  });

  it("never on a voice labelled You from your own voice", () => {
    const you = { ...sp("voice:1"), label: "You", own_voice: true };
    const shown = shownSuggestions([you, sp("voice:2")], [sugg("voice:1", "p-anna"), sugg("voice:2", "p-bruno")], people);
    expect([...shown.keys()]).toEqual(["voice:2"]);
  });

  it("keeps the first suggestion when a voice appears twice", () => {
    const shown = shownSuggestions(speakers, [sugg("voice:1", "p-anna"), sugg("voice:1", "p-bruno")], people);
    expect(shown.get("voice:1")?.id).toBe("p-anna");
  });
});

describe("dismissal memory", () => {
  it("adds without touching the previous set and is idempotent", () => {
    const a = new Set<string>();
    const b = withDismissal(a, "voice:2", "p-anna");
    expect(a.size).toBe(0);
    expect(b.has(dismissKey("voice:2", "p-anna"))).toBe(true);
    const c = withDismissal(b, "voice:2", "p-anna");
    expect(c.size).toBe(1);
    expect(withDismissal(c, "voice:2", "p-bruno").size).toBe(2);
  });

  it("keys voice and person apart", () => {
    expect(dismissKey("voice:1", "p-2")).not.toBe(dismissKey("voice:12", "p-"));
  });
});

describe("shortName", () => {
  it("takes the first word for the Not button", () => {
    expect(shortName("Anna Rossi")).toBe("Anna");
    expect(shortName("  Bruno ")).toBe("Bruno");
    expect(shortName("")).toBe("");
  });
});
