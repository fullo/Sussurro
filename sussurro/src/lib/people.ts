/* Pure helpers for the People registry (#132): matching, the participant
 * chips' link / "Add to People" actions, and the People screen's search,
 * editor validation and duplicate finder.
 *
 * Matching mirrors archive/people.rs (`name_key`, `match_person`): a name
 * matches a person's name or alias ignoring case, accents and extra
 * whitespace, and a name matching more than one person links to nobody. */

import { isValidEmail } from "./participants";
import type { Participant, Person } from "./types";

/** Comparison key: accents dropped (NFKD minus combining marks), lowercase,
 *  whitespace trimmed and collapsed. */
export function nameKey(s: string): string {
  return s
    .normalize("NFKD")
    .replace(/\p{M}/gu, "")
    .toLowerCase()
    .split(/\s+/)
    .filter(Boolean)
    .join(" ");
}

const emailKey = (s: string | undefined) => (s ?? "").trim().toLowerCase();

/** Does `display` match the person's name or one of its aliases? */
export function personMatches(p: Person, display: string): boolean {
  const key = nameKey(display);
  return key !== "" && (nameKey(p.name) === key || p.aliases.some((a) => nameKey(a) === key));
}

/** The one person `display` matches, or null (nobody, or ambiguous). */
export function matchPerson(people: Person[], display: string): Person | null {
  const hits = people.filter((p) => personMatches(p, display));
  return hits.length === 1 ? hits[0] : null;
}

/** The registry entry a participant is: same email, else its name. */
export function personFor(people: Person[], p: Participant): Person | null {
  if (p.email) {
    const byEmail = people.find((q) => q.email && emailKey(q.email) === emailKey(p.email));
    if (byEmail) return byEmail;
  }
  return matchPerson(people, p.name);
}

/** The email a one-click "link" would give this chip: only for a
 *  participant without one whose name matches a person that has one. */
export function linkEmail(people: Person[], p: Participant): string | null {
  if (p.email) return null;
  return matchPerson(people, p.name)?.email ?? null;
}

/** Link participant `index` to the registry (adds the email only). */
export function linkParticipant(list: Participant[], index: number, people: Person[]): Participant[] {
  const email = list[index] ? linkEmail(people, list[index]) : null;
  if (!email) return list;
  return list.map((p, i) => (i === index ? { ...p, email } : p));
}

/** "Add to People" is offered for a participant that is nobody in the
 *  registry yet — neither by email nor by any name or alias (an ambiguous
 *  name is already there, twice). */
export function canAddToPeople(people: Person[], p: Participant): boolean {
  if (!nameKey(p.name)) return false;
  if (p.email && people.some((q) => q.email && emailKey(q.email) === emailKey(p.email))) return false;
  return !people.some((q) => personMatches(q, p.name));
}

/** A new registry entry from a participant chip. */
export function personFromParticipant(p: Participant): Person {
  return { id: "", name: p.name.trim(), email: p.email?.trim() || undefined, aliases: [] };
}

/** Registry entries offered as the chip editor's autocomplete. */
export function peopleSuggestions(people: Person[]): Participant[] {
  return people.map((p) => (p.email ? { name: p.name, email: p.email } : { name: p.name }));
}

/** People screen search: name, alias or email contains the query
 *  (accent- and case-insensitive). */
export function filterPeople(people: Person[], query: string): Person[] {
  const q = nameKey(query);
  if (!q) return people;
  return people.filter(
    (p) => nameKey(p.name).includes(q) || p.aliases.some((a) => nameKey(a).includes(q)) || emailKey(p.email).includes(q),
  );
}

/** Problems that keep the editor's Save disabled; mirrors `clean` and
 *  `check_conflicts` in archive/people.rs. Empty = savable. */
export function personProblems(draft: Person, people: Person[]): string[] {
  const out: string[] = [];
  const key = nameKey(draft.name);
  if (!key) out.push("Type a name.");
  const email = draft.email?.trim() ?? "";
  if (email && !isValidEmail(email)) out.push(`“${email}” doesn't look like an email.`);
  for (const other of people) {
    if (other.id === draft.id) continue;
    if (key && nameKey(other.name) === key) out.push(`${other.name} is already in People — edit that entry or merge the two.`);
    else if (email && other.email && emailKey(other.email) === emailKey(email))
      out.push(`${other.email} already belongs to ${other.name}.`);
  }
  return out;
}

/** Aliases typed as a list: one per line or comma, trimmed, no repeats,
 *  none equal to the name. */
export function parseAliases(text: string, name = ""): string[] {
  const seen = new Set([nameKey(name)]);
  const out: string[] = [];
  for (const raw of text.split(/[\n,;]/)) {
    const a = raw.trim().replace(/\s+/g, " ");
    const k = nameKey(a);
    if (!k || seen.has(k)) continue;
    seen.add(k);
    out.push(a);
  }
  return out;
}

/** Groups of likely duplicates (same email, or one's name is the other's
 *  name or alias), each listed once, for the "merge" hint. */
export function findDuplicates(people: Person[]): Person[][] {
  const parent = people.map((_, i) => i);
  const find = (i: number): number => (parent[i] === i ? i : (parent[i] = find(parent[i])));
  const same = (a: Person, b: Person) =>
    (a.email && b.email && emailKey(a.email) === emailKey(b.email)) ||
    personMatches(b, a.name) ||
    personMatches(a, b.name);
  for (let i = 0; i < people.length; i++)
    for (let j = i + 1; j < people.length; j++) if (same(people[i], people[j])) parent[find(j)] = find(i);
  const groups = new Map<number, Person[]>();
  people.forEach((p, i) => {
    const r = find(i);
    groups.set(r, [...(groups.get(r) ?? []), p]);
  });
  return [...groups.values()].filter((g) => g.length > 1);
}

/** Preview of a merge (what archive/people.rs `merge_people` stores): the
 *  others' names and aliases become aliases, the first email wins. */
export function mergePreview(into: Person, others: Person[]): Person {
  const aliases = parseAliases([...into.aliases, ...others.flatMap((o) => [o.name, ...o.aliases])].join("\n"), into.name);
  const email = into.email || others.find((o) => o.email)?.email;
  return { ...into, email, aliases };
}

/** "Appears in 3 items" / "Not in any item yet". */
export function usageLabel(n: number | undefined): string {
  if (n === undefined) return "";
  if (n === 0) return "Not in any item yet";
  return `Appears in ${n} item${n === 1 ? "" : "s"}`;
}
