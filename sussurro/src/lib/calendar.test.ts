import { describe, expect, it } from "vitest";
import {
  actionLabel,
  addedMessage,
  clock,
  defaultPicks,
  linkInputError,
  matchMessage,
  noAttendeesMessage,
  overlapHint,
  pickedAttendees,
  timeRange,
  type CalendarCandidate,
  type PlannedAttendee,
} from "./calendar";

const planned = (over: Partial<PlannedAttendee>): PlannedAttendee => ({
  name: "Anna Rossi",
  email: "anna@example.com",
  action: "add",
  email_from_people: false,
  in_people: false,
  declined: false,
  organizer: false,
  ...over,
});

const candidate = (over: Partial<CalendarCandidate> = {}): CalendarCandidate => ({
  uid: "u",
  title: "Weekly sync",
  start: "2026-09-24T10:00:00+02:00",
  end: "2026-09-24T11:00:00+02:00",
  all_day: false,
  overlaps: true,
  overlap_minutes: 55,
  attendees: [{ name: "Anna Rossi", email: "anna@example.com", declined: false, organizer: true }],
  plan: [],
  ...over,
});

describe("picks", () => {
  it("ticks new attendees only, never an email completion or a decline", () => {
    const plan = [
      planned({}),
      planned({ name: "Marco", action: "complete_email" }),
      planned({ name: "Sara", action: "listed" }),
      planned({ name: "Paolo", declined: true }),
    ];
    expect(defaultPicks(plan)).toEqual([true, false, false, false]);
    // A listed attendee is never sent, even if ticked.
    expect(pickedAttendees(plan, [true, true, true, false]).map((p) => p.name)).toEqual(["Anna Rossi", "Marco"]);
  });
});

describe("labels", () => {
  it("shows times on the meeting's own clock", () => {
    expect(clock("2026-09-24T10:05:00+02:00")).toBe("10:05");
    expect(clock("nonsense")).toBe("");
    expect(timeRange(candidate())).toBe("10:00–11:00");
    expect(timeRange(candidate({ all_day: true }))).toBe("all day");
    expect(timeRange(candidate({ end: "2026-09-24T10:00:00+02:00" }))).toBe("10:00");
  });

  it("explains the overlap", () => {
    expect(overlapHint(candidate())).toBe("overlaps the recording by 55 min");
    expect(overlapHint(candidate({ overlap_minutes: 0 }))).toMatch(/just before or after/);
    expect(overlapHint(candidate({ overlaps: false, overlap_minutes: 0 }))).toMatch(/same day/);
  });

  it("describes each attendee's action", () => {
    expect(actionLabel(planned({ organizer: true }))).toBe("anna@example.com · organizer");
    expect(actionLabel(planned({ email_from_people: true }))).toBe("anna@example.com (from People)");
    expect(actionLabel(planned({ email: null }))).toBe("no email in the calendar");
    expect(actionLabel(planned({ action: "complete_email" }))).toMatch(/without an email — add anna@example.com/);
    expect(actionLabel(planned({ action: "listed", declined: true }))).toBe("already listed · declined");
  });

  it("says why nothing is offered", () => {
    const m = { source: "team.ics", events_read: 0, candidates: [], notes: [] };
    expect(matchMessage(m)).toBe("team.ics has no events.");
    expect(matchMessage({ ...m, events_read: 4 })).toMatch(/No event in the calendar/);
    expect(matchMessage({ ...m, events_read: 4, candidates: [candidate({ overlaps: false })] })).toMatch(/No event overlaps/);
    expect(matchMessage({ ...m, events_read: 4, candidates: [candidate()] })).toBe("");
    expect(noAttendeesMessage(candidate({ attendees: [] }))).toMatch(/free\/busy/);
    expect(noAttendeesMessage(candidate())).toBe("");
  });

  it("summarises what was added", () => {
    expect(addedMessage(0, 0)).toMatch(/Nothing to add/);
    expect(addedMessage(1, 0)).toBe("1 participant added from the calendar.");
    expect(addedMessage(3, 2)).toBe("3 participants added, 2 emails completed from the calendar.");
    expect(addedMessage(0, 1)).toBe("1 email completed from the calendar.");
  });
});

describe("linkInputError", () => {
  it("accepts https and webcal addresses", () => {
    expect(linkInputError("")).toBeNull();
    expect(linkInputError(" https://calendar.google.com/calendar/ical/x/private-y/basic.ics ")).toBeNull();
    expect(linkInputError("webcal://p01-caldav.icloud.com/published/2/abc")).toBeNull();
    expect(linkInputError("calendar.google.com/x")).toMatch(/https:\/\//);
    expect(linkInputError("ftp://example.com/cal.ics")).toMatch(/https:\/\//);
  });
});
