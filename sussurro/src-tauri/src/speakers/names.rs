//! Names from the meeting page (plan §4.3 layer 2, #131, P8): the page
//! events of a browser meeting (`speaker_active`, `speaker_idle`,
//! `speaker_name`, `participants` — [`MeetingEvent`]) become a timeline of
//! who was speaking, and each remote line takes the name that was active
//! for most of it. Pure — unit tested; [`SharedNames`] lets the `/live`
//! connection feed the timeline while the engine reads it.
//!
//! **Timeline.** Every event carries `t_ms`, the client's position in the
//! audio it sent (the clock the lines' `start_ms` use). A speaker is an
//! `id` (a participant key stable for the call — on Meet the RTP
//! contributing source) or, from protocol 1 clients, a bare name:
//! - with an `id`, the speaker is active from `speaker_active` to its
//!   `speaker_idle` (several can overlap);
//! - a bare name is one "who is speaking" indicator: it stays active until
//!   the next bare name (or its own `speaker_idle`).
//!
//! An id's name is the last `speaker_name` for it (`null` = unknown after
//! all), else the name its `speaker_active` carried. The last binding
//! applies to the whole call: the key is the same person throughout, and
//! the page may learn the name late.
//!
//! **Lag.** The page's speaking indicator and captions show up after the
//! audio; their intervals move earlier by [`AttributionParams::dom_lag_ms`]
//! / `caption_lag_ms` (RTP needs none). Segment edges are the least sure
//! part (VAD, residual lag), so a line longer than 3 × `tolerance_ms` is
//! measured without its first and last `tolerance_ms`.
//!
//! **Decision** ([`attribute`]): the name covering the most of the
//! (trimmed) line wins if it covers at least `min_share_pct` of it and
//! beats the runner-up by `margin_pct`. A winner whose name is unknown, a
//! close call (overlapping speakers) or no signal at all leave the line to
//! the voice clustering ("Voice N"): a wrong name is worse than none.
//!
//! **Bounds (#217).** The events come from a web page, so the timeline is
//! bounded: at most [`MAX_SPEAKER_KEYS`] distinct speakers (later new ones
//! are ignored), [`MAX_TIMELINE_PARTICIPANTS`] participants, and
//! [`MAX_INTERVALS`] intervals — past that the oldest finished half ages
//! out and [`NameTimeline::forgotten_before`] says up to when the timeline
//! is incomplete (the end-of-run pass leaves those lines as they are).

use crate::archive::meeting::{MeetingEvent, NameSource};
use crate::archive::people::{link_new_participants, name_key, Person};
use crate::archive::types::{normalize_participants, Participant};
use crate::archive::ItemMeta;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

/// Distinct speakers (ids and bare names) a timeline follows; a page
/// inventing new ids past this is ignored for new ones.
pub const MAX_SPEAKER_KEYS: usize = 1_000;
/// Participants kept (the protocol's own cap on one list).
pub const MAX_TIMELINE_PARTICIPANTS: usize = 500;
/// Intervals kept before the oldest finished half ages out (a busy
/// three-hour call has a few thousand).
pub const MAX_INTERVALS: usize = 20_000;

/// How remote lines are matched to the page's speaker timeline. The lags
/// are first guesses: the real indicator lag is measured on live calls
/// (#105 / #184); change them here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AttributionParams {
    /// The page's speaking indicator lags the audio by about this much.
    pub dom_lag_ms: u64,
    /// Live captions lag the audio by about this much.
    pub caption_lag_ms: u64,
    /// Edge of a line left out of the vote (VAD edges, lag jitter).
    pub tolerance_ms: u64,
    /// The winner must cover at least this share of the line (percent).
    pub min_share_pct: u32,
    /// …and at least this multiple of the runner-up (percent: 150 = 1.5×).
    pub margin_pct: u32,
}

impl Default for AttributionParams {
    fn default() -> Self {
        Self {
            dom_lag_ms: 400,
            caption_lag_ms: 1_500,
            tolerance_ms: 250,
            min_share_pct: 50,
            margin_pct: 150,
        }
    }
}

