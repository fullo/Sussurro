//! Date and time values of a calendar file and their time zones.
//!
//! A `DATE-TIME` is UTC (`…Z`), in a named zone (`TZID=…`) or floating
//! (neither). A named zone is resolved, in order:
//!
//! 1. as an IANA name (`Europe/Rome`, also behind an old
//!    `/mozilla.org/…/Europe/Rome` prefix), through `chrono-tz`;
//! 2. from the file's own `VTIMEZONE` with that `TZID` (Outlook writes
//!    Windows names such as `W. Europe Standard Time` and defines them
//!    there): its `STANDARD` / `DAYLIGHT` observances and their yearly rules;
//! 3. through a short table of common Windows names;
//! 4. otherwise like a floating time, and the zone is reported as unknown.
//!
//! Floating times (and all-day dates) are read in the meeting's own UTC
//! offset — the one written in the item's `date` — which is what the
//! person who recorded the meeting saw on their clock.

use super::ics::{Component, Property};
use super::rrule::{RRule, Until};
use anyhow::{anyhow, bail, Result};
use chrono::{
    DateTime, Duration, FixedOffset, LocalResult, NaiveDate, NaiveDateTime, Offset, TimeZone, Utc,
};
use std::cell::RefCell;
use std::collections::{BTreeSet, HashMap};

/// Where a date-time's wall-clock time is measured.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ZoneRef {
    Utc,
    Floating,
    Named(String),
}

/// A `DATE` or `DATE-TIME` value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DateValue {
    Date(NaiveDate),
    DateTime(NaiveDateTime, ZoneRef),
}

impl DateValue {
    pub fn is_date(&self) -> bool {
        matches!(self, DateValue::Date(_))
    }
}

/// Parse one value (`20260925`, `20260925T100000`, `20260925T080000Z`)
/// with the property's `VALUE` / `TZID` parameters.
pub fn parse_value(v: &str, value_type: Option<&str>, tzid: Option<&str>) -> Result<DateValue> {
    let v = v.trim();
    let is_date = value_type.is_some_and(|t| t.eq_ignore_ascii_case("DATE")) || v.len() == 8;
    if is_date {
        return NaiveDate::parse_from_str(v, "%Y%m%d")
            .map(DateValue::Date)
            .map_err(|_| anyhow!("bad date {v:?}"));
    }
    let (body, utc) = match v.strip_suffix('Z').or_else(|| v.strip_suffix('z')) {
        Some(b) => (b, true),
        None => (v, false),
    };
    let dt = NaiveDateTime::parse_from_str(body, "%Y%m%dT%H%M%S")
        .map_err(|_| anyhow!("bad date-time {v:?}"))?;
    let zone = if utc {
        ZoneRef::Utc
    } else {
        match tzid.map(str::trim).filter(|t| !t.is_empty()) {
            Some(t) => ZoneRef::Named(t.to_string()),
            None => ZoneRef::Floating,
        }
    };
    Ok(DateValue::DateTime(dt, zone))
}

/// A property's value (the first of a list).
pub fn prop_value(p: &Property) -> Result<DateValue> {
    let first = p.value.split(',').next().unwrap_or_default();
    parse_value(first, p.param("VALUE"), p.param("TZID"))
}

/// Every value of a list property (`EXDATE`, `RDATE`); a `PERIOD` counts
/// by its start. Unreadable entries are skipped.
pub fn prop_values(p: &Property) -> Vec<DateValue> {
    p.value
        .split(',')
        .filter_map(|v| {
            let v = v.split('/').next().unwrap_or(v);
            parse_value(
                v,
                p.param("VALUE")
                    .filter(|t| !t.eq_ignore_ascii_case("PERIOD")),
                p.param("TZID"),
            )
            .ok()
        })
        .collect()
}

