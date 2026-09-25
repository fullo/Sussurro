//! Meeting attendees from a calendar (0.11, #252; plan P22, E21, §4.9).
//!
//! The user imports an `.ics` file or uses a private ICS link saved in
//! Settings → Calendar ([`link`]); no account, no OAuth (those come in
//! Track A as further [`CalendarSource`]s). For a meeting item:
//!
//! 1. the calendar's events around the recording are listed ([`find`]):
//!    those overlapping the recording (with [`MATCH_TOLERANCE_MIN`] of
//!    slack on both sides) first, by overlap; when none overlaps, the other
//!    events of that day, so the user can still pick one;
//! 2. for the chosen event each attendee gets a planned action
//!    ([`plan_attendees`]): *add* as a participant, *complete the email*
//!    of a participant already listed without one (opt-in in the UI: the
//!    user may have removed it on purpose), or *already listed*;
//! 3. the picked actions are applied ([`apply_attendees`]); the existing
//!    People linking then fills emails the calendar didn't have. People
//!    entries are only suggested (the chips' "+ People"), never created.
//!
//! **What an ICS feed carries** depends on the provider and on the detail
//! level the user shares: a "free/busy" feed has no titles or attendees,
//! and the UI says what it found.
//!
//! Parsing is pure and tested on fixtures ([`ics`], [`time`], [`rrule`]):
//! folded lines, UTC / `TZID` / floating times, `VTIMEZONE`s, all-day
//! events, `RRULE` with `EXDATE`, `RDATE` and moved or cancelled
//! occurrences (`RECURRENCE-ID`), expanded only around the day asked.

pub mod ics;
pub mod link;
pub mod rrule;
pub mod time;

use crate::archive::people::{is_valid_email, match_person, name_key, PeopleMatcher, Person};
use crate::archive::types::{normalize_participants, ItemMeta, Participant};
use anyhow::{anyhow, bail, Result};
use chrono::{DateTime, Duration, FixedOffset, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use time::{DateValue, ZoneRef, Zones};

/// Slack on both sides of the recording when matching events: a meeting
/// recorded from a few minutes early, or started late, still matches.
pub const MATCH_TOLERANCE_MIN: i64 = 15;
/// Largest calendar file or link body read (a big, years-long calendar is
/// a few MB).
pub const MAX_ICS_BYTES: u64 = 20 * 1024 * 1024;
/// Events offered at most.
pub const MAX_CANDIDATES: usize = 20;
/// Events read from one file at most.
pub const MAX_EVENTS: usize = 200_000;

/// Someone invited to an event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attendee {
    /// `CN`, when it is a name (not a copy of the email).
    pub name: Option<String>,
    pub email: Option<String>,
    /// Answered "no" to the invitation.
    #[serde(default)]
    pub declined: bool,
    /// The event's organizer.
    #[serde(default)]
    pub organizer: bool,
}

/// One occurrence of an event.
#[derive(Debug, Clone, PartialEq)]
pub struct Event {
    pub uid: String,
    pub title: String,
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
    pub all_day: bool,
    pub attendees: Vec<Attendee>,
}

/// A source of calendar events (E21): an ICS file or link today, Google
/// or Microsoft through OAuth later (Track A).
pub trait CalendarSource {
    /// The event occurrences that overlap `[from, to]`.
    fn events_between(&self, from: DateTime<Utc>, to: DateTime<Utc>) -> Vec<Event>;
    /// What the reader noticed (unknown time zones, rules it can't expand,
    /// unreadable lines), in words for the UI.
    fn notes(&self) -> Vec<String>;
}

// ---- ICS -------------------------------------------------------------------

/// A `VEVENT` as read, before expansion.
#[derive(Debug, Clone)]
struct RawEvent {
    uid: String,
    title: String,
    start: DateValue,
    /// Length of each occurrence.
    length: Duration,
    rrule: Option<rrule::RRule>,
    rdates: Vec<DateValue>,
    exdates: Vec<DateValue>,
    /// Set on an occurrence that replaces one of a recurring event.
    recurrence_id: Option<DateValue>,
    cancelled: bool,
    attendees: Vec<Attendee>,
}

