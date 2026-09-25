//! Voice profiles (0.11, #241; plan P12, P13, E13): what Sussurro knows of
//! the voice of a person of the People registry, and how a document voice
//! is matched against it. Pure — the files live in [`super::voices`].
//!
//! **Enrolment (P12).** Only for a person with *Recognise this voice* on,
//! and only from **confirmed lines**: the lines of the document speakers
//! the user linked to that person (`DocSpeaker::person_id`, set only by
//! `SpeakerEdit::Link`). A line the user moved to another speaker is no
//! longer that speaker's line, so it drops out on its own. Each line's
//! stored WeSpeaker embedding (`Segment.embedding`, the mean of its ≤ 3 s
//! windows) counts with its duration. Lines flagged as overlapping speech
//! will be left out too once segments carry that flag (#244).
//!
//! **Shape (E13, spike V0-1 #235).** One pooled centroid per person — the
//! duration-weighted mean of every confirmed line, whatever the recording
//! condition, L2-normalised — plus the seconds per condition and the ids
//! of the documents the lines came from. Per-condition centroids were
//! measured and dropped: they never raised recall and, scored as the best
//! of several, they raised wrong suggestions on room audio.
//!
//! **Minimum (P12).** A profile makes suggestions only with at least
//! [`MIN_PROFILE_SPEECH_MS`] of confirmed speech from at least
//! [`MIN_PROFILE_DOCUMENTS`] documents. The spike measured 30 s from 2
//! documents almost as good; 60 s stays until the maintainer changes it.
//!
//! **Matching (E13).** A document voice (the mean of its lines, as for
//! "Voice N") is scored by cosine against every ready profile. The best
//! one is suggested when its cosine is at least [`SUGGEST_THRESHOLD`] and
//! it leads the runner-up by at least [`SUGGEST_MARGIN`]. When the voice
//! was recorded by a room mic **and** the profile holds room-mic lines,
//! the threshold is [`ROOM_THRESHOLD`]: room-to-room pairs share the
//! room's echo, which lifts the scores of different people. A match is
//! only ever a suggestion — nothing is linked without a click.
//!
//! **Privacy (P13).** Voiceprints linked to a person are biometric data
//! (GDPR art. 9): the vectors stay in `<app data>/voices/`, never reach the
//! UI, the HTTP API, diagnostics, the config export or a log. `Debug` on
//! the types here prints sizes, not vectors.

use super::cluster::{dot, l2_normalize};
use super::model::EMBEDDING_DIM;
use crate::archive::{Channel, Segment, SegmentsFile};
use serde::{Deserialize, Serialize};

/// Cosine a document voice needs with a profile to be suggested (E13).
pub const SUGGEST_THRESHOLD: f32 = 0.65;
/// The threshold when the voice comes from a room mic and the profile has
/// room-mic lines (E13): on the spike's simulated rooms 0.65 gave 5.9 %
/// wrong suggestions room-to-room, 0.75 gave 0.1 % at 94 % recall.
pub const ROOM_THRESHOLD: f32 = 0.75;
/// How far the best profile must lead the runner-up (E13).
pub const SUGGEST_MARGIN: f32 = 0.05;
/// Confirmed speech a profile needs before it makes suggestions (P12).
pub const MIN_PROFILE_SPEECH_MS: u64 = 60_000;
/// Documents the confirmed speech must come from (P12).
pub const MIN_PROFILE_DOCUMENTS: usize = 2;
/// `version` of a profile file.
pub const PROFILE_VERSION: u32 = 1;
/// The embedding model profiles are built from (E8, E13).
pub const PROFILE_MODEL: &str = "wespeaker-resnet34-lm";

/// How a line was recorded, as far as matching cares (E13).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Condition {
    /// A microphone close to the speaker: a remote participant's own mic
    /// (the remote or system-audio channel), or the user's mic in a
    /// two-channel session.
    Close,
    /// A microphone in the room picking up several people (a meeting in
    /// the room), or audio whose acoustics are unknown (a file) — the
    /// stricter rule.
    Room,
}

/// The condition of a line on `channel` in an item whose frontmatter
/// `source` is `source`.
pub fn line_condition(channel: Channel, source: &str) -> Condition {
    match channel {
        Channel::Remote | Channel::System => Condition::Close,
        // In a browser meeting or a system-audio recording the mic is the
        // user's own; anywhere else it is a mic in the room.
        Channel::Mic if source.starts_with("browser:") || source == "system" => Condition::Close,
        Channel::Mic | Channel::File => Condition::Room,
    }
}

