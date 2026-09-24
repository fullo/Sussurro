//! Library facets (0.9, #135; plan §9): filter the archive by type, tag,
//! category, participant and date, with counts from the search index.
//!
//! **Semantics.** Values selected within one facet are ORed, facets are
//! ANDed with each other and with the full-text query. Counts are
//! *disjunctive*: each facet is counted over the items matching the query
//! and every *other* facet's selection, so a facet's own alternatives stay
//! visible while one of its values is selected.
//!
//! **Grouping.** Tags and categories group by [`value_key`] (case and
//! spacing folded, accents kept: "caffè" and "caffe" are different tags).
//! Participants group by who they are: the People registry person they
//! link to (`person:<id>`, same rule as the People screen's counts,
//! [`PeopleMatcher`]), otherwise their normalized name (`name:<name key>`).
//! The key is stored in the index next to each participant and recomputed
//! when `people.json` changes (see `index::refresh_people`).
//!
//! **Dates.** Buckets are cumulative (today ⊂ this week ⊂ this month ⊂ this
//! year; "older" is before this year), so the date facet selects one bucket
//! or a custom range, not several. They compare the item's day *as written
//! in its frontmatter* (the local day where it was recorded) with the
//! viewer's local `today`, sent by the UI — never a UTC day, so an item
//! recorded at 00:30 local time is "today", not "yesterday". Weeks start on
//! Monday (ISO 8601).
//!
//! **Safety.** Every value reaches SQLite as a bound parameter; the SQL
//! text is assembled only from fixed fragments and `?` placeholders whose
//! number depends on the list lengths (capped at [`MAX_VALUES`]).

use super::index::{fts_query, SearchFilters};
use super::people::{name_key, PeopleMatcher, Person};
use super::store::ItemSummary;
use super::types::ItemType;
use anyhow::{bail, Context, Result};
use chrono::{Datelike, NaiveDate};
use rusqlite::types::Value;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Most values one facet may select (keeps the statement size bounded).
pub const MAX_VALUES: usize = 200;

/// Facet key prefix of a participant linked to a People registry person.
pub const PERSON_PREFIX: &str = "person:";
/// Facet key prefix of a participant known only by name.
pub const NAME_PREFIX: &str = "name:";

/// A date bucket, relative to the viewer's local today.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DateBucket {
    Today,
    Week,
    Month,
    Year,
    Older,
}

impl DateBucket {
    pub const ALL: [DateBucket; 5] = [
        DateBucket::Today,
        DateBucket::Week,
        DateBucket::Month,
        DateBucket::Year,
        DateBucket::Older,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            DateBucket::Today => "today",
            DateBucket::Week => "week",
            DateBucket::Month => "month",
            DateBucket::Year => "year",
            DateBucket::Older => "older",
        }
    }

    fn label(self) -> &'static str {
        match self {
            DateBucket::Today => "Today",
            DateBucket::Week => "This week",
            DateBucket::Month => "This month",
            DateBucket::Year => "This year",
            DateBucket::Older => "Older",
        }
    }
}

/// First days of the buckets for a given local today.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DateBounds {
    pub today: NaiveDate,
    /// Monday of this week.
    pub week: NaiveDate,
    pub month: NaiveDate,
    pub year: NaiveDate,
}

impl DateBounds {
    pub fn new(today: NaiveDate) -> Self {
        let week = today - chrono::Days::new(u64::from(today.weekday().num_days_from_monday()));
        DateBounds {
            today,
            week,
            month: today.with_day(1).unwrap_or(today),
            year: NaiveDate::from_ymd_opt(today.year(), 1, 1).unwrap_or(today),
        }
    }

    /// SQL condition on `i.day` (fixed text) and its arguments selecting a
    /// bucket: from the bucket's first day through today, or before this
    /// year for "older". An item dated after today (another time zone, a
    /// wrong clock) is in no bucket, only in the unfiltered list.
    fn condition(&self, bucket: DateBucket) -> (&'static str, Vec<String>) {
        let day = |d: NaiveDate| d.format("%Y-%m-%d").to_string();
        let through_today = |from: NaiveDate| vec![day(from), day(self.today)];
        match bucket {
            DateBucket::Today => ("i.day = ?", vec![day(self.today)]),
            DateBucket::Week => ("i.day BETWEEN ? AND ?", through_today(self.week)),
            DateBucket::Month => ("i.day BETWEEN ? AND ?", through_today(self.month)),
            DateBucket::Year => ("i.day BETWEEN ? AND ?", through_today(self.year)),
            DateBucket::Older => ("i.day < ?", vec![day(self.year)]),
        }
    }
}

/// The viewer's today from the filters (`YYYY-MM-DD`), else this
/// computer's local date.
pub fn resolve_today(v: &Option<String>) -> Result<NaiveDate> {
    match v.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        None => Ok(chrono::Local::now().date_naive()),
        Some(s) => NaiveDate::parse_from_str(s.get(..10).unwrap_or(s), "%Y-%m-%d")
            .with_context(|| format!("invalid today '{s}' (expected YYYY-MM-DD)")),
    }
}