/// A parsed `.ics` file: a [`CalendarSource`].
pub struct IcsCalendar {
    events: Vec<RawEvent>,
    zones: Zones,
    /// `(uid, original start)` of occurrences replaced or cancelled by a
    /// `RECURRENCE-ID` entry.
    overridden: HashSet<(String, DateTime<Utc>)>,
    skipped_events: usize,
    skipped_lines: usize,
    unsupported_rules: usize,
}

/// A mailto address (`mailto:` is case-insensitive), or the `EMAIL`
/// parameter Apple writes next to a `urn:uuid:` value.
fn attendee_email(p: &ics::Property) -> Option<String> {
    let from_value = p
        .value
        .trim()
        .get(..7)
        .filter(|s| s.eq_ignore_ascii_case("mailto:"))
        .map(|_| p.value.trim()[7..].trim().to_string());
    from_value
        .into_iter()
        .chain(p.param("EMAIL").map(|e| e.trim().to_string()))
        .find(|e| is_valid_email(e))
}

fn attendee(p: &ics::Property, organizer: bool) -> Option<Attendee> {
    // Rooms and equipment are not people.
    if let Some(cu) = p.param("CUTYPE") {
        if cu.eq_ignore_ascii_case("ROOM") || cu.eq_ignore_ascii_case("RESOURCE") {
            return None;
        }
    }
    let email = attendee_email(p);
    let name = p
        .param("CN")
        .map(|n| n.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|n| !n.is_empty())
        // Google writes the address as the CN when it knows no name.
        .filter(|n| {
            !email.as_deref().is_some_and(|e| e.eq_ignore_ascii_case(n)) && !is_valid_email(n)
        });
    if name.is_none() && email.is_none() {
        return None;
    }
    let declined = p
        .param("PARTSTAT")
        .is_some_and(|s| s.eq_ignore_ascii_case("DECLINED"));
    Some(Attendee {
        name,
        email,
        declined,
        organizer,
    })
}

/// Organizer first, then attendees; one entry per email (or name).
fn attendees_of(ev: &ics::Component) -> Vec<Attendee> {
    let mut out: Vec<Attendee> = Vec::new();
    let all = ev
        .props_named("ORGANIZER")
        .filter_map(|p| attendee(p, true))
        .chain(
            ev.props_named("ATTENDEE")
                .filter_map(|p| attendee(p, false)),
        );
    for a in all {
        let dup = out.iter_mut().find(|b| match (&a.email, &b.email) {
            (Some(x), Some(y)) => x.eq_ignore_ascii_case(y),
            (None, None) => a.name.as_deref().map(name_key) == b.name.as_deref().map(name_key),
            _ => false,
        });
        match dup {
            Some(b) => {
                // The organizer is usually also an attendee: merge the two.
                b.name = b.name.take().or(a.name);
                b.declined = a.declined;
            }
            None => out.push(a),
        }
    }
    out
}

fn raw_event(c: &ics::Component, zones: &Zones, index: usize) -> Result<RawEvent> {
    let start = time::prop_value(c.prop("DTSTART").ok_or_else(|| anyhow!("no DTSTART"))?)?;
    let start_at = zones.value_instant(&start);
    let length = if let Some(end) = c.prop("DTEND") {
        zones.value_instant(&time::prop_value(end)?) - start_at
    } else if let Some(d) = c.prop("DURATION") {
        time::parse_duration(&d.value)?
    } else if start.is_date() {
        Duration::days(1)
    } else {
        Duration::zero()
    };
    let uid = c
        .prop("UID")
        .map(|p| p.value.trim().to_string())
        .filter(|u| !u.is_empty())
        .unwrap_or_else(|| format!("event-{index}"));
    let title = c
        .prop("SUMMARY")
        .map(|p| ics::unescape(&p.value).trim().to_string())
        .filter(|t| !t.is_empty())
        .unwrap_or_default();
    let list = |name: &str| -> Vec<DateValue> {
        c.props_named(name).flat_map(time::prop_values).collect()
    };
    Ok(RawEvent {
        uid,
        title,
        start,
        length: length.max(Duration::zero()),
        rrule: None,
        rdates: list("RDATE"),
        exdates: list("EXDATE"),
        recurrence_id: c.prop("RECURRENCE-ID").map(time::prop_value).transpose()?,
        cancelled: c
            .prop("STATUS")
            .is_some_and(|s| s.value.trim().eq_ignore_ascii_case("CANCELLED")),
        attendees: attendees_of(c),
    })
}

