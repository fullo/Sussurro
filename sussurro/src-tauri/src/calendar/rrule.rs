//! RRULE (RFC 5545 §3.3.10) expansion, just enough to find the occurrences
//! of a recurring event near one day (E21): `FREQ` daily / weekly /
//! monthly / yearly with `INTERVAL`, `COUNT`, `UNTIL`, `BYDAY` (with
//! ordinals such as `2TU` or `-1FR`), `BYMONTHDAY` (negative too),
//! `BYMONTH`, `BYSETPOS` and `WKST`. That covers what Google, Outlook and
//! Apple write for meetings (and the yearly rules of `VTIMEZONE`s).
//!
//! Anything else (`BYYEARDAY`, `BYWEEKNO`, `BYHOUR`…, sub-daily
//! frequencies) is refused by [`RRule::parse`]; the caller then keeps only
//! the first occurrence and says so.
//!
//! Everything here works on wall-clock times ([`NaiveDateTime`]) in the
//! event's own time zone: a weekly 10:00 meeting stays at 10:00 across a
//! DST change. The caller turns each occurrence into an instant.

use anyhow::{anyhow, bail, Result};
use chrono::{Datelike, Duration, NaiveDate, NaiveDateTime, Weekday};

/// Recurrence frequency (the sub-daily ones are not supported).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Freq {
    Daily,
    Weekly,
    Monthly,
    Yearly,
}

/// `UNTIL`, as written: a date, or a date-time (UTC when it ended in `Z`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Until {
    Date(NaiveDate),
    Local(NaiveDateTime),
    Utc(NaiveDateTime),
}

/// A parsed, supported recurrence rule.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RRule {
    pub freq: Freq,
    pub interval: u32,
    pub count: Option<u32>,
    pub until: Option<Until>,
    /// `(ordinal, weekday)`: `2TU` is `(Some(2), Tue)`, `MO` is `(None, Mon)`.
    pub by_day: Vec<(Option<i32>, Weekday)>,
    pub by_month_day: Vec<i32>,
    pub by_month: Vec<u32>,
    pub by_set_pos: Vec<i32>,
    pub wkst: Weekday,
}

/// Periods (days, weeks, months or years) walked at most per expansion, a
/// guard against rules that never match (`BYMONTH=2;BYMONTHDAY=30`).
pub const MAX_PERIODS: u32 = 200_000;

fn weekday(s: &str) -> Option<Weekday> {
    Some(match s {
        "MO" => Weekday::Mon,
        "TU" => Weekday::Tue,
        "WE" => Weekday::Wed,
        "TH" => Weekday::Thu,
        "FR" => Weekday::Fri,
        "SA" => Weekday::Sat,
        "SU" => Weekday::Sun,
        _ => return None,
    })
}

fn int_list(v: &str, what: &str, valid: impl Fn(i32) -> bool) -> Result<Vec<i32>> {
    v.split(',')
        .map(|s| {
            let n: i32 = s
                .trim()
                .parse()
                .map_err(|_| anyhow!("bad {what} value {s:?}"))?;
            if !valid(n) {
                bail!("{what} value {n} out of range");
            }
            Ok(n)
        })
        .collect()
}

/// `20261231`, `20261231T235959` or `20261231T235959Z`.
pub fn parse_until(v: &str) -> Result<Until> {
    let v = v.trim();
    if v.len() == 8 {
        return NaiveDate::parse_from_str(v, "%Y%m%d")
            .map(Until::Date)
            .map_err(|_| anyhow!("bad UNTIL {v:?}"));
    }
    let (body, utc) = match v.strip_suffix('Z').or_else(|| v.strip_suffix('z')) {
        Some(b) => (b, true),
        None => (v, false),
    };
    let dt = NaiveDateTime::parse_from_str(body, "%Y%m%dT%H%M%S")
        .map_err(|_| anyhow!("bad UNTIL {v:?}"))?;
    Ok(if utc {
        Until::Utc(dt)
    } else {
        Until::Local(dt)
    })
}

