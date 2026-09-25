/* Meeting attendees from a calendar (#252, P22): the shapes the backend's
 * `calendar_*` commands return (src-tauri/src/calendar/) and the pure
 * helpers of the "Add attendees from calendar…" panel.
 *
 * The backend plans each attendee: "add" (a new participant), "complete_email"
 * (a participant already listed by name without an email — offered, never
 * ticked by default: the user may have removed that email on purpose) or
 * "listed" (nothing to do; an existing email is never replaced). */

import type { Item } from "./types";

export type PlanAction = "add" | "complete_email" | "listed";

export interface CalendarAttendee {
  name: string | null;
  email: string | null;
  declined: boolean;
  organizer: boolean;
}

export interface PlannedAttendee {
  name: string;
  email: string | null;
  action: PlanAction;
  /** The email comes from People, not from the calendar. */
  email_from_people: boolean;
  in_people: boolean;
  declined: boolean;
  organizer: boolean;
}

export interface CalendarCandidate {
  uid: string;
  title: string;
  /** RFC 3339 in the meeting's own UTC offset. */
  start: string;
  end: string;
  all_day: boolean;
  /** Overlaps the recording (15 min tolerance). */
  overlaps: boolean;
  overlap_minutes: number;
  attendees: CalendarAttendee[];
  plan: PlannedAttendee[];
}

export interface CalendarMatch {
  /** File name, or the link's host. */
  source: string;
  events_read: number;
  candidates: CalendarCandidate[];
  notes: string[];
}

export interface CalendarLinkStatus {
  saved: boolean;
  host: string | null;
  store_available: boolean;
  store_name: string;
  error: string;
}

export interface AttendeesAdded {
  item: Item;
  added: number;
  completed: number;
}

/** Whether an attendee has anything to apply. */
export function pickable(p: PlannedAttendee): boolean {
  return p.action !== "listed";
}

/** Ticked at first: new, non-declined attendees. Completing an email is
 *  always the user's explicit choice. */
export function defaultPicks(plan: PlannedAttendee[]): boolean[] {
  return plan.map((p) => p.action === "add" && !p.declined);
}

export function pickedAttendees(plan: PlannedAttendee[], picks: boolean[]): PlannedAttendee[] {
  return plan.filter((p, i) => pickable(p) && picks[i]);
}

/** "10:00" from an RFC 3339 string, on the meeting's own clock. */
export function clock(rfc3339: string): string {
  const m = /T(\d{2}):(\d{2})/.exec(rfc3339);
  return m ? `${m[1]}:${m[2]}` : "";
}

/** "10:00–11:00", or "all day". */
export function timeRange(c: Pick<CalendarCandidate, "start" | "end" | "all_day">): string {
  if (c.all_day) return "all day";
  const a = clock(c.start);
  const b = clock(c.end);
  return a === b ? a : `${a}–${b}`;
}

/** How an event relates to the recording, in words. */
export function overlapHint(c: Pick<CalendarCandidate, "overlaps" | "overlap_minutes">): string {
  if (!c.overlaps) return "same day, not during the recording";
  if (c.overlap_minutes > 0) return `overlaps the recording by ${c.overlap_minutes} min`;
  return "just before or after the recording";
}

const plural = (n: number, word: string) => `${n} ${word}${n === 1 ? "" : "s"}`;

/** What one attendee line says next to its name. */
export function actionLabel(p: PlannedAttendee): string {
  const bits: string[] = [];
  if (p.action === "listed") bits.push("already listed");
  else if (p.action === "complete_email") bits.push(`listed without an email — add ${p.email}`);
  else if (p.email) bits.push(p.email_from_people ? `${p.email} (from People)` : p.email);
  else bits.push("no email in the calendar");
  if (p.organizer) bits.push("organizer");
  if (p.declined) bits.push("declined");
  return bits.join(" · ");
}

/** Why the list is empty, or what the chosen event lacks. */
export function matchMessage(m: CalendarMatch): string {
  if (m.events_read === 0) return `${m.source} has no events.`;
  if (!m.candidates.length) return "No event in the calendar on this meeting's day.";
  if (!m.candidates.some((c) => c.overlaps))
    return "No event overlaps the recording. These are the other events that day: pick one if it is this meeting.";
  return "";
}

/** A candidate whose event lists nobody (free/busy feeds, private events). */
export function noAttendeesMessage(c: CalendarCandidate): string {
  return c.attendees.length
    ? ""
    : "This event has no attendee list — the calendar may share only free/busy details or hide guests.";
}

/** The flash message after adding. */
export function addedMessage(added: number, completed: number): string {
  if (!added && !completed) return "Nothing to add: everyone is already listed.";
  const parts: string[] = [];
  if (added) parts.push(`${plural(added, "participant")} added`);
  if (completed) parts.push(`${plural(completed, "email")} completed`);
  const s = parts.join(", ");
  return `${s.charAt(0).toUpperCase()}${s.slice(1)} from the calendar.`;
}

/** Light check before sending a link to the backend (which validates it
 *  fully): something that looks like an http(s) or webcal address. */
export function linkInputError(input: string): string | null {
  const s = input.trim();
  if (!s) return null;
  if (!/^(https?|webcals?):\/\/\S+$/i.test(s)) return "Paste the full address, starting with https:// or webcal://";
  return null;
}