impl IcsCalendar {
    /// Parse `text`; floating times are read at `floating` (the meeting's
    /// UTC offset). Errors only when the file isn't a calendar at all;
    /// unreadable events are skipped and counted in [`Self::notes`].
    pub fn parse(text: &str, floating: FixedOffset) -> Result<IcsCalendar> {
        let parsed = ics::parse(text)?;
        let zones = Zones::new(&parsed.calendars, floating);
        let mut events = Vec::new();
        let mut skipped_events = 0;
        let mut unsupported_rules = 0;
        for c in parsed
            .calendars
            .iter()
            .flat_map(|cal| cal.children.iter())
            .filter(|c| c.name == "VEVENT")
        {
            if events.len() >= MAX_EVENTS {
                bail!("the calendar has more than {MAX_EVENTS} events");
            }
            match raw_event(c, &zones, events.len()) {
                Ok(mut ev) => {
                    if let Some(r) = c.prop("RRULE") {
                        match rrule::RRule::parse(&r.value) {
                            Ok(rule) => ev.rrule = Some(rule),
                            Err(_) => unsupported_rules += 1,
                        }
                    }
                    events.push(ev);
                }
                Err(_) => skipped_events += 1,
            }
        }
        let overridden = events
            .iter()
            .filter_map(|e| {
                e.recurrence_id
                    .as_ref()
                    .map(|r| (e.uid.clone(), zones.value_instant(r)))
            })
            .collect();
        Ok(IcsCalendar {
            events,
            zones,
            overridden,
            skipped_events,
            skipped_lines: parsed.skipped_lines,
            unsupported_rules,
        })
    }

    /// Events read (a recurring event counts once).
    pub fn len(&self) -> usize {
        self.events.len()
    }

    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }

    /// An occurrence removed by `EXDATE`, or replaced by a `RECURRENCE-ID`
    /// entry (which is itself never excluded that way).
    fn excluded(&self, ev: &RawEvent, start: DateTime<Utc>, local_date: NaiveDate) -> bool {
        (ev.recurrence_id.is_none() && self.overridden.contains(&(ev.uid.clone(), start)))
            || ev.exdates.iter().any(|x| match x {
                DateValue::Date(d) => *d == local_date,
                other => self.zones.value_instant(other) == start,
            })
    }

    /// Occurrence starts of `ev` up to `to` (instants), in order.
    fn starts(
        &self,
        ev: &RawEvent,
        from: DateTime<Utc>,
        to: DateTime<Utc>,
    ) -> Vec<(DateTime<Utc>, NaiveDate)> {
        let mut out = Vec::new();
        let (local, zone) = match &ev.start {
            DateValue::Date(d) => (
                d.and_hms_opt(0, 0, 0).unwrap_or_default(),
                ZoneRef::Floating,
            ),
            DateValue::DateTime(t, z) => (*t, z.clone()),
        };
        let earliest = from - ev.length;
        let mut push = |t: DateTime<Utc>, d: NaiveDate| {
            if t >= earliest && t <= to && !self.excluded(ev, t, d) {
                out.push((t, d));
            }
        };
        match (&ev.rrule, &ev.recurrence_id) {
            (Some(rule), None) => {
                let until = rule.until.map(|u| match u {
                    rrule::Until::Date(d) => d.and_hms_opt(23, 59, 59).unwrap_or_default(),
                    rrule::Until::Local(t) => t,
                    rrule::Until::Utc(t) => self.zones.local(t.and_utc(), &zone),
                });
                let horizon = self.zones.local(to, &zone).date() + Duration::days(1);
                rule.expand(local, until, horizon, &mut |t| {
                    let at = self.zones.instant(t, &zone);
                    if at > to {
                        return false;
                    }
                    push(at, t.date());
                    true
                });
            }
            _ => push(self.zones.instant(local, &zone), local.date()),
        }
        if ev.recurrence_id.is_none() {
            for r in &ev.rdates {
                let d = match r {
                    DateValue::Date(d) => *d,
                    DateValue::DateTime(t, _) => t.date(),
                };
                push(self.zones.value_instant(r), d);
            }
        }
        out.sort();
        out.dedup();
        out
    }
}