/// `DURATION` (§3.3.6): `PT1H30M`, `P1D`, `P2W`, `-PT15M`. Negative ones
/// are returned as is (the caller clamps).
pub fn parse_duration(v: &str) -> Result<Duration> {
    let v = v.trim();
    let (neg, rest) = match v.strip_prefix('-') {
        Some(r) => (true, r),
        None => (false, v.strip_prefix('+').unwrap_or(v)),
    };
    let rest = rest
        .strip_prefix('P')
        .or_else(|| rest.strip_prefix('p'))
        .ok_or_else(|| anyhow!("bad duration {v:?}"))?;
    let mut total = Duration::zero();
    let mut num = String::new();
    let mut in_time = false;
    let mut any = false;
    for c in rest.chars() {
        match c.to_ascii_uppercase() {
            'T' => in_time = true,
            d if d.is_ascii_digit() => num.push(d),
            unit => {
                let n: i64 = num.parse().map_err(|_| anyhow!("bad duration {v:?}"))?;
                num.clear();
                any = true;
                total += match (unit, in_time) {
                    ('W', false) => Duration::weeks(n),
                    ('D', false) => Duration::days(n),
                    ('H', true) => Duration::hours(n),
                    ('M', true) => Duration::minutes(n),
                    ('S', true) => Duration::seconds(n),
                    _ => bail!("bad duration {v:?}"),
                };
            }
        }
    }
    if !num.is_empty() || !any {
        bail!("bad duration {v:?}");
    }
    Ok(if neg { -total } else { total })
}

/// A `VTIMEZONE` as a sorted list of transitions: from each local
/// wall-clock onset, the offset in force.
#[derive(Debug, Clone, Default)]
pub struct VTimezone {
    transitions: Vec<(NaiveDateTime, FixedOffset)>,
    /// Offset before the first transition.
    before: Option<FixedOffset>,
}

/// Transitions are generated up to this year.
const LAST_YEAR: i32 = 2100;

/// `+0100`, `-0530`, `+013000`.
fn parse_offset(v: &str) -> Result<FixedOffset> {
    let v = v.trim();
    let (sign, digits) = match v.split_at_checked(1) {
        Some(("+", d)) => (1, d),
        Some(("-", d)) => (-1, d),
        _ => bail!("bad UTC offset {v:?}"),
    };
    if !(digits.len() == 4 || digits.len() == 6) || !digits.chars().all(|c| c.is_ascii_digit()) {
        bail!("bad UTC offset {v:?}");
    }
    let n = |r: std::ops::Range<usize>| digits[r].parse::<i32>().unwrap_or(0);
    let secs = n(0..2) * 3600 + n(2..4) * 60 + if digits.len() == 6 { n(4..6) } else { 0 };
    FixedOffset::east_opt(sign * secs).ok_or_else(|| anyhow!("bad UTC offset {v:?}"))
}

impl VTimezone {
    /// Build from a `VTIMEZONE` component. Observances that can't be read
    /// are skipped; none readable is an error.
    pub fn from_component(c: &Component) -> Result<VTimezone> {
        let mut transitions = Vec::new();
        let mut first: Option<(NaiveDateTime, FixedOffset)> = None;
        let horizon = NaiveDate::from_ymd_opt(LAST_YEAR, 12, 31).unwrap_or(NaiveDate::MAX);
        for obs in c
            .children
            .iter()
            .filter(|o| o.name == "STANDARD" || o.name == "DAYLIGHT")
        {
            let Some(Ok(DateValue::DateTime(start, _))) = obs.prop("DTSTART").map(prop_value)
            else {
                continue;
            };
            let (Some(Ok(from)), Some(Ok(to))) = (
                obs.prop("TZOFFSETFROM").map(|p| parse_offset(&p.value)),
                obs.prop("TZOFFSETTO").map(|p| parse_offset(&p.value)),
            ) else {
                continue;
            };
            if first.is_none_or(|(t, _)| start < t) {
                first = Some((start, from));
            }
            let mut onsets = vec![start];
            if let Some(Ok(rule)) = obs.prop("RRULE").map(|p| RRule::parse(&p.value)) {
                let until = rule.until.map(|u| match u {
                    Until::Date(d) => d.and_hms_opt(23, 59, 59).unwrap_or_default(),
                    Until::Local(t) => t,
                    // UNTIL is UTC here; the onset is in the "from" time.
                    Until::Utc(t) => t + Duration::seconds(from.local_minus_utc() as i64),
                });
                rule.expand(start, until, horizon, &mut |t| {
                    onsets.push(t);
                    true
                });
            }
            for p in obs.props_named("RDATE") {
                for v in prop_values(p) {
                    if let DateValue::DateTime(t, _) = v {
                        onsets.push(t);
                    }
                }
            }
            transitions.extend(onsets.into_iter().map(|t| (t, to)));
        }
        if transitions.is_empty() {
            bail!("VTIMEZONE without a readable observance");
        }
        transitions.sort_by_key(|(t, _)| *t);
        transitions.dedup_by_key(|(t, _)| *t);
        Ok(VTimezone {
            transitions,
            before: first.map(|(_, from)| from),
        })
    }