fn duration_ms(s: &Segment) -> u64 {
    s.end_ms.saturating_sub(s.start_ms)
}

/// A line's embedding, L2-normalised, when it is usable: the model's size,
/// finite, not zero, and a line with some duration.
fn line_embedding(s: &Segment) -> Option<Vec<f32>> {
    let e = s.embedding.as_ref()?;
    if e.len() != EMBEDDING_DIM || duration_ms(s) == 0 || !e.iter().all(|x| x.is_finite()) {
        return None;
    }
    if e.iter().all(|x| *x == 0.0) {
        return None;
    }
    Some(l2_normalize(e.clone()))
}

/// Duration-weighted sums of L2-normalised line embeddings, per condition.
#[derive(Clone, PartialEq, Default)]
pub struct VoiceLines {
    /// Σ duration × line embedding (f64: the order-independent part of a
    /// rebuild's determinism is the sorted document order, this keeps the
    /// rounding small).
    sum: Vec<f64>,
    pub speech_ms: u64,
    pub close_ms: u64,
    pub room_ms: u64,
}

impl std::fmt::Debug for VoiceLines {
    // Never prints the vector (P13).
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VoiceLines")
            .field("dims", &self.sum.len())
            .field("speech_ms", &self.speech_ms)
            .field("close_ms", &self.close_ms)
            .field("room_ms", &self.room_ms)
            .finish()
    }
}

impl VoiceLines {
    fn add(&mut self, e: &[f32], ms: u64, condition: Condition) {
        if self.sum.is_empty() {
            self.sum = vec![0.0; e.len()];
        }
        let w = ms as f64;
        self.sum
            .iter_mut()
            .zip(e)
            .for_each(|(a, b)| *a += w * f64::from(*b));
        self.speech_ms += ms;
        match condition {
            Condition::Close => self.close_ms += ms,
            Condition::Room => self.room_ms += ms,
        }
    }

    fn merge(&mut self, other: &VoiceLines) {
        if other.sum.is_empty() {
            return;
        }
        if self.sum.is_empty() {
            self.sum = vec![0.0; other.sum.len()];
        }
        self.sum
            .iter_mut()
            .zip(&other.sum)
            .for_each(|(a, b)| *a += b);
        self.speech_ms += other.speech_ms;
        self.close_ms += other.close_ms;
        self.room_ms += other.room_ms;
    }

    pub fn is_empty(&self) -> bool {
        self.speech_ms == 0
    }

    /// The L2-normalised mean; empty when there are no lines.
    fn centroid(&self) -> Vec<f32> {
        if self.is_empty() {
            return Vec::new();
        }
        l2_normalize(self.sum.iter().map(|x| *x as f32).collect())
    }

    /// The condition that carries more speech; a tie counts as room (the
    /// stricter rule).
    fn condition(&self) -> Condition {
        if self.close_ms > self.room_ms {
            Condition::Close
        } else {
            Condition::Room
        }
    }
}

fn lines_where(file: &SegmentsFile, source: &str, keep: impl Fn(&str) -> bool) -> VoiceLines {
    let mut out = VoiceLines::default();
    for s in &file.segments {
        if !s.speaker_id.as_deref().is_some_and(&keep) {
            continue;
        }
        if let Some(e) = line_embedding(s) {
            out.add(&e, duration_ms(s), line_condition(s.channel, source));
        }
    }
    out
}

/// Does any speaker of the document link to `person_id`?
pub fn links_person(file: &SegmentsFile, person_id: &str) -> bool {
    file.speakers
        .iter()
        .any(|s| s.person_id.as_deref() == Some(person_id))
}

/// The confirmed lines of `person_id` in one document (`source` is the
/// item's frontmatter `source`): every usable line of every speaker linked
/// to them. Empty when there are none.
pub fn person_lines(file: &SegmentsFile, source: &str, person_id: &str) -> VoiceLines {
    let ids: Vec<&str> = file
        .speakers
        .iter()
        .filter(|s| s.person_id.as_deref() == Some(person_id))
        .map(|s| s.id.as_str())
        .collect();
    if ids.is_empty() {
        return VoiceLines::default();
    }
    lines_where(file, source, |id| ids.contains(&id))
}