impl CalendarSource for IcsCalendar {
    fn events_between(&self, from: DateTime<Utc>, to: DateTime<Utc>) -> Vec<Event> {
        let mut out = Vec::new();
        for ev in self.events.iter().filter(|e| !e.cancelled) {
            for (start, _) in self.starts(ev, from, to) {
                let end = start + ev.length;
                if start <= to && end >= from {
                    out.push(Event {
                        uid: ev.uid.clone(),
                        title: ev.title.clone(),
                        start,
                        end,
                        all_day: ev.start.is_date(),
                        attendees: ev.attendees.clone(),
                    });
                }
            }
        }
        out.sort_by(|a, b| a.start.cmp(&b.start).then_with(|| a.title.cmp(&b.title)));
        out
    }

    fn notes(&self) -> Vec<String> {
        let mut notes = Vec::new();
        let unknown = self.zones.unknown();
        if !unknown.is_empty() {
            notes.push(format!(
                "Unknown time zone {}: its times were read as the meeting's local time.",
                unknown
                    .iter()
                    .map(|z| format!("“{z}”"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        if self.unsupported_rules > 0 {
            notes.push(format!(
                "{} recurring event{} use{} a rule Sussurro can't expand: only their first date is used.",
                self.unsupported_rules,
                plural(self.unsupported_rules),
                if self.unsupported_rules == 1 { "s" } else { "" },
            ));
        }
        if self.skipped_events > 0 {
            notes.push(format!(
                "{} event{} could not be read (no or bad start time).",
                self.skipped_events,
                plural(self.skipped_events)
            ));
        }
        if self.skipped_lines > 0 {
            notes.push(format!(
                "{} unreadable line{} skipped.",
                self.skipped_lines,
                plural(self.skipped_lines)
            ));
        }
        notes
    }
}

fn plural(n: usize) -> &'static str {
    if n == 1 {
        ""
    } else {
        "s"
    }
}

// ---- Matching ----------------------------------------------------------------

/// When the meeting was recorded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MeetingWindow {
    pub start: DateTime<FixedOffset>,
    pub end: DateTime<FixedOffset>,
}

/// `HH:MM:SS` (or `MM:SS`) as written in the frontmatter.
fn parse_hms(s: &str) -> Option<Duration> {
    let parts: Vec<i64> = s
        .trim()
        .split(':')
        .map(|p| p.trim().parse::<i64>().ok().filter(|n| *n >= 0))
        .collect::<Option<_>>()?;
    let secs = match parts.as_slice() {
        [h, m, s] => h * 3600 + m * 60 + s,
        [m, s] => m * 60 + s,
        _ => return None,
    };
    Some(Duration::seconds(secs))
}

/// The recording's start (the item's `date`) and end (plus `duration`).
pub fn meeting_window(meta: &ItemMeta) -> Result<MeetingWindow> {
    let start = DateTime::parse_from_rfc3339(meta.date.trim())
        .map_err(|_| anyhow!("this item has no valid start time, so no event can be matched"))?;
    let length = meta
        .duration
        .as_deref()
        .and_then(parse_hms)
        .unwrap_or_else(Duration::zero);
    Ok(MeetingWindow {
        start,
        end: start + length,
    })
}

/// An event offered for a meeting.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Candidate {
    pub uid: String,
    pub title: String,
    /// RFC 3339 in the meeting's UTC offset.
    pub start: String,
    pub end: String,
    pub all_day: bool,
    /// Overlaps the recording (within the tolerance).
    pub overlaps: bool,
    /// Minutes of actual overlap with the recording.
    pub overlap_minutes: i64,
    pub attendees: Vec<Attendee>,
    /// What adding its attendees would do ([`plan_attendees`]).
    pub plan: Vec<PlannedAttendee>,
}

fn overlap(a: (DateTime<Utc>, DateTime<Utc>), b: (DateTime<Utc>, DateTime<Utc>)) -> Duration {
    (a.1.min(b.1) - a.0.max(b.0)).max(Duration::zero())
}

/// Events that fit the meeting, best first: the ones overlapping the
/// recording ± `tolerance` (timed before all-day, then by overlap, then by
/// how close they start); when none does, the day's other events by how
/// close they start. All-day events without attendees are left out (a
/// holiday is not a meeting).
pub fn match_events(
    source: &dyn CalendarSource,
    w: &MeetingWindow,
    tolerance: Duration,
) -> Vec<(Event, bool, Duration)> {
    let offset = *w.start.offset();
    let rec = (w.start.with_timezone(&Utc), w.end.with_timezone(&Utc));
    let day_start = w
        .start
        .date_naive()
        .and_hms_opt(0, 0, 0)
        .and_then(|t| t.and_local_timezone(offset).single())
        .map_or(rec.0, |t| t.with_timezone(&Utc));
    let day_end = (w
        .end
        .date_naive()
        .succ_opt()
        .and_then(|d| d.and_hms_opt(0, 0, 0))
        .and_then(|t| t.and_local_timezone(offset).single())
        .map_or(rec.1, |t| t.with_timezone(&Utc)))
    .max(rec.1 + tolerance);
    let from = day_start.min(rec.0 - tolerance);
    let events: Vec<Event> = source
        .events_between(from, day_end)
        .into_iter()
        .filter(|e| !(e.all_day && e.attendees.is_empty()))
        .collect();
    let widened = (rec.0 - tolerance, rec.1 + tolerance);
    let fits = |e: &Event| {
        if e.end == e.start {
            e.start >= widened.0 && e.start <= widened.1
        } else {
            e.start < widened.1 && e.end > widened.0
        }
    };
    let distance = |e: &Event| (e.start - rec.0).num_seconds().abs();
    let mut hits: Vec<(Event, bool, Duration)> = events
        .iter()
        .filter(|e| fits(e))
        .map(|e| (e.clone(), true, overlap((e.start, e.end), rec)))
        .collect();
    if hits.is_empty() {
        hits = events
            .into_iter()
            .map(|e| {
                let o = overlap((e.start, e.end), rec);
                (e, false, o)
            })
            .collect();
    }
    hits.sort_by(|(a, _, oa), (b, _, ob)| {
        a.all_day
            .cmp(&b.all_day)
            .then(ob.cmp(oa))
            .then(distance(a).cmp(&distance(b)))
    });
    hits.truncate(MAX_CANDIDATES);
    hits
}

/// [`match_events`] with each event's attendee plan, for the UI.
pub fn find(
    source: &dyn CalendarSource,
    meta: &ItemMeta,
    people: &[Person],
) -> Result<Vec<Candidate>> {
    if !meta.item_type.has_participants() {
        bail!("notes have no participants");
    }
    let w = meeting_window(meta)?;
    let offset = *w.start.offset();
    Ok(
        match_events(source, &w, Duration::minutes(MATCH_TOLERANCE_MIN))
            .into_iter()
            .map(|(e, overlaps, o)| Candidate {
                plan: plan_attendees(&meta.participants, &e.attendees, people),
                uid: e.uid,
                title: e.title,
                start: e.start.with_timezone(&offset).to_rfc3339(),
                end: e.end.with_timezone(&offset).to_rfc3339(),
                all_day: e.all_day,
                overlaps,
                overlap_minutes: o.num_minutes(),
                attendees: e.attendees,
            })
            .collect(),
    )
}

// ---- Attendees → participants ------------------------------------------------

/// What adding an attendee does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanAction {
    /// A new participant.
    Add,
    /// A participant already listed by name, without an email: the email
    /// can be added (never by default — it may have been removed).
    CompleteEmail,
    /// Already listed (same email, or same name with an email): untouched.
    Listed,
}

/// One attendee and what adding it would do.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlannedAttendee {
    pub name: String,
    pub email: Option<String>,
    pub action: PlanAction,
    /// The email comes from the People registry, not from the calendar.
    #[serde(default)]
    pub email_from_people: bool,
    /// Already someone in the People registry.
    #[serde(default)]
    pub in_people: bool,
    #[serde(default)]
    pub declined: bool,
    #[serde(default)]
    pub organizer: bool,
}