impl RRule {
    /// Parse the value of an `RRULE` property. Unsupported parts are an
    /// error (the caller falls back to the first occurrence).
    pub fn parse(value: &str) -> Result<RRule> {
        let mut freq = None;
        let mut rule = RRule {
            freq: Freq::Daily,
            interval: 1,
            count: None,
            until: None,
            by_day: Vec::new(),
            by_month_day: Vec::new(),
            by_month: Vec::new(),
            by_set_pos: Vec::new(),
            wkst: Weekday::Mon,
        };
        for part in value.trim().split(';').filter(|p| !p.trim().is_empty()) {
            let (k, v) = part
                .split_once('=')
                .ok_or_else(|| anyhow!("bad rule part {part:?}"))?;
            let k = k.trim().to_ascii_uppercase();
            let v = v.trim().to_ascii_uppercase();
            match k.as_str() {
                "FREQ" => {
                    freq = Some(match v.as_str() {
                        "DAILY" => Freq::Daily,
                        "WEEKLY" => Freq::Weekly,
                        "MONTHLY" => Freq::Monthly,
                        "YEARLY" => Freq::Yearly,
                        other => bail!("FREQ={other} is not supported"),
                    })
                }
                "INTERVAL" => {
                    let n: u32 = v.parse().map_err(|_| anyhow!("bad INTERVAL {v:?}"))?;
                    rule.interval = n.max(1);
                }
                "COUNT" => rule.count = Some(v.parse().map_err(|_| anyhow!("bad COUNT {v:?}"))?),
                "UNTIL" => rule.until = Some(parse_until(&v)?),
                "BYDAY" => {
                    for d in v.split(',') {
                        let d = d.trim();
                        if d.len() < 2 {
                            bail!("bad BYDAY {d:?}");
                        }
                        let (num, day) = d.split_at(d.len() - 2);
                        let wd = weekday(day).ok_or_else(|| anyhow!("bad BYDAY {d:?}"))?;
                        let ord = if num.is_empty() {
                            None
                        } else {
                            let n: i32 = num.parse().map_err(|_| anyhow!("bad BYDAY {d:?}"))?;
                            if n == 0 || n.abs() > 53 {
                                bail!("bad BYDAY {d:?}");
                            }
                            Some(n)
                        };
                        rule.by_day.push((ord, wd));
                    }
                }
                "BYMONTHDAY" => {
                    rule.by_month_day =
                        int_list(&v, "BYMONTHDAY", |n| n != 0 && (-31..=31).contains(&n))?
                }
                "BYMONTH" => {
                    rule.by_month = int_list(&v, "BYMONTH", |n| (1..=12).contains(&n))?
                        .into_iter()
                        .map(|n| n as u32)
                        .collect()
                }
                "BYSETPOS" => {
                    rule.by_set_pos =
                        int_list(&v, "BYSETPOS", |n| n != 0 && (-366..=366).contains(&n))?
                }
                "WKST" => rule.wkst = weekday(&v).ok_or_else(|| anyhow!("bad WKST {v:?}"))?,
                // A rule may repeat the start's own time: harmless.
                other => bail!("{other} is not supported"),
            }
        }
        rule.freq = freq.ok_or_else(|| anyhow!("the rule has no FREQ"))?;
        Ok(rule)
    }

    /// Walk the occurrences from `dtstart` (included, RFC 5545: it is the
    /// first one), in order, calling `visit` with each until it returns
    /// false, `COUNT` or `until` is reached, or a period starts after
    /// `horizon`. `until` is the rule's `UNTIL` already brought into the
    /// event's wall-clock time by the caller.
    pub fn expand(
        &self,
        dtstart: NaiveDateTime,
        until: Option<NaiveDateTime>,
        horizon: NaiveDate,
        visit: &mut dyn FnMut(NaiveDateTime) -> bool,
    ) {
        let time = dtstart.time();
        let start = dtstart.date();
        let mut seen = 0u32;
        for k in 0..MAX_PERIODS {
            let Some((period_start, dates)) = self.period(start, k) else {
                return;
            };
            if period_start > horizon {
                return;
            }
            for d in dates {
                let dt = d.and_time(time);
                if dt < dtstart {
                    continue;
                }
                if until.is_some_and(|u| dt > u) {
                    return;
                }
                seen += 1;
                if !visit(dt) {
                    return;
                }
                if self.count.is_some_and(|c| seen >= c) {
                    return;
                }
            }
        }
    }