    /// The offset in force at local wall-clock time `t`.
    pub fn offset_at(&self, t: NaiveDateTime) -> FixedOffset {
        let i = self.transitions.partition_point(|(onset, _)| *onset <= t);
        match i {
            0 => self.before.unwrap_or_else(|| self.transitions[0].1),
            _ => self.transitions[i - 1].1,
        }
    }
}

/// Windows zone names Outlook may write without a `VTIMEZONE` (CLDR's
/// mapping, the most used zones only).
const WINDOWS_ZONES: &[(&str, &str)] = &[
    ("UTC", "UTC"),
    ("GMT Standard Time", "Europe/London"),
    ("Greenwich Standard Time", "Atlantic/Reykjavik"),
    ("W. Europe Standard Time", "Europe/Berlin"),
    ("Central Europe Standard Time", "Europe/Budapest"),
    ("Central European Standard Time", "Europe/Warsaw"),
    ("Romance Standard Time", "Europe/Paris"),
    ("E. Europe Standard Time", "Europe/Chisinau"),
    ("FLE Standard Time", "Europe/Kiev"),
    ("GTB Standard Time", "Europe/Bucharest"),
    ("Russian Standard Time", "Europe/Moscow"),
    ("Turkey Standard Time", "Europe/Istanbul"),
    ("Israel Standard Time", "Asia/Jerusalem"),
    ("Arabian Standard Time", "Asia/Dubai"),
    ("India Standard Time", "Asia/Kolkata"),
    ("China Standard Time", "Asia/Shanghai"),
    ("Singapore Standard Time", "Asia/Singapore"),
    ("Tokyo Standard Time", "Asia/Tokyo"),
    ("Korea Standard Time", "Asia/Seoul"),
    ("AUS Eastern Standard Time", "Australia/Sydney"),
    ("New Zealand Standard Time", "Pacific/Auckland"),
    ("Eastern Standard Time", "America/New_York"),
    ("Central Standard Time", "America/Chicago"),
    ("Mountain Standard Time", "America/Denver"),
    ("US Mountain Standard Time", "America/Phoenix"),
    ("Pacific Standard Time", "America/Los_Angeles"),
    ("Alaskan Standard Time", "America/Anchorage"),
    ("Hawaiian Standard Time", "Pacific/Honolulu"),
    ("Atlantic Standard Time", "America/Halifax"),
    ("E. South America Standard Time", "America/Sao_Paulo"),
    ("SA Pacific Standard Time", "America/Bogota"),
    ("Central Standard Time (Mexico)", "America/Mexico_City"),
    ("South Africa Standard Time", "Africa/Johannesburg"),
    ("Egypt Standard Time", "Africa/Cairo"),
];

