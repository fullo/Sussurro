//! Fixture tests: a Google feed, an Outlook feed (Windows and custom
//! `VTIMEZONE`s) and an Apple-ish file with the odd cases, matched against
//! meetings the way the app does it.

use super::*;
use crate::archive::types::ItemType;

const GOOGLE: &str = include_str!("testdata/google.ics");
const OUTLOOK: &str = include_str!("testdata/outlook.ics");
const MISC: &str = include_str!("testdata/misc.ics");

fn offset(h: i32) -> FixedOffset {
    FixedOffset::east_opt(h * 3600).unwrap()
}

fn meeting(date: &str, duration: &str) -> ItemMeta {
    ItemMeta {
        item_type: ItemType::Meeting,
        title: "Meeting".into(),
        date: date.into(),
        duration: Some(duration.into()),
        ..Default::default()
    }
}

/// Titles of the candidates for a meeting, with whether each overlaps.
fn titles(text: &str, date: &str, duration: &str) -> Vec<(String, bool, i64)> {
    let m = meeting(date, duration);
    let w = meeting_window(&m).unwrap();
    let cal = IcsCalendar::parse(text, *w.start.offset()).unwrap();
    find(&cal, &m, &[])
        .unwrap()
        .into_iter()
        .map(|c| (c.title, c.overlaps, c.overlap_minutes))
        .collect()
}

fn t(s: &str, o: bool, m: i64) -> (String, bool, i64) {
    (s.to_string(), o, m)
}

#[test]
fn google_weekly_event_matches_the_recording() {
    // Thursday 24 Sept 2026, recorded 10:02–10:57 in Rome.
    assert_eq!(
        titles(GOOGLE, "2026-09-24T10:02:00+02:00", "00:55:00"),
        [t("Weekly sync, product", true, 55), t("Coffee", true, 12)]
    );
    // The same folded file with CRLF line ends reads the same.
    assert_eq!(
        titles(
            &GOOGLE.replace('\n', "\r\n"),
            "2026-09-24T10:02:00+02:00",
            "00:55:00"
        )
        .len(),
        2
    );
    // Winter time (CET): still 10:00 local, so a full overlap.
    assert_eq!(
        titles(GOOGLE, "2026-12-10T10:00:00+01:00", "01:00:00"),
        [t("Weekly sync, product", true, 60)]
    );
    // Recorded from another zone (London, one hour behind): same instant.
    assert_eq!(
        titles(GOOGLE, "2026-12-10T09:00:00+00:00", "01:00:00"),
        [t("Weekly sync, product", true, 60)]
    );
}

#[test]
fn exdate_and_moved_occurrences() {
    // 17 Sept is excluded: no event overlaps, and nothing else that day.
    assert!(titles(GOOGLE, "2026-09-17T10:00:00+02:00", "01:00:00").is_empty());
    // 1 Oct was moved to 14:00: at 10:00 only the moved one is offered, as
    // "same day" (not overlapping)…
    assert_eq!(
        titles(GOOGLE, "2026-10-01T10:00:00+02:00", "01:00:00"),
        [t("Weekly sync, product (moved)", false, 0)]
    );
    // …and at 14:05 it matches, with its own attendees.
    let m = meeting("2026-10-01T14:05:00+02:00", "00:50:00");
    let cal = IcsCalendar::parse(GOOGLE, offset(2)).unwrap();
    let c = find(&cal, &m, &[]).unwrap();
    assert_eq!(c.len(), 1);
    assert!(c[0].overlaps);
    let names: Vec<&str> = c[0].plan.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, ["Anna Rossi", "Sara Colombo"]);
}

#[test]
fn tolerance_around_the_recording() {
    // Starts 10 min after the event ended: within the 15 min tolerance.
    assert_eq!(
        titles(GOOGLE, "2026-12-10T11:10:00+01:00", "00:30:00"),
        [t("Weekly sync, product", true, 0)]
    );
    // 20 min after: not overlapping any more, offered as same day.
    assert_eq!(
        titles(GOOGLE, "2026-12-10T11:20:00+01:00", "00:30:00"),
        [t("Weekly sync, product", false, 0)]
    );
    // A recording without a duration is an instant.
    assert_eq!(
        titles(GOOGLE, "2026-12-10T09:50:00+01:00", ""),
        [t("Weekly sync, product", true, 0)]
    );
}