impl AttributionParams {
    fn lag(&self, source: NameSource) -> i64 {
        (match source {
            NameSource::Rtp => 0,
            NameSource::Dom => self.dom_lag_ms,
            NameSource::Caption => self.caption_lag_ms,
        }) as i64
    }
}

/// Who a line is, per the page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Attribution {
    /// This display name was active for most of the line.
    Named(String),
    /// A participant was, but the page never told their name.
    Unknown,
    /// Two speakers, or too little of the line covered.
    Ambiguous,
    /// Nobody was shown speaking.
    Silent,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
enum Key {
    Id(String),
    Name(String),
}

#[derive(Debug, Clone)]
struct Interval {
    key: Key,
    /// Lag-compensated, ms on the session clock (may be negative).
    start: i64,
    /// `None`: still active.
    end: Option<i64>,
}

/// The page's speaker timeline of one meeting.
#[derive(Debug, Default, Clone)]
pub struct NameTimeline {
    params: AttributionParams,
    intervals: Vec<Interval>,
    /// Open interval per key (index into `intervals`) and its source.
    open: HashMap<Key, (usize, NameSource)>,
    /// `speaker_name` bindings: the last one wins (`None` = cleared).
    bound: HashMap<String, Option<String>>,
    /// Names `speaker_active` carried with an id.
    carried: HashMap<String, String>,
    /// Participants in order of first appearance.
    participants: Vec<String>,
    /// `name_key`s of `participants`.
    participant_keys: HashSet<String>,
    /// Every speaker key seen (bounded by [`MAX_SPEAKER_KEYS`]).
    keys: HashSet<Key>,
    /// Intervals ending before this (ms, lag-compensated) aged out.
    forgotten_before: Option<i64>,
}

impl NameTimeline {
    pub fn new(params: AttributionParams) -> Self {
        Self {
            params,
            ..Default::default()
        }
    }

    pub fn from_events(params: AttributionParams, events: &[MeetingEvent]) -> Self {
        let mut t = Self::new(params);
        for e in events {
            t.push(e);
        }
        t
    }

    pub fn params(&self) -> &AttributionParams {
        &self.params
    }

    /// Whether the page said anything about speakers at all.
    pub fn is_empty(&self) -> bool {
        self.intervals.is_empty() && self.bound.is_empty() && self.participants.is_empty()
    }

    fn key(id: &Option<String>, name: &Option<String>) -> Option<Key> {
        match (id, name) {
            (Some(id), _) => Some(Key::Id(id.clone())),
            (None, Some(n)) => Some(Key::Name(n.clone())),
            (None, None) => None,
        }
    }

    fn close(&mut self, key: &Key, at: i64) {
        if let Some((i, _)) = self.open.remove(key) {
            let iv = &mut self.intervals[i];
            iv.end = Some(at.max(iv.start));
        }
    }

    fn add_participant(&mut self, name: &str) {
        if self.participants.len() >= MAX_TIMELINE_PARTICIPANTS {
            return;
        }
        let k = name_key(name);
        if !k.is_empty() && self.participant_keys.insert(k) {
            self.participants.push(name.to_string());
        }
    }

    /// Whether events for `key` are taken: a known speaker, or a new one
    /// while there is room.
    fn admit(&mut self, key: &Key) -> bool {
        if self.keys.contains(key) {
            return true;
        }
        if self.keys.len() >= MAX_SPEAKER_KEYS {
            return false;
        }
        self.keys.insert(key.clone());
        true
    }

    /// The timeline has no information on lines that end before this (ms),
    /// because older intervals aged out; `None` while complete.
    pub fn forgotten_before(&self) -> Option<u64> {
        self.forgotten_before.map(|t| t.max(0) as u64)
    }