    /// The `k`-th period after the one holding `start`: its first day and
    /// its matching dates, sorted, with `BYSETPOS` applied. `None` past the
    /// calendar's range.
    fn period(&self, start: NaiveDate, k: u32) -> Option<(NaiveDate, Vec<NaiveDate>)> {
        let step = k.checked_mul(self.interval)?;
        let (first, mut dates) = match self.freq {
            Freq::Daily => {
                let day = start.checked_add_signed(Duration::days(step as i64))?;
                let keep = self.month_ok(day)
                    && self.month_day_ok(day)
                    && (self.by_day.is_empty()
                        || self.by_day.iter().any(|(_, w)| *w == day.weekday()));
                (day, if keep { vec![day] } else { vec![] })
            }
            Freq::Weekly => {
                let back = (7 + start.weekday().num_days_from_monday()
                    - self.wkst.num_days_from_monday())
                    % 7;
                let week = start
                    .checked_sub_signed(Duration::days(back as i64))?
                    .checked_add_signed(Duration::weeks(step as i64))?;
                let mut out = Vec::new();
                for i in 0..7 {
                    let d = week.checked_add_signed(Duration::days(i))?;
                    let day_ok = if self.by_day.is_empty() {
                        d.weekday() == start.weekday()
                    } else {
                        self.by_day.iter().any(|(_, w)| *w == d.weekday())
                    };
                    if day_ok && self.month_ok(d) {
                        out.push(d);
                    }
                }
                (week, out)
            }
            Freq::Monthly => {
                let months = start.month0() as i64 + step as i64;
                let year = start.year() as i64 + months.div_euclid(12);
                let month = months.rem_euclid(12) as u32 + 1;
                let first = NaiveDate::from_ymd_opt(i32::try_from(year).ok()?, month, 1)?;
                let out = if !self.by_month.is_empty() && !self.by_month.contains(&month) {
                    vec![]
                } else {
                    self.days_in_month(first, start.day())
                };
                (first, out)
            }
            Freq::Yearly => {
                let year = start.year().checked_add(i32::try_from(step).ok()?)?;
                let first = NaiveDate::from_ymd_opt(year, 1, 1)?;
                let mut out = Vec::new();
                if !self.by_month.is_empty() {
                    for &m in &self.by_month {
                        if let Some(f) = NaiveDate::from_ymd_opt(year, m, 1) {
                            out.extend(self.days_in_month(f, start.day()));
                        }
                    }
                } else if !self.by_month_day.is_empty() {
                    for m in 1..=12 {
                        if let Some(f) = NaiveDate::from_ymd_opt(year, m, 1) {
                            out.extend(self.days_in_month(f, start.day()));
                        }
                    }
                } else if !self.by_day.is_empty() {
                    // BYDAY alone: ordinals count within the year.
                    let last = NaiveDate::from_ymd_opt(year, 12, 31)?;
                    let mut d = first;
                    while d <= last {
                        if self
                            .by_day
                            .iter()
                            .any(|&(ord, w)| day_matches(d, ord, w, d.ordinal0(), last.ordinal0()))
                        {
                            out.push(d);
                        }
                        d = d.succ_opt()?;
                    }
                } else if let Some(d) = NaiveDate::from_ymd_opt(year, start.month(), start.day()) {
                    out.push(d);
                }
                (first, out)
            }
        };
        dates.sort();
        dates.dedup();
        Some((first, self.set_pos(dates)))
    }

    /// Matching days of the month starting at `first`: `BYMONTHDAY` and
    /// `BYDAY` (ordinals within the month) when given, else the start's
    /// day of the month (skipped in months without it).
    fn days_in_month(&self, first: NaiveDate, start_day: u32) -> Vec<NaiveDate> {
        let len = month_len(first);
        if self.by_day.is_empty() && self.by_month_day.is_empty() {
            return NaiveDate::from_ymd_opt(first.year(), first.month(), start_day)
                .into_iter()
                .collect();
        }
        (1..=len)
            .filter_map(|d| NaiveDate::from_ymd_opt(first.year(), first.month(), d))
            .filter(|d| {
                self.month_day_ok(*d)
                    && (self.by_day.is_empty()
                        || self
                            .by_day
                            .iter()
                            .any(|&(ord, w)| day_matches(*d, ord, w, d.day0(), len - 1)))
            })
            .collect()
    }

    fn month_ok(&self, d: NaiveDate) -> bool {
        self.by_month.is_empty() || self.by_month.contains(&d.month())
    }

    fn month_day_ok(&self, d: NaiveDate) -> bool {
        if self.by_month_day.is_empty() {
            return true;
        }
        let len = month_len(d) as i32;
        let day = d.day() as i32;
        self.by_month_day
            .iter()
            .any(|&n| if n > 0 { n == day } else { len + 1 + n == day })
    }