/// A voice of one document as matching sees it (the query of
/// [`suggest`]).
#[derive(Clone, PartialEq)]
pub struct DocVoice {
    /// L2-normalised duration-weighted mean of the voice's lines.
    pub embedding: Vec<f32>,
    pub condition: Condition,
    pub speech_ms: u64,
}

impl std::fmt::Debug for DocVoice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DocVoice")
            .field("dims", &self.embedding.len())
            .field("condition", &self.condition)
            .field("speech_ms", &self.speech_ms)
            .finish()
    }
}

/// The voice of document speaker `speaker_id`: the mean of its lines
/// carrying an embedding, with the condition most of its speech was
/// recorded in. `None` when none of its lines has voice data.
pub fn document_voice(file: &SegmentsFile, source: &str, speaker_id: &str) -> Option<DocVoice> {
    let lines = lines_where(file, source, |id| id == speaker_id);
    (!lines.is_empty()).then(|| DocVoice {
        embedding: lines.centroid(),
        condition: lines.condition(),
        speech_ms: lines.speech_ms,
    })
}

/// `<app data>/voices/<person id>.json`. Its existence means *Recognise
/// this voice* is on for that person; turning it off deletes it.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct VoiceProfile {
    pub version: u32,
    pub person_id: String,
    pub model: String,
    /// One pooled centroid, L2-normalised; empty while there are no
    /// confirmed lines.
    #[serde(default)]
    pub centroid: Vec<f32>,
    /// Confirmed speech, in total and per condition.
    #[serde(default)]
    pub speech_ms: u64,
    #[serde(default)]
    pub close_ms: u64,
    #[serde(default)]
    pub room_ms: u64,
    /// Ids of the documents the lines came from, sorted (ids only).
    #[serde(default)]
    pub documents: Vec<String>,
    /// RFC 3339 time of the last build.
    #[serde(default)]
    pub updated: String,
}

impl std::fmt::Debug for VoiceProfile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VoiceProfile")
            .field("person_id", &self.person_id)
            .field("dims", &self.centroid.len())
            .field("speech_ms", &self.speech_ms)
            .field("close_ms", &self.close_ms)
            .field("room_ms", &self.room_ms)
            .field("documents", &self.documents.len())
            .finish()
    }
}

impl VoiceProfile {
    /// Enough confirmed speech from enough documents to make suggestions
    /// (P12), from the current model.
    pub fn ready(&self) -> bool {
        self.model == PROFILE_MODEL
            && self.centroid.len() == EMBEDDING_DIM
            && self.speech_ms >= MIN_PROFILE_SPEECH_MS
            && self.documents.len() >= MIN_PROFILE_DOCUMENTS
    }

    /// Same content, whenever it was built.
    pub fn same_as(&self, other: &VoiceProfile) -> bool {
        VoiceProfile {
            updated: String::new(),
            ..self.clone()
        } == VoiceProfile {
            updated: String::new(),
            ..other.clone()
        }
    }
}

/// Build `person_id`'s profile from each document's confirmed lines
/// (`(document id, lines)`; documents without lines are left out). The
/// result depends only on the set of documents: they are summed in id
/// order.
pub fn build_profile(
    person_id: &str,
    docs: &[(String, VoiceLines)],
    updated: &str,
) -> VoiceProfile {
    let mut docs: Vec<&(String, VoiceLines)> = docs.iter().filter(|(_, l)| !l.is_empty()).collect();
    docs.sort_by(|a, b| a.0.cmp(&b.0));
    docs.dedup_by(|a, b| a.0 == b.0);
    let mut all = VoiceLines::default();
    for (_, lines) in &docs {
        all.merge(lines);
    }
    VoiceProfile {
        version: PROFILE_VERSION,
        person_id: person_id.to_string(),
        model: PROFILE_MODEL.to_string(),
        centroid: all.centroid(),
        speech_ms: all.speech_ms,
        close_ms: all.close_ms,
        room_ms: all.room_ms,
        documents: docs.iter().map(|(id, _)| id.clone()).collect(),
        updated: updated.to_string(),
    }
}