/// The IANA zone a `TZID` names, directly, behind a vendor prefix
/// (`/mozilla.org/20050126_1/Europe/Rome`) or as a common Windows name.
fn iana(tzid: &str) -> Option<chrono_tz::Tz> {
    let t = tzid.trim().trim_matches('"');
    if let Ok(tz) = t.parse::<chrono_tz::Tz>() {
        return Some(tz);
    }
    if t.starts_with('/') {
        // The last one or two path segments.
        let parts: Vec<&str> = t.split('/').filter(|s| !s.is_empty()).collect();
        for n in [3, 2, 1] {
            if parts.len() >= n {
                if let Ok(tz) = parts[parts.len() - n..].join("/").parse::<chrono_tz::Tz>() {
                    return Some(tz);
                }
            }
        }
    }
    None
}

fn windows_zone(tzid: &str) -> Option<chrono_tz::Tz> {
    let t = tzid.trim().trim_matches('"');
    WINDOWS_ZONES
        .iter()
        .find(|(w, _)| w.eq_ignore_ascii_case(t))
        .and_then(|(_, i)| i.parse().ok())
}

/// A resolved zone.
#[derive(Debug, Clone)]
enum Resolved {
    Iana(chrono_tz::Tz),
    File(VTimezone),
    Unknown,
}

/// The zones of one file, and the offset for floating times.
pub struct Zones {
    vtimezones: HashMap<String, VTimezone>,
    floating: FixedOffset,
    cache: RefCell<HashMap<String, Resolved>>,
    unknown: RefCell<BTreeSet<String>>,
}

impl Zones {
    /// `calendars`' `VTIMEZONE`s; floating times at `floating`.
    pub fn new(calendars: &[Component], floating: FixedOffset) -> Zones {
        let mut vtimezones = HashMap::new();
        for c in calendars {
            for tz in c.children.iter().filter(|c| c.name == "VTIMEZONE") {
                let Some(id) = tz.prop("TZID").map(|p| p.value.trim().to_string()) else {
                    continue;
                };
                if let Ok(v) = VTimezone::from_component(tz) {
                    vtimezones.insert(id, v);
                }
            }
        }
        Zones {
            vtimezones,
            floating,
            cache: RefCell::new(HashMap::new()),
            unknown: RefCell::new(BTreeSet::new()),
        }
    }

    pub fn floating(&self) -> FixedOffset {
        self.floating
    }

    /// `TZID`s that could not be resolved (read as floating).
    pub fn unknown(&self) -> Vec<String> {
        self.unknown.borrow().iter().cloned().collect()
    }

    fn resolve(&self, tzid: &str) -> Resolved {
        if let Some(r) = self.cache.borrow().get(tzid) {
            return r.clone();
        }
        let r = if let Some(tz) = iana(tzid) {
            Resolved::Iana(tz)
        } else if let Some(v) = self.vtimezones.get(tzid.trim()) {
            Resolved::File(v.clone())
        } else if let Some(tz) = windows_zone(tzid) {
            Resolved::Iana(tz)
        } else {
            self.unknown.borrow_mut().insert(tzid.to_string());
            Resolved::Unknown
        };
        self.cache.borrow_mut().insert(tzid.to_string(), r.clone());
        r
    }

    /// The instant of wall-clock time `t` in `zone`. A time skipped by a
    /// DST change moves forward by an hour; a repeated one takes the first.
    pub fn instant(&self, t: NaiveDateTime, zone: &ZoneRef) -> DateTime<Utc> {
        let fixed =
            |off: FixedOffset| (t - Duration::seconds(off.local_minus_utc() as i64)).and_utc();
        match zone {
            ZoneRef::Utc => t.and_utc(),
            ZoneRef::Floating => fixed(self.floating),
            ZoneRef::Named(id) => match self.resolve(id) {
                Resolved::Iana(tz) => match tz.from_local_datetime(&t) {
                    LocalResult::Single(d) => d.with_timezone(&Utc),
                    LocalResult::Ambiguous(a, _) => a.with_timezone(&Utc),
                    LocalResult::None => {
                        let later = t + Duration::hours(1);
                        match tz.from_local_datetime(&later) {
                            LocalResult::Single(d) | LocalResult::Ambiguous(d, _) => {
                                d.with_timezone(&Utc)
                            }
                            LocalResult::None => fixed(tz.offset_from_utc_datetime(&t).fix()),
                        }
                    }
                },
                Resolved::File(v) => fixed(v.offset_at(t)),
                Resolved::Unknown => fixed(self.floating),
            },
        }
    }