/// "anna.rossi@x" → "Anna Rossi" (mirrors `nameFromEmail` in
/// `src/lib/participants.ts`).
pub fn name_from_email(email: &str) -> String {
    let local = email.trim().split('@').next().unwrap_or_default();
    local
        .split(['.', '_', '+', '-'])
        .filter(|w| !w.is_empty())
        .map(|w| {
            let mut c = w.chars();
            c.next()
                .map(|f| f.to_uppercase().collect::<String>() + c.as_str())
                .unwrap_or_default()
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Plan the attendees of an event against the item's `existing`
/// participants. A name comes from `CN`, else from the People entry with
/// that email, else from the email; an email from the calendar, else from
/// the People entry the name matches. Existing participants are never
/// changed by the plan itself.
pub fn plan_attendees(
    existing: &[Participant],
    attendees: &[Attendee],
    people: &[Person],
) -> Vec<PlannedAttendee> {
    let matcher = PeopleMatcher::new(people);
    let by_id: HashMap<&str, &Person> = people.iter().map(|p| (p.id.as_str(), p)).collect();
    let mut out: Vec<PlannedAttendee> = Vec::new();
    for a in attendees {
        let person_by_email = a
            .email
            .as_deref()
            .and_then(|e| matcher.person_for("", e))
            .and_then(|id| by_id.get(id).copied());
        let name = a
            .name
            .clone()
            .or_else(|| person_by_email.map(|p| p.name.clone()))
            .or_else(|| a.email.as_deref().map(name_from_email))
            .filter(|n| !n.trim().is_empty());
        let Some(name) = name else { continue };
        let (email, email_from_people) = match &a.email {
            Some(e) => (Some(e.clone()), false),
            None => match match_person(people, &name).and_then(|p| p.email.clone()) {
                Some(e) => (Some(e), true),
                None => (None, false),
            },
        };
        let same_email = |p: &Participant| matches!((&p.email, &email), (Some(x), Some(y)) if x.trim().eq_ignore_ascii_case(y.trim()));
        let key = name_key(&name);
        let action = if existing.iter().any(same_email) {
            PlanAction::Listed
        } else if let Some(p) = existing.iter().find(|p| name_key(&p.name) == key) {
            if p.email.as_deref().is_some_and(|e| !e.trim().is_empty()) || email.is_none() {
                PlanAction::Listed
            } else {
                PlanAction::CompleteEmail
            }
        } else {
            PlanAction::Add
        };
        let planned = PlannedAttendee {
            in_people: matcher
                .person_for(&name, email.as_deref().unwrap_or(""))
                .is_some(),
            name,
            email,
            action,
            email_from_people,
            declined: a.declined,
            organizer: a.organizer,
        };
        // Two attendees resolving to the same person are listed once.
        let dup = out.iter().any(|p| match (&p.email, &planned.email) {
            (Some(x), Some(y)) => x.eq_ignore_ascii_case(y),
            _ => name_key(&p.name) == name_key(&planned.name),
        });
        if !dup {
            out.push(planned);
        }
    }
    out
}

/// What [`apply_attendees`] changed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct Applied {
    pub added: usize,
    pub completed: usize,
}

/// Apply the attendees the user picked to `meta`'s participants, checked
/// again against the current list (it may have changed since the plan):
/// `Add` appends a participant unless one with the same email, or the same
/// name, is listed; `CompleteEmail` sets the email of the participant with
/// that name only if it still has none. Nothing else is touched: names
/// and existing emails are never replaced.
pub fn apply_attendees(meta: &mut ItemMeta, picked: &[PlannedAttendee]) -> Result<Applied> {
    if !meta.item_type.has_participants() {
        bail!("notes have no participants");
    }
    let mut applied = Applied::default();
    for p in picked {
        let name = p.name.split_whitespace().collect::<Vec<_>>().join(" ");
        let email = p
            .email
            .as_deref()
            .map(str::trim)
            .filter(|e| !e.is_empty())
            .map(str::to_string);
        if name.is_empty() {
            continue;
        }
        if let Some(e) = &email {
            if !is_valid_email(e) {
                bail!("“{e}” doesn't look like an email");
            }
            if meta.participants.iter().any(|q| {
                q.email
                    .as_deref()
                    .is_some_and(|x| x.trim().eq_ignore_ascii_case(e))
            }) {
                continue;
            }
        }
        let key = name_key(&name);
        let listed = meta
            .participants
            .iter_mut()
            .find(|q| name_key(&q.name) == key);
        match (p.action, listed) {
            (PlanAction::Add, None) => {
                meta.participants.push(Participant { name, email });
                applied.added += 1;
            }
            (PlanAction::CompleteEmail, Some(q))
                if email.is_some() && q.email.as_deref().is_none_or(|x| x.trim().is_empty()) =>
            {
                q.email = email;
                applied.completed += 1;
            }
            _ => {}
        }
    }
    meta.participants = normalize_participants(&meta.participants);
    Ok(applied)
}

#[cfg(test)]
mod tests;