/// A person a document voice sounds like.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Suggestion {
    pub person_id: String,
    /// Cosine of the voice with the person's profile.
    pub score: f32,
    /// Lead over the runner-up profile (a missing runner-up counts as 0).
    pub margin: f32,
}

/// The threshold the best profile must reach for a voice in `condition`.
pub fn threshold_for(profile: &VoiceProfile, condition: Condition) -> f32 {
    if condition == Condition::Room && profile.room_ms > 0 {
        ROOM_THRESHOLD
    } else {
        SUGGEST_THRESHOLD
    }
}

/// Who a document voice sounds like, if anyone (E13): the best-scoring
/// ready profile, when it reaches its threshold ([`threshold_for`]) and
/// leads the runner-up by [`SUGGEST_MARGIN`]. Only the best profile is
/// ever suggested — a runner-up passing a lower threshold is not.
/// Profiles that are not [`VoiceProfile::ready`] take no part.
pub fn suggest(
    profiles: &[VoiceProfile],
    voice: &[f32],
    condition: Condition,
) -> Option<Suggestion> {
    if voice.len() != EMBEDDING_DIM || !voice.iter().all(|x| x.is_finite()) {
        return None;
    }
    let v = l2_normalize(voice.to_vec());
    if v.iter().all(|x| *x == 0.0) {
        return None;
    }
    let mut scored: Vec<(f32, &VoiceProfile)> = profiles
        .iter()
        .filter(|p| p.ready())
        .map(|p| (dot(&p.centroid, &v), p))
        .collect();
    // Highest first; ties by person id so the answer never depends on the
    // order the profiles were read in.
    scored.sort_by(|a, b| {
        b.0.total_cmp(&a.0)
            .then_with(|| a.1.person_id.cmp(&b.1.person_id))
    });
    let (score, best) = *scored.first()?;
    let runner_up = scored.get(1).map_or(0.0, |s| s.0);
    let margin = score - runner_up;
    (score >= threshold_for(best, condition) && margin >= SUGGEST_MARGIN).then(|| Suggestion {
        person_id: best.person_id.clone(),
        score,
        margin,
    })
}