#[test]
fn google_attendees_become_a_plan() {
    let m = meeting("2026-09-24T10:02:00+02:00", "00:55:00");
    let cal = IcsCalendar::parse(GOOGLE, offset(2)).unwrap();
    let c = &find(&cal, &m, &[]).unwrap()[0];
    // Organizer merged with its own attendee line; the room is dropped;
    // an attendee without a name gets one from the address.
    let got: Vec<(&str, Option<&str>, bool, bool)> = c
        .plan
        .iter()
        .map(|p| (p.name.as_str(), p.email.as_deref(), p.organizer, p.declined))
        .collect();
    assert_eq!(
        got,
        [
            ("Anna Rossi", Some("anna@example.com"), true, false),
            (
                "Bianchi, Marco",
                Some("marco.bianchi@example.com"),
                false,
                false
            ),
            ("Lucia Verdi", Some("lucia.verdi@example.org"), false, false),
            ("Paolo Neri", Some("paolo@example.com"), false, true),
        ]
    );
    assert!(c.plan.iter().all(|p| p.action == PlanAction::Add));
    assert_eq!(c.start, "2026-09-24T10:00:00+02:00");
    assert_eq!(c.end, "2026-09-24T11:00:00+02:00");
}

#[test]
fn outlook_windows_and_custom_zones() {
    assert_eq!(
        titles(OUTLOOK, "2026-09-24T10:02:00+02:00", "00:55:00"),
        // Last Thursday of the month (Windows zone) and a US call written in
        // a custom VTIMEZONE (04:00 EDT = 10:00 in Rome). The cancelled
        // planning and the finished (COUNT=3) onboarding are left out.
        [t("Monthly review", true, 55), t("US call", true, 55)]
    );
    // August's occurrence is excluded (EXDATE with a Windows TZID).
    assert!(titles(OUTLOOK, "2026-08-27T10:00:00+02:00", "01:00:00").is_empty());
    // The onboarding's third and last week.
    assert_eq!(
        titles(OUTLOOK, "2026-09-17T10:00:00+02:00", "01:00:00"),
        [t("Onboarding", true, 60)]
    );
    // An attendee without CN is named from the address.
    let m = meeting("2026-09-24T10:02:00+02:00", "00:55:00");
    let cal = IcsCalendar::parse(OUTLOOK, offset(2)).unwrap();
    assert!(cal.notes().is_empty(), "{:?}", cal.notes());
    let c = &find(&cal, &m, &[]).unwrap()[0];
    let names: Vec<&str> = c.plan.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, ["Anna Rossi", "Giulia", "Marco Bianchi"]);
}

#[test]
fn floating_all_day_rdate_until_and_odd_cases() {
    let m = meeting("2026-09-24T10:00:00+02:00", "01:00:00");
    let cal = IcsCalendar::parse(MISC, offset(2)).unwrap();
    let c = find(&cal, &m, &[]).unwrap();
    let got: Vec<(&str, bool, bool)> = c
        .iter()
        .map(|c| (c.title.as_str(), c.overlaps, c.all_day))
        .collect();
    // Floating 10:00 in the meeting's offset; the all-day offsite (it has
    // an attendee) comes after the timed event.
    assert_eq!(
        got,
        [("Design review", true, false), ("Offsite", true, true)]
    );
    // Apple's EMAIL parameter; a name without an address; an attendee with
    // neither is dropped.
    let plan: Vec<(&str, Option<&str>)> = c[0]
        .plan
        .iter()
        .map(|p| (p.name.as_str(), p.email.as_deref()))
        .collect();
    assert_eq!(
        plan,
        [("Giulia Neri", Some("giulia@example.com")), ("Luca", None)]
    );

    // Later that day: the RDATE retro (17:00) and the unknown-zone event
    // (read at the meeting's offset), each before the all-day offsite; the
    // standup ended the day before.
    let later = titles(MISC, "2026-09-24T17:00:00+02:00", "00:20:00");
    assert_eq!(later, [t("Retro", true, 20), t("Offsite", true, 20)]);
    let noon = titles(MISC, "2026-09-24T12:00:00+02:00", "00:30:00");
    assert_eq!(noon, [t("Somewhere", true, 30), t("Offsite", true, 30)]);
    assert!(titles(MISC, "2026-09-24T09:30:00+02:00", "00:10:00")
        .iter()
        .all(|(t, _, _)| t != "Daily standup"));
    assert_eq!(
        titles(MISC, "2026-09-23T09:30:00+02:00", "00:15:00"),
        [t("Daily standup", true, 15)]
    );

    let notes = cal.notes().join("\n");
    assert!(notes.contains("“Atlantis Standard Time”"), "{notes}");
    assert!(notes.contains("1 recurring event uses a rule"), "{notes}");
    assert!(notes.contains("2 events could not be read"), "{notes}");
    assert!(notes.contains("1 unreadable line"), "{notes}");
}