/// Grouping key of a tag or category: lowercased, whitespace trimmed and
/// collapsed. Accents are kept.
pub fn value_key(s: &str) -> String {
    s.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// Facet key of a participant (`email` empty when unknown).
pub fn participant_key(matcher: &PeopleMatcher, name: &str, email: &str) -> String {
    match matcher.person_for(name, email) {
        Some(id) => format!("{PERSON_PREFIX}{id}"),
        None => format!("{NAME_PREFIX}{}", name_key(name)),
    }
}

/// One value of a facet with the number of items it would show.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct FacetValue {
    /// What goes back in the filters.
    pub key: String,
    /// What the UI shows.
    pub label: String,
    pub count: usize,
}

/// Facet counts for the current query and filters.
#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
pub struct Facets {
    /// Items matching the query and every filter.
    pub total: usize,
    /// Always the three types, in order.
    pub types: Vec<FacetValue>,
    /// By count, then label.
    pub tags: Vec<FacetValue>,
    pub categories: Vec<FacetValue>,
    pub participants: Vec<FacetValue>,
    /// Always the five buckets, in order (cumulative, see the module doc).
    pub dates: Vec<FacetValue>,
}

/// The Library's list and its facets, from one consistent index snapshot.
#[derive(Debug, Clone, Serialize)]
pub struct FacetedSearch {
    pub items: Vec<ItemSummary>,
    pub facets: Facets,
}

/// Which facet a count leaves out of the filters (disjunctive counting).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Facet {
    Type,
    Tag,
    Category,
    Participant,
    Date,
}

/// `FROM … WHERE …` of the items matching a query and filters, with its
/// arguments in placeholder order. Selects from `items i` (plus `items_fts`
/// when there is a text query).
pub(super) struct Matching {
    pub from_where: String,
    pub args: Vec<Value>,
    pub fts: bool,
}

fn placeholders(n: usize) -> String {
    vec!["?"; n].join(", ")
}