/// [`suggest`] for document speaker `speaker_id` ([`document_voice`]).
pub fn suggest_for_speaker(
    profiles: &[VoiceProfile],
    file: &SegmentsFile,
    source: &str,
    speaker_id: &str,
) -> Option<Suggestion> {
    let voice = document_voice(file, source, speaker_id)?;
    suggest(profiles, &voice.embedding, voice.condition)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::archive::DocSpeaker;

    /// A unit vector along axis `k` with a little of axis `k + 1`, so
    /// different `k` are (nearly) orthogonal voices.
    pub(crate) fn axis(k: usize) -> Vec<f32> {
        let mut v = vec![0.0; EMBEDDING_DIM];
        v[k % EMBEDDING_DIM] = 1.0;
        v
    }

    /// `a` rotated towards `b` so that cos(result, a) = `cos`.
    pub(crate) fn towards(a: &[f32], b: &[f32], cos: f32) -> Vec<f32> {
        let sin = (1.0 - cos * cos).sqrt();
        a.iter().zip(b).map(|(x, y)| cos * x + sin * y).collect()
    }

    pub(crate) fn line(
        id: u32,
        speaker: &str,
        ms: u64,
        emb: Vec<f32>,
        channel: Channel,
    ) -> Segment {
        Segment {
            id,
            channel,
            start_ms: u64::from(id) * 100_000,
            end_ms: u64::from(id) * 100_000 + ms,
            speaker_id: Some(speaker.to_string()),
            embedding: Some(emb),
            ..Default::default()
        }
    }

    pub(crate) fn speaker(id: &str, person: Option<&str>) -> DocSpeaker {
        DocSpeaker {
            id: id.to_string(),
            label: id.to_string(),
            person_id: person.map(str::to_string),
            ..Default::default()
        }
    }

    fn doc(lines: Vec<Segment>, speakers: Vec<DocSpeaker>) -> SegmentsFile {
        SegmentsFile {
            speakers,
            segments: lines,
            ..Default::default()
        }
    }

    fn profile(person: &str, centroid: Vec<f32>, room_ms: u64) -> VoiceProfile {
        VoiceProfile {
            version: PROFILE_VERSION,
            person_id: person.to_string(),
            model: PROFILE_MODEL.to_string(),
            centroid: l2_normalize(centroid),
            speech_ms: 60_000,
            close_ms: 60_000 - room_ms,
            room_ms,
            documents: vec!["a".into(), "b".into()],
            updated: String::new(),
        }
    }

    #[test]
    fn conditions_follow_channel_and_source() {
        assert_eq!(
            line_condition(Channel::Remote, "browser:meet.google.com"),
            Condition::Close
        );
        assert_eq!(line_condition(Channel::System, "system"), Condition::Close);
        assert_eq!(
            line_condition(Channel::Mic, "browser:meet.google.com"),
            Condition::Close
        );
        assert_eq!(line_condition(Channel::Mic, "system"), Condition::Close);
        assert_eq!(line_condition(Channel::Mic, "mic"), Condition::Room);
        assert_eq!(
            line_condition(Channel::File, "file:talk.mp3"),
            Condition::Room
        );
    }

    #[test]
    fn person_lines_take_only_linked_speakers_and_usable_embeddings() {
        let f = doc(
            vec![
                line(1, "voice:1", 10_000, axis(0), Channel::Remote),
                line(2, "voice:2", 10_000, axis(1), Channel::Remote),
                // Linked, but no embedding / wrong size / zero / no duration.
                Segment {
                    embedding: None,
                    ..line(3, "voice:1", 5_000, axis(0), Channel::Remote)
                },
                line(4, "voice:1", 5_000, vec![1.0; 3], Channel::Remote),
                line(
                    5,
                    "voice:1",
                    5_000,
                    vec![0.0; EMBEDDING_DIM],
                    Channel::Remote,
                ),
                line(6, "voice:1", 0, axis(0), Channel::Remote),
                line(
                    7,
                    "voice:1",
                    5_000,
                    vec![f32::NAN; EMBEDDING_DIM],
                    Channel::Remote,
                ),
                // A line moved away from the linked speaker is not theirs.
                line(8, "voice:2", 20_000, axis(0), Channel::Remote),
                line(9, "voice:3", 4_000, axis(0), Channel::Mic),
            ],
            vec![
                speaker("voice:1", Some("p-anna")),
                speaker("voice:2", None),
                speaker("voice:3", Some("p-anna")),
            ],
        );
        let l = person_lines(&f, "browser:meet.google.com", "p-anna");
        assert_eq!(l.speech_ms, 14_000);
        assert_eq!(l.close_ms, 14_000, "mic of a browser meeting is close");
        assert!(person_lines(&f, "mic", "p-bob").is_empty());
        let room = person_lines(&f, "mic", "p-anna");
        assert_eq!((room.close_ms, room.room_ms), (10_000, 4_000));
        assert!(links_person(&f, "p-anna") && !links_person(&f, "p-bob"));
    }

    #[test]
    fn centroid_is_the_duration_weighted_mean_of_normalised_lines() {
        // Two lines: 30 s on axis 0, 10 s on axis 1 (the second one not
        // normalised: it must count as a unit vector).
        let f = doc(
            vec![
                line(1, "voice:1", 30_000, axis(0), Channel::Remote),
                line(
                    2,
                    "voice:1",
                    10_000,
                    axis(1).iter().map(|x| x * 5.0).collect(),
                    Channel::Remote,
                ),
            ],
            vec![speaker("voice:1", Some("p-anna"))],
        );
        let p = build_profile(
            "p-anna",
            &[("d1".into(), person_lines(&f, "system", "p-anna"))],
            "t",
        );
        let n = (0.75f32 * 0.75 + 0.25 * 0.25).sqrt();
        assert!((p.centroid[0] - 0.75 / n).abs() < 1e-6);
        assert!((p.centroid[1] - 0.25 / n).abs() < 1e-6);
        assert_eq!(p.speech_ms, 40_000);
        assert_eq!(p.documents, vec!["d1".to_string()]);
    }

    #[test]
    fn minimums_need_sixty_seconds_from_two_documents() {
        let lines = |ms: u64| {
            let f = doc(
                vec![line(1, "v", ms, axis(0), Channel::Remote)],
                vec![speaker("v", Some("p"))],
            );
            person_lines(&f, "system", "p")
        };
        // 60 s from one document: not ready.
        let one = build_profile("p", &[("a".into(), lines(60_000))], "t");
        assert!(!one.ready());
        // 59.999 s from two documents: not ready.
        let short = build_profile(
            "p",
            &[("a".into(), lines(30_000)), ("b".into(), lines(29_999))],
            "t",
        );
        assert!(!short.ready());
        // 60 s from two documents: ready.
        let ok = build_profile(
            "p",
            &[("a".into(), lines(30_000)), ("b".into(), lines(30_000))],
            "t",
        );
        assert!(ok.ready());
        assert_eq!(ok.documents.len(), 2);
        // A document without lines doesn't count as a document.
        let empty = build_profile(
            "p",
            &[
                ("a".into(), lines(60_000)),
                ("b".into(), VoiceLines::default()),
            ],
            "t",
        );
        assert!(!empty.ready());
        assert_eq!(empty.documents, vec!["a".to_string()]);
        // Another model's profile never suggests.
        let old = VoiceProfile {
            model: "other".into(),
            ..ok.clone()
        };
        assert!(!old.ready());
    }

    #[test]
    fn build_is_deterministic_whatever_the_document_order() {
        let mk = |k: usize, ms: u64| {
            let f = doc(
                vec![line(
                    1,
                    "v",
                    ms,
                    towards(&axis(0), &axis(k), 0.8),
                    Channel::Remote,
                )],
                vec![speaker("v", Some("p"))],
            );
            person_lines(&f, "system", "p")
        };
        let docs: Vec<(String, VoiceLines)> = (1..6)
            .map(|k| (format!("2026/09/d{k}"), mk(k, 7_000 * k as u64)))
            .collect();
        let a = build_profile("p", &docs, "t1");
        let mut rev = docs.clone();
        rev.reverse();
        let b = build_profile("p", &rev, "t2");
        assert!(a.same_as(&b));
        assert_eq!(a.centroid, b.centroid, "bit-identical");
        assert_ne!(a, b, "only `updated` differs");
    }

    #[test]
    fn suggests_above_threshold_and_margin_only() {
        let anna = profile("p-anna", axis(0), 0);
        let bob = profile("p-bob", axis(1), 0);
        let profiles = [anna.clone(), bob.clone()];
        // Clearly Anna.
        let s = suggest(
            &profiles,
            &towards(&axis(0), &axis(2), 0.8),
            Condition::Close,
        )
        .unwrap();
        assert_eq!(s.person_id, "p-anna");
        assert!((s.score - 0.8).abs() < 1e-5);
        assert!((s.margin - 0.8).abs() < 1e-5, "Bob scores 0");
        // Just below the threshold: nothing.
        assert!(suggest(
            &profiles,
            &towards(&axis(0), &axis(2), 0.64),
            Condition::Close
        )
        .is_none());
        // At the threshold: suggested.
        assert!(suggest(
            &profiles,
            &towards(&axis(0), &axis(2), 0.651),
            Condition::Close
        )
        .is_some());
        // Between Anna and Bob: both high, margin too small.
        let mid: Vec<f32> = axis(0)
            .iter()
            .zip(axis(1))
            .map(|(a, b)| a + b * 0.95)
            .collect();
        let r = suggest(&profiles, &mid, Condition::Close);
        assert!(r.is_none(), "{r:?}");
        // Same voice with a bigger lead passes.
        let lead: Vec<f32> = axis(0)
            .iter()
            .zip(axis(1))
            .map(|(a, b)| a + b * 0.8)
            .collect();
        assert_eq!(
            suggest(&profiles, &lead, Condition::Close)
                .unwrap()
                .person_id,
            "p-anna"
        );
        // Profiles not ready take no part (Bob alone would not be suggested).
        let not_ready = VoiceProfile {
            speech_ms: 59_000,
            ..anna.clone()
        };
        assert!(suggest(&[not_ready], &axis(0), Condition::Close).is_none());
        // Nobody, a bad query.
        assert!(suggest(&[], &axis(0), Condition::Close).is_none());
        assert!(suggest(&profiles, &[1.0, 0.0], Condition::Close).is_none());
        assert!(suggest(&profiles, &vec![0.0; EMBEDDING_DIM], Condition::Close).is_none());
    }

    #[test]
    fn room_voice_against_room_profile_needs_the_higher_threshold() {
        let with_room = profile("p-anna", axis(0), 20_000);
        let close_only = profile("p-anna", axis(0), 0);
        let v = towards(&axis(0), &axis(2), 0.70);
        // Room voice, profile with room lines: 0.70 < 0.75.
        assert!(suggest(std::slice::from_ref(&with_room), &v, Condition::Room).is_none());
        // Room voice, close-only profile: 0.65 applies.
        assert!(suggest(std::slice::from_ref(&close_only), &v, Condition::Room).is_some());
        // Close voice, profile with room lines: 0.65 applies.
        assert!(suggest(std::slice::from_ref(&with_room), &v, Condition::Close).is_some());
        // Room to room above 0.75.
        let strong = towards(&axis(0), &axis(2), 0.76);
        assert!(suggest(std::slice::from_ref(&with_room), &strong, Condition::Room).is_some());
        assert_eq!(threshold_for(&with_room, Condition::Room), ROOM_THRESHOLD);
        assert_eq!(
            threshold_for(&close_only, Condition::Room),
            SUGGEST_THRESHOLD
        );
    }

    #[test]
    fn only_the_best_profile_is_ever_suggested() {
        // Best: Anna with room lines at 0.72 (fails 0.75); runner-up Bob,
        // close only, at 0.66 would pass 0.65 — still no suggestion.
        let anna = profile("p-anna", axis(0), 10_000);
        let bob = profile("p-bob", axis(1), 0);
        let mut v = vec![0.0; EMBEDDING_DIM];
        v[0] = 0.72;
        v[1] = 0.66;
        v[2] = (1.0f32 - 0.72 * 0.72 - 0.66 * 0.66).sqrt();
        assert!(suggest(&[anna, bob], &v, Condition::Room).is_none());
    }

    #[test]
    fn ties_break_on_person_id_not_on_input_order() {
        let a = profile("p-a", axis(0), 0);
        let b = profile("p-b", axis(0), 0);
        // Identical profiles: margin 0, so nothing — whatever the order.
        assert!(suggest(&[a.clone(), b.clone()], &axis(0), Condition::Close).is_none());
        assert!(suggest(&[b, a], &axis(0), Condition::Close).is_none());
    }

    #[test]
    fn document_voice_and_speaker_suggestion() {
        let f = doc(
            vec![
                line(
                    1,
                    "voice:1",
                    8_000,
                    towards(&axis(0), &axis(3), 0.9),
                    Channel::Mic,
                ),
                line(
                    2,
                    "voice:1",
                    8_000,
                    towards(&axis(0), &axis(4), 0.9),
                    Channel::Mic,
                ),
                line(3, "voice:2", 8_000, axis(1), Channel::Mic),
            ],
            vec![speaker("voice:1", None), speaker("voice:2", None)],
        );
        let v = document_voice(&f, "mic", "voice:1").unwrap();
        assert_eq!(v.condition, Condition::Room);
        assert_eq!(v.speech_ms, 16_000);
        assert!(document_voice(&f, "mic", "voice:9").is_none());
        let anna = profile("p-anna", axis(0), 0);
        let s = suggest_for_speaker(std::slice::from_ref(&anna), &f, "mic", "voice:1").unwrap();
        assert_eq!(s.person_id, "p-anna");
        assert!(suggest_for_speaker(&[anna], &f, "mic", "voice:2").is_none());
    }

    #[test]
    fn debug_never_prints_vectors() {
        let p = profile("p-anna", towards(&axis(0), &axis(1), 0.123_456_7), 0);
        let text = format!("{p:?}");
        assert!(text.contains("p-anna") && text.contains("dims"));
        assert!(!text.contains("0.12"), "{text}");
        let f = doc(
            vec![line(1, "v", 5_000, axis(0), Channel::Remote)],
            vec![speaker("v", Some("p"))],
        );
        assert!(!format!("{:?}", person_lines(&f, "system", "p")).contains("1.0"));
        assert!(!format!("{:?}", document_voice(&f, "system", "v").unwrap()).contains("1.0"));
    }

    #[test]
    fn profile_file_roundtrips_exactly() {
        let p = profile("p-anna", towards(&axis(0), &axis(1), 0.3), 5_000);
        let json = serde_json::to_string(&p).unwrap();
        let back: VoiceProfile = serde_json::from_str(&json).unwrap();
        assert_eq!(back, p);
    }
}