#[test]
fn malformed_files_are_errors() {
    for (text, want) in [
        (
            "<!DOCTYPE html><html><body>Sign in</body></html>",
            "not calendar data",
        ),
        (&GOOGLE[..GOOGLE.len() / 2], "cut short"),
        (
            "BEGIN:VCALENDAR\nBEGIN:VEVENT\nEND:VTODO\nEND:VCALENDAR\n",
            "closes BEGIN:VEVENT",
        ),
        ("", "no VCALENDAR"),
    ] {
        let e = IcsCalendar::parse(text, offset(0))
            .err()
            .expect(want)
            .to_string();
        assert!(e.contains(want), "{e}");
    }
    // A calendar without events is fine, just empty.
    let cal =
        IcsCalendar::parse("BEGIN:VCALENDAR\nVERSION:2.0\nEND:VCALENDAR\n", offset(0)).unwrap();
    assert!(cal.is_empty());
}

#[test]
fn items_without_a_start_or_participants_are_refused() {
    let cal = IcsCalendar::parse(GOOGLE, offset(2)).unwrap();
    let mut m = meeting("yesterday", "00:10:00");
    assert!(find(&cal, &m, &[])
        .unwrap_err()
        .to_string()
        .contains("start time"));
    m.date = "2026-09-24T10:00:00+02:00".into();
    m.item_type = ItemType::Note;
    assert!(find(&cal, &m, &[]).is_err());
    assert!(apply_attendees(&mut m, &[]).is_err());
}

// ---- plan and apply ----

fn person(id: &str, name: &str, email: Option<&str>) -> Person {
    Person {
        id: id.into(),
        name: name.into(),
        email: email.map(Into::into),
        aliases: vec![],
    }
}

fn att(name: Option<&str>, email: Option<&str>) -> Attendee {
    Attendee {
        name: name.map(Into::into),
        email: email.map(Into::into),
        declined: false,
        organizer: false,
    }
}

fn part(name: &str, email: Option<&str>) -> Participant {
    Participant {
        name: name.into(),
        email: email.map(Into::into),
    }
}

#[test]
fn plan_uses_people_and_never_changes_listed_participants() {
    let people = vec![
        person("p-1", "Anna Rossi", Some("anna@example.com")),
        person("p-2", "Marco Bianchi", Some("marco@example.com")),
    ];
    let existing = vec![
        // From the Meet names, without an email (maybe removed on purpose).
        part("Marco Bianchi", None),
        part("Sara Colombo", Some("sara@work.example")),
        part("Voice 3", None),
    ];
    let attendees = vec![
        // No CN: named after the People entry with that email.
        att(None, Some("anna@example.com")),
        att(Some("Marco Bianchi"), Some("marco@example.com")),
        // Same name, a different email than the one listed: never replaced.
        att(Some("Sara Colombo"), Some("sara@home.example")),
        // A name the registry knows: the email is completed from People.
        att(Some("anna rossi"), None),
        att(Some("Nuovo Collega"), None),
        att(None, Some("x.y@example.org")),
    ];
    let plan = plan_attendees(&existing, &attendees, &people);
    let got: Vec<(&str, Option<&str>, PlanAction, bool, bool)> = plan
        .iter()
        .map(|p| {
            (
                p.name.as_str(),
                p.email.as_deref(),
                p.action,
                p.email_from_people,
                p.in_people,
            )
        })
        .collect();
    assert_eq!(
        got,
        [
            (
                "Anna Rossi",
                Some("anna@example.com"),
                PlanAction::Add,
                false,
                true
            ),
            (
                "Marco Bianchi",
                Some("marco@example.com"),
                PlanAction::CompleteEmail,
                false,
                true
            ),
            (
                "Sara Colombo",
                Some("sara@home.example"),
                PlanAction::Listed,
                false,
                false
            ),
            // "anna rossi" resolves to the same person as the first line.
            ("Nuovo Collega", None, PlanAction::Add, false, false),
            (
                "X Y",
                Some("x.y@example.org"),
                PlanAction::Add,
                false,
                false
            ),
        ]
    );
    // Without CN or email: the People email completes a new participant.
    let plan = plan_attendees(&[], &[att(Some("Marco  Bianchi"), None)], &people);
    assert_eq!(plan[0].email.as_deref(), Some("marco@example.com"));
    assert!(plan[0].email_from_people && plan[0].action == PlanAction::Add);
    // Listed by email under another name: untouched.
    let plan = plan_attendees(
        &[part("M.", Some("MARCO@example.com"))],
        &[att(Some("Marco Bianchi"), Some("marco@example.com"))],
        &people,
    );
    assert_eq!(plan[0].action, PlanAction::Listed);
}

