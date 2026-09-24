/* Pure helpers for the participant chips of the document header (#124).
 *
 * The editor is free text for now: "Anna Rossi <anna@example.com>",
 * "Anna Rossi" or a bare "anna@example.com". The People registry (#132)
 * plugs in by passing suggestions to the editor; everything here stays the
 * same because a suggestion is just a Participant. */

import type { ItemType, Participant } from "./types";

/** P10: meetings and transcriptions carry participants, notes never do. */
export function hasParticipants(type: ItemType): boolean {
  return type !== "note";
}

/** Basic shape check, not RFC 5322: something@domain.tld, no spaces or <>. */
export function isValidEmail(email: string): boolean {
  return /^[^\s@<>,;]+@[^\s@<>,;]+\.[^\s@<>,;.]+$/.test(email.trim());
}

/** "anna.rossi" → "Anna Rossi": a readable name guessed from an email. */
export function nameFromEmail(email: string): string {
  const local = email.trim().split("@")[0] ?? "";
  return local
    .split(/[._+-]+/)
    .filter(Boolean)
    .map((w) => w.charAt(0).toUpperCase() + w.slice(1))
    .join(" ");
}

export type ParseResult = { ok: true; participant: Participant } | { ok: false; error: string };

/** One participant from "Name <email>", "Name" or "email". */
export function parseParticipant(input: string): ParseResult {
  const text = input.trim();
  if (!text) return { ok: false, error: "Type a name." };
  const angled = /^(.*?)\s*<([^<>]*)>\s*$/.exec(text);
  let name: string;
  let email: string;
  if (angled) {
    name = angled[1].trim().replace(/^"(.*)"$/, "$1").trim();
    email = angled[2].trim();
  } else if (text.includes("@") && !/\s/.test(text)) {
    name = "";
    email = text;
  } else {
    if (/[<>]/.test(text)) return { ok: false, error: "Write the email as Name <name@example.com>." };
    return { ok: true, participant: { name: text } };
  }
  if (/[<>]/.test(name)) return { ok: false, error: "One participant at a time: separate them with commas." };
  if (email && !isValidEmail(email)) return { ok: false, error: `“${email}” doesn't look like an email.` };
  if (!name) name = email ? nameFromEmail(email) : "";
  if (!name) return { ok: false, error: "Type a name." };
  return { ok: true, participant: email ? { name, email } : { name } };
}

/** Several participants separated by commas, semicolons or new lines
 *  (a pasted address list). Separators inside <…> don't split. */
export function splitParticipantInput(input: string): string[] {
  const out: string[] = [];
  let cur = "";
  let depth = 0;
  for (const ch of input) {
    if (ch === "<") depth++;
    else if (ch === ">") depth = Math.max(0, depth - 1);
    if (depth === 0 && (ch === "," || ch === ";" || ch === "\n")) {
      if (cur.trim()) out.push(cur.trim());
      cur = "";
    } else cur += ch;
  }
  if (cur.trim()) out.push(cur.trim());
  return out;
}

/** "Anna Rossi <anna@example.com>" or "Anna Rossi": the editable text. */
export function formatParticipant(p: Participant): string {
  return p.email ? `${p.name} <${p.email}>` : p.name;
}

const same = (a: string | undefined, b: string | undefined) => (a ?? "").trim().toLowerCase() === (b ?? "").trim().toLowerCase();

/** Index of the entry `p` would duplicate, or -1. The same email is the
 *  same person; without an email, the same name is. */
function duplicateOf(list: Participant[], p: Participant, skip = -1): number {
  return list.findIndex((q, i) => {
    if (i === skip) return false;
    if (p.email && q.email) return same(p.email, q.email);
    return same(p.name, q.name);
  });
}

/** Add participants, skipping duplicates. A name already listed without an
 *  email gains the email instead of being added twice. */
export function addParticipants(list: Participant[], incoming: Participant[]): Participant[] {
  const out = list.slice();
  for (const p of incoming) {
    const at = duplicateOf(out, p);
    if (at < 0) out.push(p);
    else if (p.email && !out[at].email) out[at] = { ...out[at], email: p.email };
  }
  return out;
}

/** Replace the entry at `index`; an error when the edit would duplicate
 *  another entry. */
export function updateParticipant(
  list: Participant[],
  index: number,
  p: Participant,
): { ok: true; list: Participant[] } | { ok: false; error: string } {
  const dup = duplicateOf(list, p, index);
  if (dup >= 0) return { ok: false, error: `${list[dup].name} is already listed.` };
  const out = list.slice();
  out[index] = p;
  return { ok: true, list: out };
}

export function removeParticipant(list: Participant[], index: number): Participant[] {
  return list.filter((_, i) => i !== index);
}