fn trimmed(v: &Option<String>) -> Option<String> {
    v.as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// Distinct non-empty values of a facet list, normalized with `key`.
fn keys(facet: &str, values: &[String], key: impl Fn(&str) -> String) -> Result<Vec<String>> {
    let mut out: Vec<String> = Vec::new();
    for v in values {
        let k = key(v);
        if !k.is_empty() && !out.contains(&k) {
            out.push(k);
        }
    }
    if out.len() > MAX_VALUES {
        bail!(
            "too many {facet} selected ({}, at most {MAX_VALUES})",
            out.len()
        );
    }
    Ok(out)
}

/// Normalize a date bound to `YYYY-MM-DD` (an RFC 3339 value is cut).
fn day_bound(v: &Option<String>) -> Result<Option<String>> {
    let Some(s) = trimmed(v) else {
        return Ok(None);
    };
    let day = s.get(..10).unwrap_or(&s);
    NaiveDate::parse_from_str(day, "%Y-%m-%d")
        .with_context(|| format!("invalid date filter '{s}' (expected YYYY-MM-DD)"))?;
    Ok(Some(day.to_string()))
}

/// Build the matching clause, leaving out `skip`'s own selection.
pub(super) fn matching(
    query: &str,
    f: &SearchFilters,
    bounds: &DateBounds,
    skip: Option<Facet>,
) -> Result<Matching> {
    let mut sql = String::new();
    let mut args: Vec<Value> = Vec::new();
    let fts = fts_query(query);
    if let Some(q) = &fts {
        sql.push_str(
            "FROM items_fts JOIN items i ON i.rowid = items_fts.rowid WHERE items_fts MATCH ?",
        );
        args.push(Value::Text(q.clone()));
    } else {
        sql.push_str("FROM items i WHERE 1");
    }
    let keep = |facet: Facet| skip != Some(facet);

    if keep(Facet::Type) {
        if let Some(t) = f.item_type {
            sql.push_str(" AND i.item_type = ?");
            args.push(Value::Text(t.as_str().into()));
        }
        let mut types: Vec<ItemType> = Vec::new();
        for t in &f.types {
            if !types.contains(t) {
                types.push(*t);
            }
        }
        if !types.is_empty() {
            sql.push_str(&format!(
                " AND i.item_type IN ({})",
                placeholders(types.len())
            ));
            args.extend(types.iter().map(|t| Value::Text(t.as_str().into())));
        }
    }
    if keep(Facet::Tag) {
        if let Some(tag) = trimmed(&f.tag) {
            sql.push_str(
                " AND EXISTS (SELECT 1 FROM item_tags t WHERE t.id = i.id AND t.tag = ? COLLATE NOCASE)",
            );
            args.push(Value::Text(tag));
        }
        let tags = keys("tags", &f.tags, value_key)?;
        if !tags.is_empty() {
            sql.push_str(&format!(
                " AND EXISTS (SELECT 1 FROM item_tags t WHERE t.id = i.id AND t.tag_key IN ({}))",
                placeholders(tags.len())
            ));
            args.extend(tags.into_iter().map(Value::Text));
        }
    }
    if keep(Facet::Category) {
        if let Some(cat) = trimmed(&f.category) {
            sql.push_str(
                " AND EXISTS (SELECT 1 FROM item_categories c WHERE c.id = i.id \
                 AND c.category = ? COLLATE NOCASE)",
            );
            args.push(Value::Text(cat));
        }
        let cats = keys("categories", &f.categories, value_key)?;
        if !cats.is_empty() {
            sql.push_str(&format!(
                " AND EXISTS (SELECT 1 FROM item_categories c WHERE c.id = i.id \
                 AND c.category_key IN ({}))",
                placeholders(cats.len())
            ));
            args.extend(cats.into_iter().map(Value::Text));
        }
    }
    if keep(Facet::Participant) {
        if let Some(p) = trimmed(&f.participant) {
            sql.push_str(
                " AND EXISTS (SELECT 1 FROM item_participants p WHERE p.id = i.id \
                 AND (p.name = ? COLLATE NOCASE OR p.email = ? COLLATE NOCASE))",
            );
            args.push(Value::Text(p.clone()));
            args.push(Value::Text(p));
        }
        let people = keys("participants", &f.participants, |s| s.trim().to_string())?;
        if !people.is_empty() {
            sql.push_str(&format!(
                " AND EXISTS (SELECT 1 FROM item_participants p WHERE p.id = i.id \
                 AND p.pkey IN ({}))",
                placeholders(people.len())
            ));
            args.extend(people.into_iter().map(Value::Text));
        }
    }
    if keep(Facet::Date) {
        if let Some(bucket) = f.date_bucket {
            // `cond` is fixed text; the days are bound.
            let (cond, days) = bounds.condition(bucket);
            sql.push_str(&format!(" AND {cond}"));
            args.extend(days.into_iter().map(Value::Text));
        }
        if let Some(from) = day_bound(&f.date_from)? {
            sql.push_str(" AND i.day >= ?");
            args.push(Value::Text(from));
        }
        if let Some(to) = day_bound(&f.date_to)? {
            sql.push_str(" AND i.day <= ?");
            args.push(Value::Text(to));
        }
    }
    Ok(Matching {
        from_where: sql,
        args,
        fts: fts.is_some(),
    })
}

fn by_count_then_label(values: &mut [FacetValue]) {
    values.sort_by_cached_key(|v| {
        (
            std::cmp::Reverse(v.count),
            name_key(&v.label),
            v.key.clone(),
        )
    });
}

/// `(key, label, count)` rows of a multi-valued facet table.
fn grouped(conn: &Connection, sql: &str, args: &[Value]) -> Result<Vec<FacetValue>> {
    let mut stmt = conn.prepare(sql)?;
    let rows = stmt.query_map(rusqlite::params_from_iter(args.iter()), |r| {
        Ok(FacetValue {
            key: r.get(0)?,
            label: r.get(1)?,
            count: r.get::<_, i64>(2)?.max(0) as usize,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// Count every facet for `query` + `filters`. `people` gives the labels of
/// registry-linked participants.
pub(super) fn facets_conn(
    conn: &Connection,
    query: &str,
    filters: &SearchFilters,
    people: &[Person],
) -> Result<Facets> {
    let bounds = DateBounds::new(resolve_today(&filters.today)?);

    let all = matching(query, filters, &bounds, None)?;
    let total: i64 = conn.query_row(
        &format!("SELECT count(*) {}", all.from_where),
        rusqlite::params_from_iter(all.args.iter()),
        |r| r.get(0),
    )?;

    // Types: always the three, in order.
    let m = matching(query, filters, &bounds, Some(Facet::Type))?;
    let mut by_type: HashMap<String, usize> = HashMap::new();
    {
        let mut stmt = conn.prepare(&format!(
            "SELECT i.item_type, count(*) {} GROUP BY i.item_type",
            m.from_where
        ))?;
        let rows = stmt.query_map(rusqlite::params_from_iter(m.args.iter()), |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
        })?;
        for row in rows {
            let (t, n) = row?;
            by_type.insert(t, n.max(0) as usize);
        }
    }
    let types = [ItemType::Note, ItemType::Meeting, ItemType::Transcription]
        .iter()
        .map(|t| FacetValue {
            key: t.as_str().into(),
            label: t.as_str().into(),
            count: by_type.get(t.as_str()).copied().unwrap_or(0),
        })
        .collect();

    // Dates: one pass, one sum per bucket.
    let m = matching(query, filters, &bounds, Some(Facet::Date))?;
    let mut args: Vec<Value> = Vec::new();
    let mut sums = Vec::new();
    for b in DateBucket::ALL {
        let (cond, days) = bounds.condition(b);
        sums.push(format!("coalesce(sum({cond}), 0)"));
        args.extend(days.into_iter().map(Value::Text));
    }
    args.extend(m.args.iter().cloned());
    let counts: Vec<i64> = conn.query_row(
        &format!("SELECT {} {}", sums.join(", "), m.from_where),
        rusqlite::params_from_iter(args.iter()),
        |r| (0..DateBucket::ALL.len()).map(|i| r.get(i)).collect(),
    )?;
    let dates = DateBucket::ALL
        .iter()
        .zip(counts)
        .map(|(b, n)| FacetValue {
            key: b.as_str().into(),
            label: b.label().into(),
            count: n.max(0) as usize,
        })
        .collect();

    let m = matching(query, filters, &bounds, Some(Facet::Tag))?;
    let mut tags = grouped(
        conn,
        &format!(
            "SELECT t.tag_key, min(t.tag), count(DISTINCT t.id) FROM item_tags t \
             WHERE t.id IN (SELECT i.id {}) GROUP BY t.tag_key",
            m.from_where
        ),
        &m.args,
    )?;
    by_count_then_label(&mut tags);

    let m = matching(query, filters, &bounds, Some(Facet::Category))?;
    let mut categories = grouped(
        conn,
        &format!(
            "SELECT c.category_key, min(c.category), count(DISTINCT c.id) FROM item_categories c \
             WHERE c.id IN (SELECT i.id {}) GROUP BY c.category_key",
            m.from_where
        ),
        &m.args,
    )?;
    by_count_then_label(&mut categories);

    let m = matching(query, filters, &bounds, Some(Facet::Participant))?;
    let mut participants = grouped(
        conn,
        &format!(
            "SELECT p.pkey, min(p.name), count(DISTINCT p.id) FROM item_participants p \
             WHERE p.id IN (SELECT i.id {}) GROUP BY p.pkey",
            m.from_where
        ),
        &m.args,
    )?;
    let names: HashMap<&str, &str> = people
        .iter()
        .map(|p| (p.id.as_str(), p.name.as_str()))
        .collect();
    for v in &mut participants {
        if let Some(name) = v
            .key
            .strip_prefix(PERSON_PREFIX)
            .and_then(|id| names.get(id))
        {
            v.label = (*name).to_string();
        }
    }
    by_count_then_label(&mut participants);

    Ok(Facets {
        total: total.max(0) as usize,
        types,
        tags,
        categories,
        participants,
        dates,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    #[test]
    fn bounds_start_weeks_on_monday_and_months_on_the_first() {
        // Thursday 24 September 2026.
        let b = DateBounds::new(d("2026-09-24"));
        assert_eq!(b.week, d("2026-09-21"));
        assert_eq!(b.month, d("2026-09-01"));
        assert_eq!(b.year, d("2026-01-01"));
        // Sunday: still the week that began on Monday.
        assert_eq!(DateBounds::new(d("2026-09-27")).week, d("2026-09-21"));
        // Monday: the week begins today.
        assert_eq!(DateBounds::new(d("2026-09-28")).week, d("2026-09-28"));
        // A week across a month and a year boundary.
        let b = DateBounds::new(d("2027-01-01"));
        assert_eq!(b.week, d("2026-12-28"));
        assert_eq!(b.month, d("2027-01-01"));
        assert_eq!(b.year, d("2027-01-01"));
    }

    #[test]
    fn today_comes_from_the_ui_or_the_local_clock() {
        assert_eq!(
            resolve_today(&Some("2026-02-03".into())).unwrap(),
            d("2026-02-03")
        );
        assert_eq!(
            resolve_today(&Some("2026-02-03T23:59:00+01:00".into())).unwrap(),
            d("2026-02-03")
        );
        assert_eq!(
            resolve_today(&None).unwrap(),
            chrono::Local::now().date_naive()
        );
        assert!(resolve_today(&Some("tomorrow".into())).is_err());
        assert!(resolve_today(&Some("2026-02-30".into())).is_err());
    }

    #[test]
    fn value_keys_fold_case_and_spacing_but_not_accents() {
        assert_eq!(value_key("  Release   0.7 "), "release 0.7");
        assert_eq!(value_key("CAFFÈ"), "caffè");
        assert_ne!(value_key("caffè"), value_key("caffe"));
    }

    #[test]
    fn placeholders_only_depend_on_the_count() {
        assert_eq!(placeholders(3), "?, ?, ?");
        assert_eq!(placeholders(1), "?");
    }

    #[test]
    fn too_many_values_are_refused() {
        let many: Vec<String> = (0..=MAX_VALUES).map(|i| format!("t{i}")).collect();
        let f = SearchFilters {
            tags: many,
            ..Default::default()
        };
        let b = DateBounds::new(d("2026-09-24"));
        assert!(matching("", &f, &b, None).is_err());
        // Duplicates collapse before the cap.
        let f = SearchFilters {
            tags: vec!["A".into(); MAX_VALUES * 2],
            ..Default::default()
        };
        let m = matching("", &f, &b, None).unwrap();
        assert_eq!(m.args.len(), 1);
    }
}

/// Facets against a real index built from an archive folder.
#[cfg(test)]
mod index_tests {
    use super::*;
    use crate::archive::index::{rebuild_index, with_index, Index};
    use crate::archive::people::{add_person, modify};
    use crate::archive::store::{create_item, update_meta};
    use crate::archive::types::{ItemMeta, Participant, Segment, SegmentsFile};
    use std::path::{Path, PathBuf};

    const TODAY: &str = "2026-09-24"; // a Thursday

    struct Fx {
        _tmp: tempfile::TempDir,
        archive: PathBuf,
        db: PathBuf,
    }

    fn fx() -> Fx {
        let tmp = tempfile::tempdir().unwrap();
        Fx {
            archive: tmp.path().join("Sussurro"),
            db: tmp.path().join("appdata").join("index.sqlite"),
            _tmp: tmp,
        }
    }

    fn who(name: &str, email: Option<&str>) -> Participant {
        Participant {
            name: name.into(),
            email: email.map(Into::into),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn add(
        archive: &Path,
        t: ItemType,
        title: &str,
        date: &str,
        text: &str,
        tags: &[&str],
        cats: &[&str],
        people: Vec<Participant>,
    ) -> String {
        let meta = ItemMeta {
            item_type: t,
            title: title.into(),
            date: date.into(),
            tags: tags.iter().map(|s| s.to_string()).collect(),
            categories: cats.iter().map(|s| s.to_string()).collect(),
            participants: people,
            ..Default::default()
        };
        let segs = SegmentsFile {
            segments: vec![Segment {
                id: 0,
                start_ms: 0,
                end_ms: 1000,
                raw: text.into(),
                text: text.into(),
                ..Default::default()
            }],
            ..Default::default()
        };
        create_item(archive, &meta, &segs).unwrap()
    }

    /// Five items:
    /// - `standup`  meeting  today 00:30 (+02:00)  tags release, Roadmap  cat team  Anna (email), Voice 1
    /// - `retro`    meeting  Monday this week      tags RELEASE           cat team  "annie" (alias), Bruno
    /// - `idea`     note     1 Sep 2026            tags idea              cat —     —
    /// - `podcast`  transcr. 15 Mar 2026           tags release           cat media "Anna R" (email of Anna), "voice  1"
    /// - `old`      note     31 Dec 2025 23:30 (-05:00)  tags —           cat team  —
    struct Items {
        fx: Fx,
        standup: String,
        retro: String,
        idea: String,
        podcast: String,
        old: String,
    }

    fn items() -> Items {
        let fx = fx();
        modify(&fx.archive, |ps| {
            add_person(
                ps,
                &Person {
                    name: "Anna Rossi".into(),
                    email: Some("anna@example.com".into()),
                    aliases: vec!["Annie".into()],
                    ..Default::default()
                },
            )
        })
        .unwrap();
        let a = &fx.archive;
        let standup = add(
            a,
            ItemType::Meeting,
            "Standup",
            "2026-09-24T00:30:00+02:00",
            "Parliamo della release",
            &["release", "Roadmap"],
            &["team"],
            vec![
                who("Anna Rossi", Some("anna@example.com")),
                who("Voice 1", None),
            ],
        );
        let retro = add(
            a,
            ItemType::Meeting,
            "Retro",
            "2026-09-21T17:00:00+02:00",
            "Cosa è andato bene",
            &["RELEASE"],
            &["Team"],
            vec![who("annie", None), who("Bruno", None)],
        );
        let idea = add(
            a,
            ItemType::Note,
            "Idea",
            "2026-09-01T09:00:00+02:00",
            "Un'idea per la release",
            &["idea"],
            &[],
            vec![],
        );
        let podcast = add(
            a,
            ItemType::Transcription,
            "Podcast",
            "2026-03-15T10:00:00+01:00",
            "Intervista",
            &["release"],
            &["media"],
            vec![
                who("Anna R", Some("ANNA@example.com")),
                who("voice  1", None),
            ],
        );
        let old = add(
            a,
            ItemType::Note,
            "Old",
            "2025-12-31T23:30:00-05:00",
            "Fine anno",
            &[],
            &["team"],
            vec![],
        );
        rebuild_index(&fx.archive, &fx.db).unwrap();
        Items {
            fx,
            standup,
            retro,
            idea,
            podcast,
            old,
        }
    }

    fn base() -> SearchFilters {
        SearchFilters {
            today: Some(TODAY.into()),
            ..Default::default()
        }
    }

    fn count(values: &[FacetValue], key: &str) -> usize {
        values.iter().find(|v| v.key == key).map_or(0, |v| v.count)
    }

    fn ids(r: &FacetedSearch) -> Vec<String> {
        let mut v: Vec<String> = r.items.iter().map(|s| s.id.clone()).collect();
        v.sort();
        v
    }

    fn sorted(mut v: Vec<&String>) -> Vec<String> {
        v.sort();
        v.into_iter().cloned().collect()
    }

    fn run(it: &Items, query: &str, f: &SearchFilters) -> FacetedSearch {
        Index::open(&it.fx.archive, &it.fx.db)
            .unwrap()
            .search_faceted(query, f)
            .unwrap()
    }

    #[test]
    fn counts_every_facet_with_grouped_values() {
        let it = items();
        let r = run(&it, "", &base());
        let f = &r.facets;
        assert_eq!(f.total, 5);
        assert_eq!(r.items.len(), 5);
        assert_eq!(count(&f.types, "meeting"), 2);
        assert_eq!(count(&f.types, "note"), 2);
        assert_eq!(count(&f.types, "transcription"), 1);
        // Case-folded tags; the most used first.
        assert_eq!(f.tags[0].key, "release");
        assert_eq!(f.tags[0].count, 3);
        assert_eq!(f.tags[0].label, "RELEASE"); // min() of the spellings
        assert_eq!(count(&f.tags, "roadmap"), 1);
        assert_eq!(count(&f.tags, "idea"), 1);
        assert_eq!(count(&f.categories, "team"), 3);
        assert_eq!(count(&f.categories, "media"), 1);
        // Participants: Anna by email, by alias and by email with another
        // name are one person; "Voice 1" and "voice  1" are one name.
        let anna = f
            .participants
            .iter()
            .find(|v| v.key.starts_with(PERSON_PREFIX))
            .unwrap();
        assert_eq!((anna.label.as_str(), anna.count), ("Anna Rossi", 3));
        assert_eq!(count(&f.participants, "name:voice 1"), 2);
        assert_eq!(count(&f.participants, "name:bruno"), 1);
        assert_eq!(f.participants.len(), 3);
        // Cumulative date buckets.
        assert_eq!(count(&f.dates, "today"), 1);
        assert_eq!(count(&f.dates, "week"), 2);
        assert_eq!(count(&f.dates, "month"), 3);
        assert_eq!(count(&f.dates, "year"), 4);
        assert_eq!(count(&f.dates, "older"), 1);
    }

    #[test]
    fn or_within_a_facet_and_across_facets() {
        let it = items();
        // Two tags: either.
        let f = SearchFilters {
            tags: vec!["Idea".into(), "roadmap".into()],
            ..base()
        };
        assert_eq!(ids(&run(&it, "", &f)), sorted(vec![&it.idea, &it.standup]));
        // Two types: either.
        let f = SearchFilters {
            types: vec![ItemType::Note, ItemType::Transcription],
            ..base()
        };
        assert_eq!(
            ids(&run(&it, "", &f)),
            sorted(vec![&it.idea, &it.podcast, &it.old])
        );
        // Tag AND category AND participant.
        let anna = run(&it, "", &base())
            .facets
            .participants
            .into_iter()
            .find(|v| v.key.starts_with(PERSON_PREFIX))
            .unwrap()
            .key;
        let f = SearchFilters {
            tags: vec!["release".into()],
            categories: vec!["TEAM".into()],
            participants: vec![anna.clone()],
            ..base()
        };
        assert_eq!(ids(&run(&it, "", &f)), sorted(vec![&it.standup, &it.retro]));
        // Participant values OR'ed, then AND with the type.
        let f = SearchFilters {
            participants: vec!["name:bruno".into(), "name:voice 1".into()],
            types: vec![ItemType::Transcription],
            ..base()
        };
        assert_eq!(ids(&run(&it, "", &f)), vec![it.podcast.clone()]);
    }

    #[test]
    fn counts_are_disjunctive_and_follow_the_text_query() {
        let it = items();
        let f = SearchFilters {
            types: vec![ItemType::Meeting],
            ..base()
        };
        let r = run(&it, "", &f);
        assert_eq!(r.facets.total, 2);
        // The type facet still offers the other types with their counts…
        assert_eq!(count(&r.facets.types, "note"), 2);
        assert_eq!(count(&r.facets.types, "meeting"), 2);
        // …while the other facets count only meetings.
        assert_eq!(count(&r.facets.tags, "release"), 2);
        assert_eq!(count(&r.facets.tags, "idea"), 0);
        assert_eq!(count(&r.facets.categories, "media"), 0);
        assert_eq!(count(&r.facets.dates, "older"), 0);

        // A text query narrows every count.
        let r = run(&it, "release", &base());
        assert_eq!(r.facets.total, 4);
        assert_eq!(
            ids(&r),
            sorted(vec![&it.standup, &it.retro, &it.idea, &it.podcast])
        );
        assert_eq!(count(&r.facets.types, "note"), 1);
        assert_eq!(count(&r.facets.dates, "year"), 4);
        assert_eq!(count(&r.facets.dates, "older"), 0);
        assert_eq!(count(&r.facets.categories, "team"), 2);
    }

    #[test]
    fn date_buckets_use_the_local_day_and_the_viewers_today() {
        let it = items();
        let by = |bucket: DateBucket, today: &str| {
            ids(&run(
                &it,
                "",
                &SearchFilters {
                    date_bucket: Some(bucket),
                    today: Some(today.into()),
                    ..Default::default()
                },
            ))
        };
        // 00:30 at +02:00 is still the 23rd in UTC, but it is "today".
        assert_eq!(by(DateBucket::Today, TODAY), vec![it.standup.clone()]);
        assert_eq!(
            by(DateBucket::Week, TODAY),
            sorted(vec![&it.standup, &it.retro])
        );
        // 23:30 at -05:00 on Dec 31 is Jan 1 in UTC, but it belongs to 2025.
        assert_eq!(by(DateBucket::Older, TODAY), vec![it.old.clone()]);
        assert_eq!(by(DateBucket::Today, "2025-12-31"), vec![it.old.clone()]);
        assert!(by(DateBucket::Today, "2026-01-01").is_empty());
        // Sunday: the week that began on Monday the 21st.
        assert_eq!(
            by(DateBucket::Week, "2026-09-27"),
            sorted(vec![&it.standup, &it.retro])
        );
        // The following Monday: a new week, nothing yet.
        assert!(by(DateBucket::Week, "2026-09-28").is_empty());
        // Custom range with a bucket-free facet.
        let r = run(
            &it,
            "",
            &SearchFilters {
                date_from: Some("2026-03-01".into()),
                date_to: Some("2026-09-01".into()),
                ..base()
            },
        );
        assert_eq!(ids(&r), sorted(vec![&it.idea, &it.podcast]));
        // The date facet ignores its own range when counting.
        assert_eq!(count(&r.facets.dates, "today"), 1);
        assert!(Index::open(&it.fx.archive, &it.fx.db)
            .unwrap()
            .search_faceted(
                "",
                &SearchFilters {
                    today: Some("not a day".into()),
                    ..Default::default()
                }
            )
            .is_err());
    }

    #[test]
    fn participants_regroup_when_the_people_registry_changes() {
        let it = items();
        let search = |f: &SearchFilters| {
            with_index(&it.fx.archive, &it.fx.db, |i| i.search_faceted("", f)).unwrap()
        };
        let r = search(&base());
        assert_eq!(count(&r.facets.participants, "name:bruno"), 1);

        // Bruno joins People, with "Voice 1" as an alias: both group under him.
        let bruno = modify(&it.fx.archive, |ps| {
            add_person(
                ps,
                &Person {
                    name: "Bruno Verdi".into(),
                    aliases: vec!["Bruno".into(), "Voice 1".into()],
                    ..Default::default()
                },
            )
        })
        .unwrap();
        let key = format!("{PERSON_PREFIX}{}", bruno.id);
        let r = search(&base());
        let v = r.facets.participants.iter().find(|v| v.key == key).unwrap();
        assert_eq!((v.label.as_str(), v.count), ("Bruno Verdi", 3));
        assert_eq!(count(&r.facets.participants, "name:bruno"), 0);
        let only = search(&SearchFilters {
            participants: vec![key.clone()],
            ..base()
        });
        assert_eq!(
            ids(&only),
            sorted(vec![&it.standup, &it.retro, &it.podcast])
        );

        // An item edited in the app is grouped with the current registry too.
        let mut meta = crate::archive::store::read_item(&it.fx.archive, &it.idea)
            .unwrap()
            .meta;
        meta.item_type = ItemType::Meeting;
        meta.participants = vec![who("bruno", None)];
        update_meta(&it.fx.archive, &it.idea, &meta).unwrap();
        Index::open(&it.fx.archive, &it.fx.db)
            .unwrap()
            .index_item(&it.idea)
            .unwrap();
        let r = Index::open(&it.fx.archive, &it.fx.db)
            .unwrap()
            .search_faceted("", &base())
            .unwrap();
        assert_eq!(
            r.facets
                .participants
                .iter()
                .find(|v| v.key == key)
                .unwrap()
                .count,
            4
        );
    }

    #[test]
    fn injection_shaped_inputs_are_plain_values() {
        let it = items();
        let evil = [
            "'; DROP TABLE items; --",
            "\" OR 1=1 --",
            "release' OR '1'='1",
            ") OR 1=1 OR (",
            "%",
            "_",
            "*",
            "\\",
            "NEAR(release",
            "person:' OR pkey LIKE '%",
            "name:%",
            "\u{0}",
        ];
        let idx = Index::open(&it.fx.archive, &it.fx.db).unwrap();
        for e in evil {
            let f = SearchFilters {
                tags: vec![e.into()],
                categories: vec![e.into()],
                participants: vec![e.into()],
                tag: Some(e.into()),
                participant: Some(e.into()),
                ..base()
            };
            let r = idx.search_faceted(e, &f).unwrap();
            assert!(r.items.is_empty(), "{e}");
            assert_eq!(r.facets.total, 0, "{e}");
            // A single evil value in one facet matches nothing either.
            for f in [
                SearchFilters {
                    tags: vec![e.into()],
                    ..base()
                },
                SearchFilters {
                    participants: vec![e.into()],
                    ..base()
                },
            ] {
                assert_eq!(idx.search_faceted("", &f).unwrap().facets.total, 0, "{e}");
            }
            // Malformed dates are refused, not interpolated.
            let f = SearchFilters {
                date_from: Some(e.into()),
                ..base()
            };
            assert!(idx.search_faceted("", &f).is_err(), "{e}");
        }
        // Everything is still there.
        assert_eq!(idx.search_faceted("", &base()).unwrap().facets.total, 5);
        // Unknown bucket and type names are refused by deserialization.
        assert!(serde_json::from_str::<SearchFilters>(r#"{"date_bucket":"1=1"}"#).is_err());
        let ok: SearchFilters = serde_json::from_str(
            r#"{"types":["note"],"tags":["a"],"date_bucket":"week","today":"2026-09-24"}"#,
        )
        .unwrap();
        assert_eq!(ok.date_bucket, Some(DateBucket::Week));
    }

    /// 10k synthetic items seeded straight into the index: one faceted
    /// search (list + every count) stays interactive. The bound is the
    /// acceptance target in release builds and lenient in debug builds
    /// (unoptimized SQLite); the time is printed either way.
    #[test]
    fn ten_thousand_items_facet_quickly() {
        let fx = fx();
        std::fs::create_dir_all(&fx.archive).unwrap();
        let idx = Index::open(&fx.archive, &fx.db).unwrap();
        let words = [
            "release", "roadmap", "budget", "design", "hiring", "retro", "caffè",
        ];
        let names = [
            "Anna Rossi",
            "Bruno",
            "Carla Neri",
            "Voice 1",
            "Voice 2",
            "Dario",
        ];
        let start_day = NaiveDate::from_ymd_opt(2023, 1, 1).unwrap();
        idx.with_conn_for_tests(|conn| {
            let tx = conn.transaction()?;
            for n in 0..10_000usize {
                let id = format!("2026/01/item-{n:05}");
                let t = [ItemType::Note, ItemType::Meeting, ItemType::Transcription][n % 3];
                let day = start_day + chrono::Days::new((n % 1360) as u64);
                let date = format!("{}T10:00:00+02:00", day.format("%Y-%m-%d"));
                let tags: Vec<String> = vec![
                    words[n % words.len()].into(),
                    format!("topic{}", n % 50),
                ];
                let cats: Vec<String> = vec![format!("cat{}", n % 8)];
                let people: Vec<Participant> = if t == ItemType::Note {
                    vec![]
                } else {
                    (0..3).map(|k| who(names[(n + k) % names.len()], None)).collect()
                };
                let meta = ItemMeta {
                    item_type: t,
                    title: format!("Item {n} {}", words[(n / 7) % words.len()]),
                    date: date.clone(),
                    tags: tags.clone(),
                    categories: cats.clone(),
                    participants: people.clone(),
                    ..Default::default()
                };
                let ts = chrono::DateTime::parse_from_rfc3339(&date).unwrap().timestamp();
                tx.execute(
                    "INSERT INTO items (id, item_type, sort_ts, day, fingerprint, edited, meta_json)
                     VALUES (?1, ?2, ?3, ?4, '-', 0, ?5)",
                    rusqlite::params![
                        id,
                        t.as_str(),
                        ts,
                        day.format("%Y-%m-%d").to_string(),
                        serde_json::to_string(&meta)?
                    ],
                )?;
                let rowid = tx.last_insert_rowid();
                tx.execute(
                    "INSERT INTO items_fts (rowid, title, body, tags, categories, participants)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    rusqlite::params![
                        rowid,
                        meta.title,
                        format!("Testo di prova numero {n} sulla {} e altro", words[n % 5]),
                        tags.join(" "),
                        cats.join(" "),
                        people.iter().map(|p| p.name.as_str()).collect::<Vec<_>>().join(" ")
                    ],
                )?;
                for tag in &tags {
                    tx.execute(
                        "INSERT INTO item_tags (id, tag, tag_key) VALUES (?1, ?2, ?3)",
                        rusqlite::params![id, tag, value_key(tag)],
                    )?;
                }
                for c in &cats {
                    tx.execute(
                        "INSERT INTO item_categories (id, category, category_key) VALUES (?1, ?2, ?3)",
                        rusqlite::params![id, c, value_key(c)],
                    )?;
                }
                for p in &people {
                    tx.execute(
                        "INSERT INTO item_participants (id, name, email, pkey) VALUES (?1, ?2, '', ?3)",
                        rusqlite::params![id, p.name, format!("{NAME_PREFIX}{}", name_key(&p.name))],
                    )?;
                }
            }
            tx.commit()?;
            Ok(())
        })
        .unwrap();

        // The budget is a release-build figure. An unoptimized test build
        // on a shared CI runner already spends ~1.4 s here on a busy M-series
        // Mac, so its limit only catches a complexity regression (#187).
        let limit = if cfg!(debug_assertions) { 15_000 } else { 200 };
        let cases = [
            ("", base()),
            (
                "release",
                SearchFilters {
                    types: vec![ItemType::Meeting, ItemType::Transcription],
                    tags: vec!["release".into(), "topic7".into()],
                    participants: vec!["name:anna rossi".into()],
                    date_bucket: Some(DateBucket::Older),
                    ..base()
                },
            ),
        ];
        for (q, f) in cases {
            let t = std::time::Instant::now();
            let r = idx.search_faceted(q, &f).unwrap();
            let ms = t.elapsed().as_millis();
            eprintln!(
                "faceted search over 10k items ({q:?}): {ms} ms, {} hits",
                r.items.len()
            );
            assert!(r.facets.total > 0);
            assert_eq!(r.items.len(), r.facets.total);
            assert!(ms < limit, "{ms} ms (limit {limit} ms)");
        }
    }
}