#[test]
fn apply_adds_and_completes_only_what_was_picked() {
    let mut m = meeting("2026-09-24T10:00:00+02:00", "01:00:00");
    m.participants = vec![
        part("Marco Bianchi", None),
        part("Sara Colombo", Some("sara@work.example")),
    ];
    let plan = plan_attendees(
        &m.participants,
        &[
            att(Some("Anna Rossi"), Some("anna@example.com")),
            att(Some("Marco Bianchi"), Some("marco@example.com")),
            att(Some("Sara Colombo"), Some("sara@home.example")),
        ],
        &[],
    );

    // The user leaves "complete Marco's email" unticked (the default).
    let picked: Vec<PlannedAttendee> = plan
        .iter()
        .filter(|p| p.action == PlanAction::Add)
        .cloned()
        .collect();
    let done = apply_attendees(&mut m, &picked).unwrap();
    assert_eq!(
        done,
        Applied {
            added: 1,
            completed: 0
        }
    );
    assert_eq!(
        m.participants,
        [
            part("Marco Bianchi", None),
            part("Sara Colombo", Some("sara@work.example")),
            part("Anna Rossi", Some("anna@example.com")),
        ]
    );

    // Applying the same plan again changes nothing.
    assert_eq!(
        apply_attendees(&mut m, &picked).unwrap(),
        Applied::default()
    );

    // Ticked: Marco's email is completed; Sara's is never replaced, even
    // by a tampered request.
    let mut all = plan.clone();
    all[2].action = PlanAction::CompleteEmail;
    let done = apply_attendees(&mut m, &all).unwrap();
    assert_eq!(
        done,
        Applied {
            added: 0,
            completed: 1
        }
    );
    assert_eq!(
        m.participants[0],
        part("Marco Bianchi", Some("marco@example.com"))
    );
    assert_eq!(
        m.participants[1],
        part("Sara Colombo", Some("sara@work.example"))
    );

    // Invalid emails are refused.
    let mut bad = plan[0].clone();
    bad.name = "Someone".into();
    bad.email = Some("not an email".into());
    assert!(apply_attendees(&mut m, &[bad]).is_err());
}

#[test]
fn names_from_emails() {
    assert_eq!(name_from_email("anna.rossi@example.com"), "Anna Rossi");
    assert_eq!(name_from_email("j_doe+cal@example.com"), "J Doe Cal");
    assert_eq!(name_from_email("élodie@example.fr"), "Élodie");
}

/// Expansion stays cheap on a long-running daily event with a far meeting.
#[test]
fn long_open_ended_series_stay_fast() {
    let text = "BEGIN:VCALENDAR\nBEGIN:VEVENT\nUID:x\nSUMMARY:Daily\nDTSTART:19900101T090000Z\nDTEND:19900101T091500Z\nRRULE:FREQ=DAILY\nEND:VEVENT\nEND:VCALENDAR\n";
    let started = std::time::Instant::now();
    assert_eq!(
        titles(text, "2026-09-24T11:00:00+02:00", "00:30:00"),
        [t("Daily", true, 15)]
    );
    assert!(started.elapsed() < std::time::Duration::from_secs(2));
}

/// End to end on a temporary archive: match, then add with the People
/// registry's usual linking.
#[test]
fn match_and_add_on_an_archive_item() {
    use crate::archive::types::SegmentsFile;
    let dir = tempfile::tempdir().unwrap();
    let archive = dir.path();
    crate::archive::people::modify(archive, |people| {
        people.push(person("p-1", "Luca", Some("luca@example.com")));
        Ok(())
    })
    .unwrap();
    let mut meta = meeting("2026-09-24T10:00:00+02:00", "01:00:00");
    meta.participants = vec![part("Giulia Neri", None)];
    let id = crate::archive::create_item(archive, &meta, &SegmentsFile::default()).unwrap();

    let found = match_item(archive, &id, MISC, "misc.ics".into()).unwrap();
    assert_eq!(found.source, "misc.ics");
    assert_eq!(found.events_read, 6);
    assert_eq!(found.candidates[0].title, "Design review");
    let plan = &found.candidates[0].plan;
    // Giulia is listed without an email (maybe on purpose): only offered.
    assert_eq!(plan[0].action, PlanAction::CompleteEmail);
    // Luca has no email in the calendar: the People registry has one.
    assert_eq!(plan[1].email.as_deref(), Some("luca@example.com"));
    assert!(plan[1].email_from_people && plan[1].in_people);

    let picked: Vec<PlannedAttendee> = plan
        .iter()
        .filter(|p| p.action == PlanAction::Add)
        .cloned()
        .collect();
    let done = add_to_item(archive, &id, &picked).unwrap();
    assert_eq!((done.applied.added, done.applied.completed), (1, 0));
    let saved = crate::archive::read_item(archive, &id)
        .unwrap()
        .meta
        .participants;
    assert_eq!(
        saved,
        [
            part("Giulia Neri", None),
            part("Luca", Some("luca@example.com"))
        ]
    );
    // Nothing picked: nothing written.
    let again = add_to_item(archive, &id, &[]).unwrap();
    assert_eq!(again.applied, Applied::default());

    // A file that isn't a calendar.
    let e = match_item(archive, &id, "<html>", "x.ics".into())
        .unwrap_err()
        .to_string();
    assert!(e.contains("not a calendar file"), "{e}");
}