    /// Instant `u` as wall-clock time in `zone` (for `UNTIL`).
    pub fn local(&self, u: DateTime<Utc>, zone: &ZoneRef) -> NaiveDateTime {
        let naive = u.naive_utc();
        let shift = |off: FixedOffset| naive + Duration::seconds(off.local_minus_utc() as i64);
        match zone {
            ZoneRef::Utc => naive,
            ZoneRef::Floating => shift(self.floating),
            ZoneRef::Named(id) => match self.resolve(id) {
                Resolved::Iana(tz) => u.with_timezone(&tz).naive_local(),
                Resolved::File(v) => shift(v.offset_at(shift(v.offset_at(naive)))),
                Resolved::Unknown => shift(self.floating),
            },
        }
    }

    /// Midnight starting `d`, in the floating offset (all-day events).
    pub fn date_instant(&self, d: NaiveDate) -> DateTime<Utc> {
        self.instant(
            d.and_hms_opt(0, 0, 0).unwrap_or_default(),
            &ZoneRef::Floating,
        )
    }

    /// Any value as an instant (dates at floating midnight).
    pub fn value_instant(&self, v: &DateValue) -> DateTime<Utc> {
        match v {
            DateValue::Date(d) => self.date_instant(*d),
            DateValue::DateTime(t, z) => self.instant(*t, z),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::calendar::ics;

    fn utc(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }

    fn at(s: &str) -> NaiveDateTime {
        NaiveDateTime::parse_from_str(s, "%Y%m%dT%H%M%S").unwrap()
    }

    fn plus2() -> FixedOffset {
        FixedOffset::east_opt(2 * 3600).unwrap()
    }

    #[test]
    fn values_and_durations() {
        assert_eq!(
            parse_value("20260925", None, None).unwrap(),
            DateValue::Date(NaiveDate::from_ymd_opt(2026, 9, 25).unwrap())
        );
        assert_eq!(
            parse_value("20260925T080000Z", None, Some("Europe/Rome")).unwrap(),
            DateValue::DateTime(at("20260925T080000"), ZoneRef::Utc)
        );
        assert_eq!(
            parse_value("20260925T100000", None, Some("Europe/Rome")).unwrap(),
            DateValue::DateTime(at("20260925T100000"), ZoneRef::Named("Europe/Rome".into()))
        );
        assert!(parse_value("2026-09-25", None, None).is_err());
        assert!(parse_value("20260925T10", None, None).is_err());
        assert_eq!(parse_duration("PT1H30M").unwrap(), Duration::minutes(90));
        assert_eq!(parse_duration("P1DT2H").unwrap(), Duration::hours(26));
        assert_eq!(parse_duration("P2W").unwrap(), Duration::weeks(2));
        assert_eq!(parse_duration("-PT15M").unwrap(), -Duration::minutes(15));
        for bad in ["1H", "PT", "PT1X", "P1H", "PT5"] {
            assert!(parse_duration(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn iana_windows_and_prefixed_names() {
        let z = Zones::new(&[], plus2());
        let rome = |id: &str, t: &str| z.instant(at(t), &ZoneRef::Named(id.into()));
        // Summer (CEST, +2) and winter (CET, +1).
        assert_eq!(
            rome("Europe/Rome", "20260925T100000"),
            utc("2026-09-25T08:00:00Z")
        );
        assert_eq!(
            rome("Europe/Rome", "20261215T100000"),
            utc("2026-12-15T09:00:00Z")
        );
        assert_eq!(
            rome("/mozilla.org/20050126_1/Europe/Rome", "20260925T100000"),
            utc("2026-09-25T08:00:00Z")
        );
        assert_eq!(
            rome("W. Europe Standard Time", "20261215T100000"),
            utc("2026-12-15T09:00:00Z")
        );
        assert_eq!(
            rome("America/New_York", "20260925T100000"),
            utc("2026-09-25T14:00:00Z")
        );
        // Floating and unknown zones use the meeting's offset, and the
        // unknown one is reported.
        assert_eq!(
            z.instant(at("20260925T100000"), &ZoneRef::Floating),
            utc("2026-09-25T08:00:00Z")
        );
        assert_eq!(
            rome("Mars/Olympus", "20260925T100000"),
            utc("2026-09-25T08:00:00Z")
        );
        assert_eq!(z.unknown(), ["Mars/Olympus"]);
        // DST gap (02:30 doesn't exist on 2026-03-29 in Rome): moved on.
        assert_eq!(
            rome("Europe/Rome", "20260329T023000"),
            utc("2026-03-29T01:30:00Z")
        );
        // And back again for UNTIL.
        assert_eq!(
            z.local(
                utc("2026-09-25T08:00:00Z"),
                &ZoneRef::Named("Europe/Rome".into())
            ),
            at("20260925T100000")
        );
    }

    /// An Outlook-style zone known only through its VTIMEZONE.
    #[test]
    fn vtimezone_observances_with_yearly_rules() {
        let text = "BEGIN:VCALENDAR\r\nBEGIN:VTIMEZONE\r\nTZID:Customized Time Zone\r\n\
BEGIN:STANDARD\r\nDTSTART:16010101T030000\r\nTZOFFSETFROM:+0200\r\nTZOFFSETTO:+0100\r\n\
RRULE:FREQ=YEARLY;INTERVAL=1;BYDAY=-1SU;BYMONTH=10\r\nEND:STANDARD\r\n\
BEGIN:DAYLIGHT\r\nDTSTART:16010101T020000\r\nTZOFFSETFROM:+0100\r\nTZOFFSETTO:+0200\r\n\
RRULE:FREQ=YEARLY;INTERVAL=1;BYDAY=-1SU;BYMONTH=3\r\nEND:DAYLIGHT\r\nEND:VTIMEZONE\r\nEND:VCALENDAR\r\n";
        let cals = ics::parse(text).unwrap().calendars;
        let z = Zones::new(&cals, FixedOffset::east_opt(0).unwrap());
        let named = ZoneRef::Named("Customized Time Zone".into());
        assert_eq!(
            z.instant(at("20260925T100000"), &named),
            utc("2026-09-25T08:00:00Z")
        );
        assert_eq!(
            z.instant(at("20261215T100000"), &named),
            utc("2026-12-15T09:00:00Z")
        );
        assert_eq!(
            z.instant(at("20260329T010000"), &named),
            utc("2026-03-29T00:00:00Z")
        );
        assert_eq!(
            z.instant(at("20260329T030000"), &named),
            utc("2026-03-29T01:00:00Z")
        );
        assert!(z.unknown().is_empty());
        assert_eq!(
            z.local(utc("2026-12-15T09:00:00Z"), &named),
            at("20261215T100000")
        );
    }

    #[test]
    fn offsets() {
        assert_eq!(
            parse_offset("+0530").unwrap().local_minus_utc(),
            5 * 3600 + 1800
        );
        assert_eq!(parse_offset("-0800").unwrap().local_minus_utc(), -8 * 3600);
        for bad in ["0100", "+1", "+01:00", "+99999999"] {
            assert!(parse_offset(bad).is_err(), "{bad}");
        }
    }
}
