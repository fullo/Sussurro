import { describe, expect, it } from "vitest";
import {
  canAddToPeople,
  filterPeople,
  findDuplicates,
  linkEmail,
  linkParticipant,
  matchPerson,
  mergePreview,
  nameKey,
  parseAliases,
  peopleSuggestions,
  personFor,
  personFromParticipant,
  personProblems,
  usageLabel,
} from "./people";
import type { Person } from "./types";

const person = (id: string, name: string, email?: string, aliases: string[] = []): Person => ({ id, name, email, aliases });

const anna = person("p-1", "Nicolò Rossi", "nico@example.com", ["Nico", "N. Rossi"]);
const marcoB = person("p-2", "Marco Bianchi", "mb@example.com", ["Marco"]);
const marcoV = person("p-3", "Marco Verdi", undefined, ["marco"]);
const people = [anna, marcoB, marcoV];

describe("nameKey / matchPerson", () => {
  it("folds case, accents and whitespace like archive/people.rs", () => {
    expect(nameKey("  Nicolò   CÀRDENAS ")).toBe("nicolo cardenas");
    expect(nameKey("Zoë\tMüller")).toBe("zoe muller");
    expect(nameKey("ﬁona")).toBe("fiona");
    expect(nameKey("Łukasz")).toBe("łukasz");
    expect(nameKey("   ")).toBe("");
  });

  it("matches names and aliases, never an ambiguous one", () => {
    for (const d of ["nicolo rossi", "NICOLÒ  ROSSI", "nico", "n. rossi"]) expect(matchPerson(people, d)?.id, d).toBe("p-1");
    expect(matchPerson(people, "Marco")).toBeNull();
    expect(matchPerson(people, "marco verdi")?.id).toBe("p-3");
    expect(matchPerson(people, "Rossi")).toBeNull();
    expect(matchPerson(people, "")).toBeNull();
  });

  it("finds a participant's entry by email first, then by name", () => {
    expect(personFor(people, { name: "Someone", email: "MB@example.com" })?.id).toBe("p-2");
    expect(personFor(people, { name: "Nico" })?.id).toBe("p-1");
    expect(personFor(people, { name: "Voice 2" })).toBeNull();
  });
});

describe("linking from the chip editor", () => {
  it("offers a link only to chips without an email that match a person with one", () => {
    expect(linkEmail(people, { name: "nico" })).toBe("nico@example.com");
    expect(linkEmail(people, { name: "nico", email: "x@example.com" })).toBeNull();
    expect(linkEmail(people, { name: "Marco Verdi" })).toBeNull(); // no email on file
    expect(linkEmail(people, { name: "Marco" })).toBeNull(); // ambiguous
  });

  it("adds the email, keeps the name and the other chips", () => {
    const list = [{ name: "Ospite" }, { name: "N. Rossi" }];
    expect(linkParticipant(list, 1, people)).toEqual([{ name: "Ospite" }, { name: "N. Rossi", email: "nico@example.com" }]);
    expect(linkParticipant(list, 0, people)).toBe(list);
    expect(linkParticipant(list, 5, people)).toBe(list);
  });

  it("offers Add to People only for someone not in the registry", () => {
    expect(canAddToPeople(people, { name: "Ospite" })).toBe(true);
    expect(canAddToPeople(people, { name: "Nico" })).toBe(false);
    expect(canAddToPeople(people, { name: "Marco" })).toBe(false);
    expect(canAddToPeople(people, { name: "New Name", email: "NICO@example.com" })).toBe(false);
    expect(canAddToPeople(people, { name: "  " })).toBe(false);
    for (const generic of ["Voice 1", "voice  12", "Voce 3", "Speaker 2", "You"]) expect(canAddToPeople(people, { name: generic }), generic).toBe(false);
    expect(canAddToPeople(people, { name: "Voice of Reason" })).toBe(true);
    expect(personFromParticipant({ name: " Ospite ", email: " o@example.com " })).toEqual({
      id: "",
      name: "Ospite",
      email: "o@example.com",
      aliases: [],
    });
    expect(personFromParticipant({ name: "Ospite" }).email).toBeUndefined();
  });

  it("turns the registry into autocomplete suggestions", () => {
    expect(peopleSuggestions([anna, marcoV])).toEqual([{ name: "Nicolò Rossi", email: "nico@example.com" }, { name: "Marco Verdi" }]);
  });
});

describe("People screen", () => {
  it("searches name, aliases and email without accents or case", () => {
    expect(filterPeople(people, "nicolo").map((p) => p.id)).toEqual(["p-1"]);
    expect(filterPeople(people, "N. ROSS").map((p) => p.id)).toEqual(["p-1"]);
    expect(filterPeople(people, "mb@").map((p) => p.id)).toEqual(["p-2"]);
    expect(filterPeople(people, "marco").map((p) => p.id)).toEqual(["p-2", "p-3"]);
    expect(filterPeople(people, "  ")).toBe(people);
  });

  it("validates the editor like the backend", () => {
    expect(personProblems(person("", "Ospite"), people)).toEqual([]);
    expect(personProblems(person("", " "), people)).toEqual(["Type a name."]);
    expect(personProblems(person("", "X", "nope"), people)[0]).toMatch(/doesn't look like an email/);
    expect(personProblems(person("", "nicolo rossi"), people)[0]).toMatch(/already in People/);
    expect(personProblems(person("", "Other", "MB@example.com"), people)[0]).toMatch(/already belongs to Marco Bianchi/);
    // Editing an entry never conflicts with itself.
    expect(personProblems({ ...anna, aliases: [] }, people)).toEqual([]);
  });

  it("parses aliases one per line or comma", () => {
    expect(parseAliases("Annie, annie\n  Anna  R. ;;Anna Rossi", "Anna Rossi")).toEqual(["Annie", "Anna R."]);
    expect(parseAliases("")).toEqual([]);
  });

  it("finds duplicate groups by email or name/alias", () => {
    const dupA = person("p-4", "Nico", undefined, []);
    const dupB = person("p-5", "Mario", "MB@example.com");
    const groups = findDuplicates([anna, marcoB, marcoV, dupA, dupB]);
    const ids = groups.map((g) => g.map((p) => p.id).sort());
    // A shared alias alone ("Marco") is not a duplicate: it is just ambiguous.
    expect(ids).toEqual([
      ["p-1", "p-4"],
      ["p-2", "p-5"],
    ]);
    expect(findDuplicates([anna, person("p-9", "Carla")])).toEqual([]);
  });

  it("previews a merge like the backend", () => {
    const into = person("p-1", "Anna Rossi", undefined, ["Annie"]);
    const other = person("p-2", "Anna R.", "anna@example.com", ["Annie", "A.R."]);
    expect(mergePreview(into, [other])).toEqual({ id: "p-1", name: "Anna Rossi", email: "anna@example.com", aliases: ["Annie", "Anna R.", "A.R."] });
    expect(mergePreview({ ...into, email: "keep@example.com" }, [other]).email).toBe("keep@example.com");
  });

  it("labels usage", () => {
    expect(usageLabel(undefined)).toBe("");
    expect(usageLabel(0)).toBe("Not in any item yet");
    expect(usageLabel(1)).toBe("Appears in 1 item");
    expect(usageLabel(4)).toBe("Appears in 4 items");
  });
});
