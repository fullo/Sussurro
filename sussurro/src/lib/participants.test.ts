import { describe, expect, it } from "vitest";
import {
  addParticipants,
  formatParticipant,
  hasParticipants,
  isValidEmail,
  nameFromEmail,
  parseParticipant,
  removeParticipant,
  splitParticipantInput,
  updateParticipant,
} from "./participants";

const anna = { name: "Anna Rossi", email: "anna@example.com" };

describe("hasParticipants", () => {
  it("is off for notes only (P10)", () => {
    expect(hasParticipants("note")).toBe(false);
    expect(hasParticipants("transcription")).toBe(true);
    expect(hasParticipants("meeting")).toBe(true);
  });
});

describe("isValidEmail / nameFromEmail", () => {
  it("accepts plain addresses and refuses the obvious mistakes", () => {
    expect(isValidEmail("anna@example.com")).toBe(true);
    expect(isValidEmail(" a.b+c@studio.example.it ")).toBe(true);
    for (const bad of ["anna", "anna@", "@example.com", "anna@example", "an na@example.com", "a@b.c.", "a@@b.com"]) {
      expect(isValidEmail(bad), bad).toBe(false);
    }
  });

  it("guesses a readable name", () => {
    expect(nameFromEmail("anna.rossi@example.com")).toBe("Anna Rossi");
    expect(nameFromEmail("marco_b+meet@example.com")).toBe("Marco B Meet");
  });
});

describe("parseParticipant", () => {
  it("reads Name <email>", () => {
    expect(parseParticipant("  Anna Rossi <anna@example.com> ")).toEqual({ ok: true, participant: anna });
    expect(parseParticipant('"Rossi, Anna" <anna@example.com>')).toEqual({
      ok: true,
      participant: { name: "Rossi, Anna", email: "anna@example.com" },
    });
  });

  it("reads a bare name, a bare email, and <email> alone", () => {
    expect(parseParticipant("Ospite")).toEqual({ ok: true, participant: { name: "Ospite" } });
    expect(parseParticipant("anna.rossi@example.com")).toEqual({
      ok: true,
      participant: { name: "Anna Rossi", email: "anna.rossi@example.com" },
    });
    expect(parseParticipant("<anna@example.com>")).toEqual({
      ok: true,
      participant: { name: "Anna", email: "anna@example.com" },
    });
    // Empty brackets: just the name.
    expect(parseParticipant("Anna <>")).toEqual({ ok: true, participant: { name: "Anna" } });
  });

  it("refuses empty input and malformed emails", () => {
    expect(parseParticipant("   ").ok).toBe(false);
    const bad = parseParticipant("Anna <anna@nowhere>");
    expect(bad.ok).toBe(false);
    if (!bad.ok) expect(bad.error).toContain("anna@nowhere");
    expect(parseParticipant("Anna <anna@example.com").ok).toBe(false);
    // Two addresses run together are not one name.
    expect(parseParticipant("Anna <a@example.com>Bob <b@example.com>").ok).toBe(false);
  });
});

describe("splitParticipantInput", () => {
  it("splits pasted lists but not inside <>", () => {
    expect(splitParticipantInput("Anna <anna@example.com>, Marco; Luca\nGiulia <g@x.io>")).toEqual([
      "Anna <anna@example.com>",
      "Marco",
      "Luca",
      "Giulia <g@x.io>",
    ]);
    expect(splitParticipantInput(" , ;")).toEqual([]);
  });
});

describe("addParticipants / updateParticipant / removeParticipant", () => {
  it("dedupes by email, else by name, case-insensitively", () => {
    const list = addParticipants([anna], [
      { name: "Anna R.", email: "ANNA@example.com" }, // same email
      { name: "anna rossi" }, // same name, no email
      { name: "Marco" },
      { name: "marco" },
    ]);
    expect(list).toEqual([anna, { name: "Marco" }]);
  });

  it("fills in the email of a name listed without one", () => {
    expect(addParticipants([{ name: "Marco" }], [{ name: "marco", email: "m@example.com" }])).toEqual([
      { name: "Marco", email: "m@example.com" },
    ]);
  });

  it("keeps two people with the same name and different emails", () => {
    const list = addParticipants([{ name: "Marco", email: "m1@example.com" }], [{ name: "Marco", email: "m2@example.com" }]);
    expect(list).toHaveLength(2);
  });

  it("edits in place and refuses an edit that duplicates another entry", () => {
    const list = [anna, { name: "Marco" }];
    const r = updateParticipant(list, 1, { name: "Marco", email: "marco@example.com" });
    expect(r).toEqual({ ok: true, list: [anna, { name: "Marco", email: "marco@example.com" }] });
    const dup = updateParticipant(list, 1, { name: "X", email: "anna@example.com" });
    expect(dup.ok).toBe(false);
    // Re-saving an entry unchanged is fine.
    expect(updateParticipant(list, 0, anna).ok).toBe(true);
  });

  it("removes by position and formats for editing", () => {
    expect(removeParticipant([anna, { name: "Marco" }], 0)).toEqual([{ name: "Marco" }]);
    expect(formatParticipant(anna)).toBe("Anna Rossi <anna@example.com>");
    expect(formatParticipant({ name: "Marco" })).toBe("Marco");
    // What formatParticipant writes, parseParticipant reads back.
    expect(parseParticipant(formatParticipant(anna))).toEqual({ ok: true, participant: anna });
  });
});