    fn set_pos(&self, dates: Vec<NaiveDate>) -> Vec<NaiveDate> {
        if self.by_set_pos.is_empty() || dates.is_empty() {
            return dates;
        }
        let n = dates.len() as i32;
        let mut out: Vec<NaiveDate> = self
            .by_set_pos
            .iter()
            .filter_map(|&p| {
                let i = if p > 0 { p - 1 } else { n + p };
                (0..n).contains(&i).then(|| dates[i as usize])
            })
            .collect();
        out.sort();
        out.dedup();
        out
    }
}

/// Does day `d` (index `idx` in its month or year, whose last index is
/// `last`) match `BYDAY` entry `(ord, w)`?
fn day_matches(d: NaiveDate, ord: Option<i32>, w: Weekday, idx: u32, last: u32) -> bool {
    if d.weekday() != w {
        return false;
    }
    match ord {
        None => true,
        Some(n) if n > 0 => (idx / 7 + 1) as i32 == n,
        Some(n) => ((last - idx) / 7 + 1) as i32 == -n,
    }
}

fn month_len(d: NaiveDate) -> u32 {
    let (y, m) = (d.year(), d.month());
    let next = if m == 12 {
        NaiveDate::from_ymd_opt(y + 1, 1, 1)
    } else {
        NaiveDate::from_ymd_opt(y, m + 1, 1)
    };
    next.and_then(|n| n.pred_opt()).map_or(31, |l| l.day())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dt(s: &str) -> NaiveDateTime {
        NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M").unwrap()
    }

    fn all(rule: &str, start: &str, limit: usize) -> Vec<String> {
        let r = RRule::parse(rule).unwrap();
        let until = r.until.map(|u| match u {
            Until::Date(d) => d.and_hms_opt(23, 59, 59).unwrap(),
            Until::Local(t) | Until::Utc(t) => t,
        });
        let mut out = Vec::new();
        r.expand(
            dt(start),
            until,
            NaiveDate::from_ymd_opt(2100, 1, 1).unwrap(),
            &mut |d| {
                out.push(d.format("%Y-%m-%d %a %H:%M").to_string());
                out.len() < limit
            },
        );
        out
    }

    #[test]
    fn weekly_with_count_and_several_days() {
        assert_eq!(
            all("FREQ=WEEKLY;COUNT=4;BYDAY=TU,TH", "2026-09-01 10:00", 99),
            [
                "2026-09-01 Tue 10:00",
                "2026-09-03 Thu 10:00",
                "2026-09-08 Tue 10:00",
                "2026-09-10 Thu 10:00"
            ]
        );
        // Every other week, starting on the start's weekday.
        assert_eq!(
            all("FREQ=WEEKLY;INTERVAL=2", "2026-09-02 09:30", 3),
            [
                "2026-09-02 Wed 09:30",
                "2026-09-16 Wed 09:30",
                "2026-09-30 Wed 09:30"
            ]
        );
    }

    #[test]
    fn weekly_interval_respects_wkst() {
        // RFC 5545's own example: WKST changes which weeks are "every other".
        let mo = all(
            "FREQ=WEEKLY;INTERVAL=2;COUNT=4;BYDAY=TU,SU;WKST=MO",
            "1997-08-05 09:00",
            99,
        );
        assert_eq!(
            mo,
            [
                "1997-08-05 Tue 09:00",
                "1997-08-10 Sun 09:00",
                "1997-08-19 Tue 09:00",
                "1997-08-24 Sun 09:00"
            ]
        );
        let su = all(
            "FREQ=WEEKLY;INTERVAL=2;COUNT=4;BYDAY=TU,SU;WKST=SU",
            "1997-08-05 09:00",
            99,
        );
        assert_eq!(
            su,
            [
                "1997-08-05 Tue 09:00",
                "1997-08-17 Sun 09:00",
                "1997-08-19 Tue 09:00",
                "1997-08-31 Sun 09:00"
            ]
        );
    }

    #[test]
    fn monthly_ordinals_last_weekday_and_setpos() {
        assert_eq!(
            all("FREQ=MONTHLY;BYDAY=2TU", "2026-09-08 15:00", 3),
            [
                "2026-09-08 Tue 15:00",
                "2026-10-13 Tue 15:00",
                "2026-11-10 Tue 15:00"
            ]
        );
        assert_eq!(
            all("FREQ=MONTHLY;BYDAY=-1FR", "2026-09-25 15:00", 2),
            ["2026-09-25 Fri 15:00", "2026-10-30 Fri 15:00"]
        );
        // Outlook's "last weekday of the month".
        assert_eq!(
            all(
                "FREQ=MONTHLY;BYDAY=MO,TU,WE,TH,FR;BYSETPOS=-1",
                "2026-09-30 17:00",
                3
            ),
            [
                "2026-09-30 Wed 17:00",
                "2026-10-30 Fri 17:00",
                "2026-11-30 Mon 17:00"
            ]
        );
        // The 31st skips short months; -1 is always the last day.
        assert_eq!(
            all("FREQ=MONTHLY", "2026-08-31 08:00", 3),
            [
                "2026-08-31 Mon 08:00",
                "2026-10-31 Sat 08:00",
                "2026-12-31 Thu 08:00"
            ]
        );
        assert_eq!(
            all("FREQ=MONTHLY;BYMONTHDAY=-1", "2026-01-31 08:00", 2),
            ["2026-01-31 Sat 08:00", "2026-02-28 Sat 08:00"]
        );
    }

    #[test]
    fn daily_until_and_yearly_timezone_rules() {
        assert_eq!(
            all("FREQ=DAILY;UNTIL=20260903T235959Z", "2026-09-01 07:00", 99).len(),
            3
        );
        assert_eq!(
            all("FREQ=DAILY;UNTIL=20260902", "2026-09-01 07:00", 99).len(),
            2
        );
        assert_eq!(
            all(
                "FREQ=DAILY;BYDAY=MO,TU,WE,TH,FR;COUNT=6",
                "2026-09-25 09:00",
                99
            ),
            [
                "2026-09-25 Fri 09:00",
                "2026-09-28 Mon 09:00",
                "2026-09-29 Tue 09:00",
                "2026-09-30 Wed 09:00",
                "2026-10-01 Thu 09:00",
                "2026-10-02 Fri 09:00"
            ]
        );
        // A VTIMEZONE's DST rule: last Sunday of March.
        assert_eq!(
            all("FREQ=YEARLY;BYMONTH=3;BYDAY=-1SU", "1981-03-29 02:00", 99)
                .into_iter()
                .filter(|s| s.starts_with("2026") || s.starts_with("2027"))
                .collect::<Vec<_>>(),
            ["2026-03-29 Sun 02:00", "2027-03-28 Sun 02:00"]
        );
        assert_eq!(
            all("FREQ=YEARLY", "2024-02-29 12:00", 2),
            ["2024-02-29 Thu 12:00", "2028-02-29 Tue 12:00"]
        );
    }

    #[test]
    fn horizon_stops_open_ended_rules() {
        let r = RRule::parse("FREQ=DAILY").unwrap();
        let mut n = 0;
        r.expand(
            dt("2026-01-01 10:00"),
            None,
            NaiveDate::from_ymd_opt(2026, 1, 10).unwrap(),
            &mut |_| {
                n += 1;
                true
            },
        );
        assert_eq!(n, 10);
        // A rule that never matches ends at the horizon too.
        let r = RRule::parse("FREQ=YEARLY;BYMONTH=2;BYMONTHDAY=30").unwrap();
        r.expand(
            dt("2026-01-01 10:00"),
            None,
            NaiveDate::from_ymd_opt(2030, 1, 1).unwrap(),
            &mut |_| panic!(),
        );
    }

    #[test]
    fn unsupported_and_malformed_rules_are_refused() {
        for bad in [
            "FREQ=HOURLY",
            "FREQ=WEEKLY;BYWEEKNO=20",
            "FREQ=YEARLY;BYYEARDAY=100",
            "FREQ=DAILY;BYHOUR=9",
            "BYDAY=MO",
            "FREQ=WEEKLY;BYDAY=XX",
            "FREQ=MONTHLY;BYMONTHDAY=0",
            "FREQ=WEEKLY;COUNT=many",
            "FREQ=WEEKLY;UNTIL=soon",
            "nonsense",
        ] {
            assert!(RRule::parse(bad).is_err(), "{bad}");
        }
        assert!(RRule::parse("freq=weekly;byday=mo").is_ok());
    }
}