    /// Past [`MAX_INTERVALS`]: drop the older half of the finished
    /// intervals (by end), keep every open one.
    fn age_out(&mut self) {
        if self.intervals.len() <= MAX_INTERVALS {
            return;
        }
        let mut ends: Vec<i64> = self.intervals.iter().filter_map(|iv| iv.end).collect();
        if ends.is_empty() {
            return;
        }
        let k = ends.len() / 2;
        let (_, &mut horizon, _) = ends.select_nth_unstable(k);
        self.intervals.retain(|iv| iv.end.is_none_or(|e| e > horizon));
        self.forgotten_before = Some(self.forgotten_before.map_or(horizon, |f| f.max(horizon)));
        for (i, iv) in self.intervals.iter().enumerate() {
            if iv.end.is_none() {
                if let Some(slot) = self.open.get_mut(&iv.key) {
                    slot.0 = i;
                }
            }
        }
    }

    /// Add one page event (in the order received).
    pub fn push(&mut self, event: &MeetingEvent) {
        match event {
            MeetingEvent::SpeakerActive {
                t_ms,
                name,
                id,
                source,
                ..
            } => {
                let Some(key) = Self::key(id, name) else {
                    return;
                };
                if !self.admit(&key) {
                    return;
                }
                if let (Some(id), Some(n)) = (id, name) {
                    self.carried.insert(id.clone(), n.clone());
                }
                let at = *t_ms as i64 - self.params.lag(*source);
                if let Key::Name(_) = key {
                    // One indicator: the previous bare name stops here.
                    let others: Vec<Key> = self
                        .open
                        .keys()
                        .filter(|k| matches!(k, Key::Name(_)) && **k != key)
                        .cloned()
                        .collect();
                    for k in others {
                        let lag = self.params.lag(self.open[&k].1);
                        self.close(&k, *t_ms as i64 - lag);
                    }
                }
                if !self.open.contains_key(&key) {
                    self.intervals.push(Interval {
                        key: key.clone(),
                        start: at,
                        end: None,
                    });
                    self.open.insert(key, (self.intervals.len() - 1, *source));
                    self.age_out();
                }
            }
            MeetingEvent::SpeakerIdle { t_ms, name, id, .. } => {
                if let Some(key) = Self::key(id, name) {
                    if let Some(&(_, source)) = self.open.get(&key) {
                        self.close(&key, *t_ms as i64 - self.params.lag(source));
                    }
                }
            }
            MeetingEvent::SpeakerName { id, name, .. } => {
                if self.admit(&Key::Id(id.clone())) {
                    self.bound.insert(id.clone(), name.clone());
                }
            }
            MeetingEvent::Participants { names, .. } => {
                for n in names {
                    self.add_participant(n);
                }
            }
            MeetingEvent::ObserverHealth { .. } => {}
        }
    }

    fn name_of<'a>(&'a self, key: &'a Key) -> Option<&'a str> {
        match key {
            Key::Name(n) => Some(n),
            Key::Id(id) => self.id_name(id),
        }
    }

    fn id_name(&self, id: &str) -> Option<&str> {
        match self.bound.get(id) {
            Some(bound) => bound.as_deref(),
            None => self.carried.get(id).map(String::as_str),
        }
    }

    /// Who the line `[start_ms, end_ms)` is, per the page.
    pub fn attribute(&self, start_ms: u64, end_ms: u64) -> Attribution {
        let p = &self.params;
        let (mut s, mut e) = (start_ms as i64, end_ms.max(start_ms) as i64);
        let tol = p.tolerance_ms as i64;
        if e - s > 3 * tol {
            s += tol;
            e -= tol;
        }
        let len = e - s;
        if len <= 0 {
            return Attribution::Silent;
        }
        // Covered time per person: a name, or an id nobody named.
        #[derive(PartialEq)]
        enum Who<'a> {
            Name(String),
            Nameless(&'a Key),
        }
        let mut cover: Vec<(Who, i64)> = Vec::new();
        for iv in &self.intervals {
            let a = iv.start.max(s);
            let b = iv.end.unwrap_or(i64::MAX).min(e);
            if b <= a {
                continue;
            }
            let who = match self.name_of(&iv.key) {
                Some(n) => Who::Name(name_key(n)),
                None => Who::Nameless(&iv.key),
            };
            match cover.iter_mut().find(|(w, _)| *w == who) {
                Some(c) => c.1 += b - a,
                None => cover.push((who, b - a)),
            }
        }
        if cover.is_empty() {
            return Attribution::Silent;
        }
        cover.sort_by_key(|c| std::cmp::Reverse(c.1));
        let best = cover[0].1.min(len);
        let second = cover.get(1).map_or(0, |c| c.1.min(len));
        if best * 100 < len * p.min_share_pct as i64 || best * 100 < second * p.margin_pct as i64 {
            return Attribution::Ambiguous;
        }
        match &cover[0].0 {
            Who::Nameless(_) => Attribution::Unknown,
            Who::Name(k) => {
                // The display form: the first name with this key.
                let shown = self
                    .intervals
                    .iter()
                    .filter_map(|iv| self.name_of(&iv.key))
                    .find(|n| name_key(n) == *k)
                    .unwrap_or(k);
                Attribution::Named(shown.to_string())
            }
        }
    }

    /// Everyone the page listed or named, in order of first appearance.
    pub fn participants(&self) -> Vec<String> {
        let mut t = Self {
            participants: self.participants.clone(),
            participant_keys: self.participant_keys.clone(),
            ..Default::default()
        };
        let mut ids: Vec<&String> = self.bound.keys().chain(self.carried.keys()).collect();
        ids.sort();
        ids.dedup();
        for iv in &self.intervals {
            if let Some(n) = self.name_of(&iv.key) {
                t.add_participant(n);
            }
        }
        for id in ids {
            if let Some(n) = self.id_name(id) {
                t.add_participant(n);
            }
        }
        t.participants
    }
}

/// [`attribute`] as a free function over a list of events (tests, tools).
pub fn attribute(
    events: &[MeetingEvent],
    params: AttributionParams,
    start_ms: u64,
    end_ms: u64,
) -> Attribution {
    NameTimeline::from_events(params, events).attribute(start_ms, end_ms)
}

/// A timeline the `/live` connection writes and the engine reads.
#[derive(Clone, Default)]
pub struct SharedNames(Arc<Mutex<NameTimeline>>);

impl SharedNames {
    pub fn new(params: AttributionParams) -> Self {
        Self(Arc::new(Mutex::new(NameTimeline::new(params))))
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, NameTimeline> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn push(&self, event: &MeetingEvent) {
        self.lock().push(event);
    }

    pub fn attribute(&self, start_ms: u64, end_ms: u64) -> Attribution {
        self.lock().attribute(start_ms, end_ms)
    }

    pub fn participants(&self) -> Vec<String> {
        self.lock().participants()
    }

    pub fn is_empty(&self) -> bool {
        self.lock().is_empty()
    }

    pub fn forgotten_before(&self) -> Option<u64> {
        self.lock().forgotten_before()
    }
}

impl PartialEq for SharedNames {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl Eq for SharedNames {}

impl std::fmt::Debug for SharedNames {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SharedNames")
    }
}

/// Add the meeting's `names` to the item's participants (those not there
/// yet, by name), and give the new ones the email of the People person
/// they match (P5; an existing entry is never changed). Notes have no
/// participants.
pub fn merge_participants(meta: &mut ItemMeta, names: &[String], people: &[Person]) {
    if !meta.item_type.has_participants() || names.is_empty() {
        return;
    }
    let old = meta.participants.clone();
    for n in names {
        let k = name_key(n);
        if !k.is_empty() && !meta.participants.iter().any(|p| name_key(&p.name) == k) {
            meta.participants.push(Participant {
                name: n.clone(),
                email: None,
            });
        }
    }
    link_new_participants(&old, meta, people);
    meta.participants = normalize_participants(&meta.participants);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::archive::ItemType;

    fn params() -> AttributionParams {
        AttributionParams {
            dom_lag_ms: 400,
            caption_lag_ms: 1_500,
            tolerance_ms: 250,
            min_share_pct: 50,
            margin_pct: 150,
        }
    }

    fn active(t: u64, id: Option<&str>, name: Option<&str>, source: NameSource) -> MeetingEvent {
        MeetingEvent::SpeakerActive {
            at_ms: t,
            t_ms: t,
            name: name.map(str::to_string),
            id: id.map(str::to_string),
            source,
        }
    }

    fn idle(t: u64, id: &str) -> MeetingEvent {
        MeetingEvent::SpeakerIdle {
            at_ms: t,
            t_ms: t,
            name: None,
            id: Some(id.into()),
        }
    }

    fn bind(id: &str, name: Option<&str>) -> MeetingEvent {
        MeetingEvent::SpeakerName {
            at_ms: 0,
            id: id.into(),
            name: name.map(str::to_string),
        }
    }

    /// Protocol 1: a bare name per indicator change.
    fn dom(t: u64, name: &str) -> MeetingEvent {
        active(t, None, Some(name), NameSource::Dom)
    }

    fn rtp(t: u64, id: &str) -> MeetingEvent {
        active(t, Some(id), None, NameSource::Rtp)
    }

    fn named(n: &str) -> Attribution {
        Attribution::Named(n.into())
    }

    #[test]
    fn the_name_active_for_most_of_a_line_wins_with_lag_compensated() {
        // The indicator shows Anna from 1.4 s, Bo from 5.4 s: each came
        // 400 ms after the voice.
        let ev = [dom(1_400, "Anna"), dom(5_400, "Bo")];
        let at = |s, e| attribute(&ev, params(), s, e);
        assert_eq!(at(1_000, 4_900), named("Anna"));
        assert_eq!(at(5_000, 8_000), named("Bo"));
        // Without the lag compensation Anna's line would start before her
        // indicator and Bo's would lose ~0.4 s to Anna; with it both are
        // clean even as short lines (no edge trimming below 750 ms).
        assert_eq!(at(1_000, 1_700), named("Anna"));
        assert_eq!(at(5_000, 5_700), named("Bo"));
        // A line straddling the change goes to whoever covers most of it.
        assert_eq!(at(3_000, 6_000), named("Anna"));
        assert_eq!(at(4_000, 8_000), named("Bo"));
    }

    #[test]
    fn a_close_call_or_little_coverage_is_ambiguous() {
        // Anna until 5 s, Bo after (lag-compensated): a line 3–7 s is
        // split evenly.
        let ev = [dom(1_400, "Anna"), dom(5_400, "Bo")];
        assert_eq!(
            attribute(&ev, params(), 3_000, 7_000),
            Attribution::Ambiguous
        );
        // Two ids speaking over each other for the whole line.
        let ev = [
            bind("csrc:1", Some("Anna")),
            bind("csrc:2", Some("Bo")),
            rtp(1_000, "csrc:1"),
            rtp(1_000, "csrc:2"),
            idle(6_000, "csrc:1"),
            idle(6_000, "csrc:2"),
        ];
        assert_eq!(
            attribute(&ev, params(), 1_000, 6_000),
            Attribution::Ambiguous
        );
        // Anna covers only 1 s of a 5 s line (the rest silent on the page).
        let ev = [
            rtp(1_000, "csrc:1"),
            idle(2_250, "csrc:1"),
            bind("csrc:1", Some("Anna")),
        ];
        assert_eq!(
            attribute(&ev, params(), 1_000, 6_000),
            Attribution::Ambiguous
        );
    }

    #[test]
    fn a_short_interruption_does_not_take_the_line() {
        let ev = [
            bind("csrc:1", Some("Anna")),
            bind("csrc:2", Some("Bo")),
            rtp(0, "csrc:1"),
            rtp(3_000, "csrc:2"),
            idle(3_600, "csrc:2"),
            idle(8_000, "csrc:1"),
        ];
        assert_eq!(attribute(&ev, params(), 500, 7_000), named("Anna"));
    }

    #[test]
    fn silence_and_events_before_or_after_give_nothing() {
        assert_eq!(attribute(&[], params(), 0, 5_000), Attribution::Silent);
        let ev = [
            rtp(10_000, "csrc:1"),
            idle(12_000, "csrc:1"),
            bind("csrc:1", Some("Anna")),
        ];
        assert_eq!(attribute(&ev, params(), 0, 5_000), Attribution::Silent);
        assert_eq!(
            attribute(&ev, params(), 13_000, 15_000),
            Attribution::Silent
        );
        assert_eq!(attribute(&ev, params(), 10_000, 12_000), named("Anna"));
        // A zero-length line.
        assert_eq!(
            attribute(&ev, params(), 11_000, 11_000),
            Attribution::Silent
        );
    }

    #[test]
    fn unknown_ids_are_unknown_until_named_and_a_late_name_applies_to_the_whole_call() {
        let mut t = NameTimeline::new(params());
        t.push(&rtp(1_000, "csrc:7"));
        t.push(&idle(4_000, "csrc:7"));
        assert_eq!(t.attribute(1_000, 4_000), Attribution::Unknown);
        t.push(&rtp(9_000, "csrc:7"));
        // Named later: the earlier interval is the same person.
        t.push(&bind("csrc:7", Some("Carla")));
        assert_eq!(t.attribute(1_000, 4_000), named("Carla"));
        // Still open: active until now.
        assert_eq!(t.attribute(9_500, 12_000), named("Carla"));
        // The observer took the name back (contradicted): unknown again.
        t.push(&bind("csrc:7", None));
        assert_eq!(t.attribute(1_000, 4_000), Attribution::Unknown);
        // A name carried on speaker_active counts until a binding says otherwise.
        let ev = [active(0, Some("csrc:9"), Some("Dino"), NameSource::Rtp)];
        assert_eq!(attribute(&ev, params(), 0, 3_000), named("Dino"));
    }

    #[test]
    fn rtp_needs_no_lag_and_captions_lag_more() {
        // Voice 2.0–5.0 s. RTP sees it on time, the indicator 400 ms and
        // captions 1.5 s late; each timeline lines up after compensation.
        let rtp_ev = [
            bind("csrc:1", Some("Anna")),
            rtp(2_000, "csrc:1"),
            idle(5_000, "csrc:1"),
        ];
        let cap = [
            active(3_500, None, Some("Anna"), NameSource::Caption),
            MeetingEvent::SpeakerIdle {
                at_ms: 6_500,
                t_ms: 6_500,
                name: Some("Anna".into()),
                id: None,
            },
        ];
        for ev in [&rtp_ev[..], &cap[..]] {
            assert_eq!(attribute(ev, params(), 2_000, 5_000), named("Anna"));
            assert_eq!(attribute(ev, params(), 5_200, 6_400), Attribution::Silent);
        }
    }

    #[test]
    fn a_rejoin_with_a_new_id_is_the_same_name() {
        let ev = [
            bind("csrc:1", Some("Anna")),
            bind("csrc:5", Some("anna ")),
            rtp(0, "csrc:1"),
            idle(2_000, "csrc:1"),
            rtp(2_000, "csrc:5"),
            idle(4_000, "csrc:5"),
        ];
        assert_eq!(attribute(&ev, params(), 0, 4_000), named("Anna"));
    }

    #[test]
    fn participants_are_listed_and_named_speakers_added() {
        let mut t = NameTimeline::new(params());
        t.push(&MeetingEvent::Participants {
            at_ms: 0,
            names: vec!["Anna".into(), "Bo".into()],
        });
        t.push(&MeetingEvent::Participants {
            at_ms: 10,
            names: vec!["bo".into(), "Carla".into()],
        });
        t.push(&dom(100, "Dino"));
        t.push(&bind("csrc:3", Some("Eva")));
        t.push(&bind("csrc:4", None));
        assert_eq!(t.participants(), ["Anna", "Bo", "Carla", "Dino", "Eva"]);
        assert!(!t.is_empty());
        assert!(NameTimeline::default().is_empty());
    }

    #[test]
    fn a_page_cannot_grow_the_timeline_without_bound() {
        let mut t = NameTimeline::new(params());
        // New speakers past the cap are ignored; known ones still count.
        for i in 0..MAX_SPEAKER_KEYS + 50 {
            t.push(&bind(&format!("csrc:{i}"), Some(&format!("P{i}"))));
        }
        assert_eq!(t.keys.len(), MAX_SPEAKER_KEYS);
        assert_eq!(t.bound.len(), MAX_SPEAKER_KEYS);
        t.push(&rtp(10, &format!("csrc:{}", MAX_SPEAKER_KEYS + 1)));
        assert!(t.intervals.is_empty(), "a new id past the cap opens nothing");
        t.push(&rtp(10, "csrc:0"));
        assert_eq!(t.intervals.len(), 1);
        // Participants: capped, deduplicated by key.
        let many: Vec<String> = (0..2 * MAX_TIMELINE_PARTICIPANTS).map(|i| format!("N{i}")).collect();
        t.push(&MeetingEvent::Participants { at_ms: 0, names: many.clone() });
        t.push(&MeetingEvent::Participants { at_ms: 0, names: many });
        assert_eq!(t.participants.len(), MAX_TIMELINE_PARTICIPANTS);
        assert_eq!(t.participants().len(), MAX_TIMELINE_PARTICIPANTS);
    }

    #[test]
    fn old_intervals_age_out_and_the_horizon_is_reported() {
        let mut t = NameTimeline::new(params());
        t.push(&rtp(0, "csrc:open")); // never idles: must survive
        t.push(&bind("csrc:a", Some("Anna")));
        assert_eq!(t.forgotten_before(), None);
        let mut ms = 0;
        while t.forgotten_before().is_none() {
            t.push(&rtp(ms, "csrc:a"));
            t.push(&idle(ms + 500, "csrc:a"));
            ms += 1_000;
        }
        assert!(t.intervals.len() <= MAX_INTERVALS);
        let horizon = t.forgotten_before().unwrap();
        assert!(horizon > 0 && horizon < ms);
        // Recent lines still resolve; the open interval kept its index.
        let last = (ms - 1_000) as i64;
        assert!(t.intervals.iter().any(|iv| iv.start == last && iv.end == Some(last + 500)));
        t.push(&idle(ms, "csrc:open"));
        assert!(t.open.is_empty());
        let open = t.intervals.iter().find(|iv| iv.key == Key::Id("csrc:open".into())).unwrap();
        assert_eq!((open.start, open.end), (0, Some(ms as i64)));
        // Growth goes on bounded.
        for _ in 0..MAX_INTERVALS {
            t.push(&rtp(ms, "csrc:a"));
            t.push(&idle(ms + 500, "csrc:a"));
            ms += 1_000;
        }
        assert!(t.intervals.len() <= MAX_INTERVALS);
        assert!(t.forgotten_before().unwrap() > horizon);
    }

    #[test]
    fn shared_names_compare_by_identity() {
        let a = SharedNames::new(params());
        let b = a.clone();
        assert_eq!(a, b);
        assert_ne!(a, SharedNames::new(params()));
        b.push(&dom(0, "Anna"));
        assert_eq!(a.attribute(0, 2_000), named("Anna"));
    }

    #[test]
    fn participants_merge_into_the_frontmatter_with_emails_from_people() {
        let people = vec![Person {
            id: "p1".into(),
            name: "Anna Rossi".into(),
            email: Some("anna@example.com".into()),
            aliases: vec![],
        }];
        let mut meta = ItemMeta {
            item_type: ItemType::Meeting,
            participants: vec![Participant {
                name: "Bo".into(),
                email: None,
            }],
            ..Default::default()
        };
        merge_participants(
            &mut meta,
            &["anna rossi".into(), "bo".into(), " ".into()],
            &people,
        );
        let got: Vec<(&str, Option<&str>)> = meta
            .participants
            .iter()
            .map(|p| (p.name.as_str(), p.email.as_deref()))
            .collect();
        assert_eq!(
            got,
            [("Bo", None), ("anna rossi", Some("anna@example.com"))]
        );
        let mut note = ItemMeta {
            item_type: ItemType::Note,
            ..Default::default()
        };
        merge_participants(&mut note, &["Anna".into()], &people);
        assert!(note.participants.is_empty());
    }
}
