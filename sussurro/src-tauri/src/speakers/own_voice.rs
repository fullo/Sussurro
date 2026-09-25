//! The user's own voice, "You" (0.11, #243; plan P14, spike V0-1 #235).
//!
//! **Enrolment.** Optional: the user reads a short paragraph shown on
//! screen (about [`TARGET_ENROL_MS`], from the mic they pick). The quiet
//! parts are dropped ([`voiced`]), the speech is cut into the same ≤ 3 s
//! windows as a live session, each window is embedded with WeSpeaker, and
//! the profile keeps one L2-normalised, duration-weighted centroid. It
//! needs at least [`MIN_ENROL_SPEECH_MS`] of speech; audio past
//! [`MAX_ENROL_MS`] is not used. The spike found 30 s enough (10 s already
//! scores almost the same). Re-enrolling replaces the profile; *Forget my
//! voice* deletes it.
//!
//! **Storage (P13).** `<app data>/voices/you.own-voice.json`, next to the
//! People profiles of #241 and under the same rules: the folder is 0700,
//! the file 0600 and written atomically, never in the archive, the HTTP
//! API, exports or the UI (the UI gets [`OwnVoiceStatus`], no vectors).
//! The name has a dot in it, so it can never be taken for a person's
//! profile (person ids are `[A-Za-z0-9_-]`): #241's scans skip it and
//! *Forget all voices* deletes it with the rest. It is the user's own
//! profile — never a People entry.
//!
//! **Labelling.** Only in single-channel recordings ([`single_channel`]):
//! a room meeting, system audio without a separate mic, a transcription
//! with *Identify voices*. The mic channel of browser and system-audio
//! meetings is already "You" and is left alone. After the voices are
//! final (end of a run, *Identify voices*, *Re-detect speakers*), the
//! document voice that best matches the profile becomes "You" when its
//! cosine is at least [`YOU_THRESHOLD`] — decided on the document's voices,
//! not line by line (single lines are not reliable enough: at 2 % false
//! "You", about 7 % of the user's lines were missed). It only happens with
//! *Label my voice as You* on (on after enrolment, can be turned off), and
//! never over a label the user set: a voice they renamed or linked to a
//! person keeps it, and a voice labelled "You" that the user renamed or
//! linked afterwards is never labelled "You" automatically again
//! ([`DocSpeaker::own_voice`]).
//!
//! **Cloning (0.13, not built here).** This profile is also the reference
//! the plan's own-voice cloning checks the consent recording against
//! (section 4.6): it is the user's voice, enrolled in the app, live.

use super::cluster::{dot, l2_normalize};
use super::doc::{voice_color, voice_label, voice_number, YOU_COLOR, YOU_ID};
use super::model::{SpeakerEmbedder, EMBEDDING_DIM};
use super::profiles::{document_voice, PROFILE_MODEL};
use super::tracker::windows;
use super::voices::VOICES_DIR;
use super::{LIVE_WINDOW_MS, MIN_EMBED_MS};
use crate::archive::{Channel, DocSpeaker, SegmentsFile};
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Cosine the best document voice needs with the profile to be "You"
/// (spike V0-1: the 2 % false-"You" point was 0.40 on AMI far-field and
/// 0.44 on Italian; at the voice level "You" was right in 100 % of the AMI
/// trials).
pub const YOU_THRESHOLD: f32 = 0.45;
/// Least speech an enrolment needs.
pub const MIN_ENROL_SPEECH_MS: u64 = 20_000;
/// Recording length the paragraph is written for (spike: 30 s is enough).
pub const TARGET_ENROL_MS: u64 = 30_000;
/// Longest recording used; the dialog stops by itself here.
pub const MAX_ENROL_MS: u64 = 90_000;
/// The profile's file in the voices folder.
pub const OWN_VOICE_FILE: &str = "you.own-voice.json";
/// `version` of the file.
pub const OWN_VOICE_VERSION: u32 = 1;

const RATE: u64 = crate::engine::segmenter::RATE;
/// Frames of the quiet-part detector, in samples (30 ms).
const FRAME: usize = 480;
/// Frames kept on each side of a loud frame (150 ms: word edges, short
/// pauses inside a phrase).
const HANGOVER: usize = 5;
/// Level under which a frame is always quiet (RMS, full scale 1.0).
const FLOOR_RMS: f32 = 0.004;
/// A frame is speech when louder than this share of the loud frames'
/// level (the 95th percentile), so the rule follows the mic's gain.
const RELATIVE: f32 = 0.1;

fn ms_to_samples(ms: u64) -> usize {
    (ms * RATE / 1000) as usize
}

fn samples_to_ms(n: usize) -> u64 {
    n as u64 * 1000 / RATE
}

/// The speech of a recording (16 kHz mono): frames louder than the floor
/// and than a share of the recording's loud level, with a short hangover,
/// joined in order. Pure.
pub fn voiced(samples: &[f32]) -> Vec<f32> {
    let rms: Vec<f32> = samples
        .chunks(FRAME)
        .map(crate::audio::resample::rms)
        .collect();
    if rms.is_empty() {
        return Vec::new();
    }
    let mut sorted: Vec<f32> = rms.iter().copied().filter(|x| x.is_finite()).collect();
    sorted.sort_by(f32::total_cmp);
    let p95 = sorted
        .get(sorted.len().saturating_sub(1) * 95 / 100)
        .copied()
        .unwrap_or(0.0);
    let threshold = FLOOR_RMS.max(RELATIVE * p95);
    let loud: Vec<bool> = rms.iter().map(|r| *r > threshold).collect();
    let mut keep = vec![false; loud.len()];
    for (i, _) in loud.iter().enumerate().filter(|(_, l)| **l) {
        let from = i.saturating_sub(HANGOVER);
        let to = (i + HANGOVER + 1).min(keep.len());
        keep[from..to].iter_mut().for_each(|k| *k = true);
    }
    samples
        .chunks(FRAME)
        .zip(keep)
        .filter(|(_, k)| *k)
        .flat_map(|(c, _)| c.iter().copied())
        .collect()
}

/// Why an enrolment with `speech_ms` of speech can't be used, or `None`.
pub fn enrolment_problem(speech_ms: u64) -> Option<String> {
    (speech_ms < MIN_ENROL_SPEECH_MS).then(|| {
        format!(
            "Only {} s of speech was heard — read the whole paragraph aloud (at least {} s of \
             speech are needed).",
            speech_ms / 1000,
            MIN_ENROL_SPEECH_MS / 1000
        )
    })
}

/// `<app data>/voices/you.own-voice.json`.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct OwnVoiceProfile {
    pub version: u32,
    pub model: String,
    /// L2-normalised, duration-weighted mean of the enrolment's windows.
    pub centroid: Vec<f32>,
    /// Speech the centroid was built from.
    pub speech_ms: u64,
    /// *Label my voice as You*: on after enrolment, can be turned off.
    #[serde(default = "yes")]
    pub label_as_you: bool,
    /// RFC 3339 time of the enrolment.
    #[serde(default)]
    pub updated: String,
}

fn yes() -> bool {
    true
}

impl std::fmt::Debug for OwnVoiceProfile {
    // Never prints the vector (P13).
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OwnVoiceProfile")
            .field("dims", &self.centroid.len())
            .field("speech_ms", &self.speech_ms)
            .field("label_as_you", &self.label_as_you)
            .finish()
    }
}

impl OwnVoiceProfile {
    /// Built by the current model, with a usable centroid.
    pub fn usable(&self) -> bool {
        self.model == PROFILE_MODEL
            && self.centroid.len() == EMBEDDING_DIM
            && self.centroid.iter().all(|x| x.is_finite())
            && self.centroid.iter().any(|x| *x != 0.0)
    }
}

/// Build the profile from an enrolment recording (16 kHz mono): at most
/// [`MAX_ENROL_MS`] is used, the quiet parts are dropped, and the speech
/// must reach [`MIN_ENROL_SPEECH_MS`]. `label_as_you` is kept from the
/// previous enrolment (on for a first one).
pub fn enrol(
    embedder: &mut dyn SpeakerEmbedder,
    samples: &[f32],
    previous: Option<&OwnVoiceProfile>,
    updated: &str,
) -> Result<OwnVoiceProfile> {
    let used = &samples[..samples.len().min(ms_to_samples(MAX_ENROL_MS))];
    let speech = voiced(used);
    let speech_ms = samples_to_ms(speech.len());
    if let Some(problem) = enrolment_problem(speech_ms) {
        bail!("{problem}");
    }
    let mut sum = vec![0f64; EMBEDDING_DIM];
    let mut weight = 0f64;
    for w in windows(
        speech.len(),
        ms_to_samples(LIVE_WINDOW_MS),
        ms_to_samples(MIN_EMBED_MS),
    ) {
        let len = w.len() as f64;
        let e = embedder.embed(&speech[w])?;
        if e.len() != EMBEDDING_DIM || !e.iter().all(|x| x.is_finite()) {
            continue;
        }
        let e = l2_normalize(e);
        sum.iter_mut()
            .zip(&e)
            .for_each(|(a, b)| *a += len * f64::from(*b));
        weight += len;
    }
    let centroid = l2_normalize(sum.iter().map(|x| *x as f32).collect());
    if weight == 0.0 || centroid.iter().all(|x| *x == 0.0) {
        bail!("no voice could be measured in the recording — try again closer to the mic");
    }
    Ok(OwnVoiceProfile {
        version: OWN_VOICE_VERSION,
        model: PROFILE_MODEL.to_string(),
        centroid,
        speech_ms,
        label_as_you: previous.is_none_or(|p| p.label_as_you),
        updated: updated.to_string(),
    })
}

/// Whether "You" can be looked for by voice in this document (`source` is
/// its frontmatter `source`): not a browser meeting, and no line already
/// on the user's own mic channel — in a two-channel session the mic is
/// "You" already and every voice is someone else.
pub fn single_channel(file: &SegmentsFile, source: &str) -> bool {
    if source.starts_with("browser:") {
        return false;
    }
    let system = source == crate::sources::system::SOURCE_LABEL;
    !file
        .segments
        .iter()
        .any(|s| s.speaker_id.as_deref() == Some(YOU_ID) || (system && s.channel == Channel::Mic))
}

/// The document voice that sounds most like the user.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct OwnVoiceMatch {
    pub speaker_id: String,
    pub score: f32,
}

/// The "Voice N" of the document that best matches `you`, with its
/// cosine; ties go to the lower voice number. `None` without voices with
/// voice data.
pub fn best_voice(file: &SegmentsFile, source: &str, you: &[f32]) -> Option<OwnVoiceMatch> {
    if you.len() != EMBEDDING_DIM || !you.iter().all(|x| x.is_finite()) {
        return None;
    }
    let you = l2_normalize(you.to_vec());
    let mut scored: Vec<(u32, f32, &str)> = file
        .speakers
        .iter()
        .filter_map(|sp| {
            let n = voice_number(&sp.id)?;
            let v = document_voice(file, source, &sp.id)?;
            Some((n, dot(&you, &v.embedding), sp.id.as_str()))
        })
        .collect();
    scored.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
    scored.first().map(|(_, score, id)| OwnVoiceMatch {
        speaker_id: id.to_string(),
        score: *score,
    })
}

/// Whether the automatic "You" may go on this speaker: a voice with no
/// label or link of the user's, which the user never took "You" off.
fn may_label(sp: &DocSpeaker) -> bool {
    let Some(n) = voice_number(&sp.id) else {
        return false;
    };
    match sp.own_voice {
        Some(true) => true,
        Some(false) => false,
        None => sp.person_id.is_none() && sp.label == voice_label(n),
    }
}

fn unlabel(sp: &mut DocSpeaker) {
    if let Some(n) = voice_number(&sp.id) {
        sp.label = voice_label(n);
        sp.color = voice_color(n);
    }
    sp.own_voice = None;
}

/// Label the user's voice "You" in a single-channel document: the best
/// matching voice ([`best_voice`]) when its cosine reaches
/// [`YOU_THRESHOLD`] and the user set no label of their own on it. A voice
/// labelled "You" automatically before that no longer is the best match
/// gets its "Voice N" back. Nothing changes in a two-channel document or
/// when the best voice carries the user's label (the next best is never
/// tried: the user said who that voice is). Returns whether anything
/// changed. Pure.
pub fn label_you(file: &mut SegmentsFile, source: &str, you: &[f32]) -> bool {
    if !single_channel(file, source) {
        return false;
    }
    let target = best_voice(file, source, you)
        .filter(|m| m.score >= YOU_THRESHOLD)
        .map(|m| m.speaker_id)
        .filter(|id| file.speakers.iter().any(|s| &s.id == id && may_label(s)));
    let mut changed = false;
    for sp in file.speakers.iter_mut() {
        let is_target = target.as_deref() == Some(sp.id.as_str());
        if sp.own_voice == Some(true) && !is_target {
            unlabel(sp);
            changed = true;
        } else if is_target && sp.own_voice != Some(true) {
            sp.own_voice = Some(true);
            sp.label = "You".to_string();
            sp.color = YOU_COLOR.to_string();
            changed = true;
        }
    }
    changed
}

/// What the UI may know of the own-voice profile (no vector, ever).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OwnVoiceStatus {
    pub enrolled: bool,
    /// Speech the profile was built from.
    pub speech_ms: u64,
    /// *Label my voice as You*.
    pub label_as_you: bool,
    /// RFC 3339 time of the enrolment; empty when not enrolled.
    pub updated: String,
    pub min_speech_ms: u64,
    pub target_ms: u64,
    pub max_ms: u64,
}

impl OwnVoiceStatus {
    fn of(p: Option<&OwnVoiceProfile>) -> Self {
        OwnVoiceStatus {
            enrolled: p.is_some_and(OwnVoiceProfile::usable),
            speech_ms: p.map_or(0, |p| p.speech_ms),
            label_as_you: p.is_some_and(|p| p.label_as_you),
            updated: p.map(|p| p.updated.clone()).unwrap_or_default(),
            min_speech_ms: MIN_ENROL_SPEECH_MS,
            target_ms: TARGET_ENROL_MS,
            max_ms: MAX_ENROL_MS,
        }
    }
}

/// The own-voice profile on disk. Cheap to create.
#[derive(Debug, Clone)]
pub struct OwnVoiceStore {
    dir: PathBuf,
}

impl OwnVoiceStore {
    /// The store in `<app data>/voices` (`app_data` is the app data dir).
    pub fn in_app_data(app_data: &Path) -> Self {
        Self::at(app_data.join(VOICES_DIR))
    }

    /// The store in the voices folder `dir` itself.
    pub fn at(dir: PathBuf) -> Self {
        OwnVoiceStore { dir }
    }

    fn path(&self) -> PathBuf {
        self.dir.join(OWN_VOICE_FILE)
    }

    fn read(&self) -> Result<Option<OwnVoiceProfile>> {
        match std::fs::read(self.path()) {
            Ok(b) => Ok(Some(
                serde_json::from_slice(&b).context("your voice profile can't be read")?,
            )),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e).context("reading your voice profile"),
        }
    }

    fn write(&self, p: &OwnVoiceProfile) -> Result<()> {
        std::fs::create_dir_all(&self.dir)
            .with_context(|| format!("creating {}", self.dir.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&self.dir, std::fs::Permissions::from_mode(0o700))?;
        }
        let json = serde_json::to_vec(p).context("serializing your voice profile")?;
        crate::settings::write_private_atomic(&self.path(), &json)
            .context("writing your voice profile")
    }

    pub fn status(&self) -> OwnVoiceStatus {
        let _l = super::voices::lock();
        OwnVoiceStatus::of(self.read().ok().flatten().as_ref())
    }

    /// Enrol (or re-enrol) from a recording ([`enrol`]); the previous
    /// profile is replaced only when the new one could be built.
    pub fn enrol(
        &self,
        embedder: &mut dyn SpeakerEmbedder,
        samples: &[f32],
    ) -> Result<OwnVoiceStatus> {
        let previous = {
            let _l = super::voices::lock();
            self.read().ok().flatten()
        };
        let now = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        let p = enrol(embedder, samples, previous.as_ref(), &now)?;
        let _l = super::voices::lock();
        self.write(&p)?;
        Ok(OwnVoiceStatus::of(Some(&p)))
    }

    /// Turn *Label my voice as You* on or off.
    pub fn set_label_as_you(&self, on: bool) -> Result<OwnVoiceStatus> {
        let _l = super::voices::lock();
        let Some(mut p) = self.read()? else {
            bail!("record your voice first");
        };
        if p.label_as_you != on {
            p.label_as_you = on;
            self.write(&p)?;
        }
        Ok(OwnVoiceStatus::of(Some(&p)))
    }

    /// *Forget my voice*: delete the profile (a real delete, never the
    /// trash). Documents keep the "You" labels they already have. Returns
    /// whether there was one.
    pub fn forget(&self) -> Result<bool> {
        let _l = super::voices::lock();
        match std::fs::remove_file(self.path()) {
            Ok(()) => Ok(true),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(e).context("deleting your voice profile"),
        }
    }

    /// The centroid to label "You" with, whatever the toggle says (an
    /// explicit *Find my voice*); `None` when not enrolled.
    pub fn centroid(&self) -> Option<Vec<f32>> {
        let _l = super::voices::lock();
        let p = self.read().ok().flatten()?;
        p.usable().then_some(p.centroid)
    }

    /// The centroid for automatic labelling: enrolled **and** *Label my
    /// voice as You* on.
    pub fn labelling_voice(&self) -> Option<Vec<f32>> {
        let _l = super::voices::lock();
        let p = self.read().ok().flatten()?;
        (p.usable() && p.label_as_you).then_some(p.centroid)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::archive::Segment;
    use crate::speakers::doc::{rename_speaker, voice_speaker};
    use crate::speakers::profiles::tests::{axis, towards};

    /// A tone of `ms` at `amp` (16 kHz).
    fn tone(ms: u64, amp: f32) -> Vec<f32> {
        (0..ms_to_samples(ms))
            .map(|i| amp * (i as f32 * 0.07).sin())
            .collect()
    }

    /// Embeds every window as the user's voice (axis 0), counting calls.
    struct Fixed(usize);
    impl SpeakerEmbedder for Fixed {
        fn embed(&mut self, samples: &[f32]) -> Result<Vec<f32>> {
            assert!(samples.len() <= ms_to_samples(LIVE_WINDOW_MS));
            self.0 += 1;
            Ok(axis(0))
        }
    }

    fn line(id: u32, speaker: &str, emb: Vec<f32>) -> Segment {
        Segment {
            id,
            channel: Channel::Mic,
            start_ms: u64::from(id) * 20_000,
            end_ms: u64::from(id) * 20_000 + 15_000,
            speaker_id: Some(speaker.to_string()),
            text: format!("line {id}"),
            embedding: Some(emb),
            ..Default::default()
        }
    }

    /// A room recording: voice 1 is someone else (axis 1), voice 2 sounds
    /// like the user at `cos` with axis 0.
    fn room(cos: f32) -> SegmentsFile {
        SegmentsFile {
            speakers: vec![voice_speaker(1), voice_speaker(2)],
            segments: vec![
                line(0, "voice:1", axis(1)),
                line(1, "voice:2", towards(&axis(0), &axis(2), cos)),
                line(2, "voice:1", axis(1)),
            ],
            ..Default::default()
        }
    }

    fn you() -> Vec<f32> {
        axis(0)
    }

    fn sp<'a>(f: &'a SegmentsFile, id: &str) -> &'a DocSpeaker {
        f.speakers.iter().find(|s| s.id == id).unwrap()
    }

    // ---- enrolment ----

    #[test]
    fn quiet_parts_are_dropped_from_the_enrolment() {
        let mut rec = tone(5_000, 0.3);
        rec.extend(vec![0.0; ms_to_samples(10_000)]);
        rec.extend(tone(5_000, 0.3));
        let ms = samples_to_ms(voiced(&rec).len());
        // 10 s of speech, plus at most the hangover on each edge.
        assert!((10_000..=10_400).contains(&ms), "{ms}");
        assert!(voiced(&[]).is_empty());
        assert!(
            voiced(&vec![0.001; 16_000]).is_empty(),
            "noise under the floor"
        );
    }

    #[test]
    fn enrolment_needs_enough_speech() {
        assert!(enrolment_problem(MIN_ENROL_SPEECH_MS - 1).is_some());
        assert!(enrolment_problem(MIN_ENROL_SPEECH_MS).is_none());
        // 30 s of recording but only 15 s of speech: refused.
        let mut rec = tone(15_000, 0.3);
        rec.extend(vec![0.0; ms_to_samples(15_000)]);
        let e = enrol(&mut Fixed(0), &rec, None, "t").unwrap_err();
        assert!(e.to_string().contains("15 s"), "{e}");
    }

    #[test]
    fn enrolment_builds_one_centroid_from_windows_of_at_most_3_s() {
        let mut emb = Fixed(0);
        let p = enrol(
            &mut emb,
            &tone(TARGET_ENROL_MS, 0.3),
            None,
            "2026-09-25T10:00:00Z",
        )
        .unwrap();
        assert_eq!(p.speech_ms, TARGET_ENROL_MS);
        assert_eq!(emb.0, 10);
        assert!(p.usable());
        assert!((dot(&p.centroid, &axis(0)) - 1.0).abs() < 1e-5);
        assert!(p.label_as_you, "on after a first enrolment");
        assert_eq!(p.model, PROFILE_MODEL);
    }

    #[test]
    fn enrolment_uses_at_most_the_maximum_and_keeps_the_toggle() {
        let mut emb = Fixed(0);
        let prev = OwnVoiceProfile {
            label_as_you: false,
            ..enrol(&mut emb, &tone(30_000, 0.3), None, "t").unwrap()
        };
        let p = enrol(
            &mut emb,
            &tone(MAX_ENROL_MS + 30_000, 0.3),
            Some(&prev),
            "t",
        )
        .unwrap();
        assert_eq!(p.speech_ms, MAX_ENROL_MS);
        assert!(!p.label_as_you, "re-enrolling keeps the user's choice");
    }

    #[test]
    fn debug_never_prints_the_vector() {
        let p = enrol(&mut Fixed(0), &tone(30_000, 0.3), None, "t").unwrap();
        let s = format!("{p:?} {:?}", OwnVoiceStatus::of(Some(&p)));
        assert!(!s.contains("centroid") && !s.contains("1.0"), "{s}");
    }

    // ---- the matching rule ----

    #[test]
    fn the_best_voice_above_the_threshold_becomes_you() {
        let mut f = room(0.6);
        assert!(label_you(&mut f, "mic", &you()));
        let v2 = sp(&f, "voice:2");
        assert_eq!(v2.label, "You");
        assert_eq!(v2.own_voice, Some(true));
        assert_eq!(v2.color, YOU_COLOR);
        assert_eq!(sp(&f, "voice:1").label, "Voice 1");
        // Lines keep their voice: only the label changes.
        assert_eq!(f.segments[1].speaker_id.as_deref(), Some("voice:2"));
        // Running it again changes nothing.
        assert!(!label_you(&mut f, "mic", &you()));
    }

    #[test]
    fn under_the_threshold_nobody_is_you() {
        let mut f = room(YOU_THRESHOLD - 0.02);
        assert!(!label_you(&mut f, "mic", &you()));
        assert!(f.speakers.iter().all(|s| s.own_voice.is_none()));
        let mut f = room(YOU_THRESHOLD + 0.02);
        assert!(label_you(&mut f, "file:talk.mp3", &you()));
    }

    #[test]
    fn only_the_best_voice_is_ever_you() {
        // Both pass the threshold: only the better one is "You".
        let mut f = room(0.9);
        f.segments[0].embedding = Some(towards(&axis(0), &axis(3), 0.5));
        f.segments[2].embedding = f.segments[0].embedding.clone();
        label_you(&mut f, "mic", &you());
        assert_eq!(sp(&f, "voice:2").own_voice, Some(true));
        assert_eq!(sp(&f, "voice:1").own_voice, None);
        assert_eq!(best_voice(&f, "mic", &you()).unwrap().speaker_id, "voice:2");
    }

    #[test]
    fn a_you_that_no_longer_matches_goes_back_to_its_voice() {
        let mut f = room(0.8);
        label_you(&mut f, "mic", &you());
        // After a re-detect the lines of voice 2 turn out to be someone else.
        f.segments[1].embedding = Some(axis(5));
        assert!(label_you(&mut f, "mic", &you()));
        let v2 = sp(&f, "voice:2");
        assert_eq!((v2.label.as_str(), v2.own_voice), ("Voice 2", None));
        assert_eq!(v2.color, voice_color(2));
    }

    #[test]
    fn two_channel_documents_are_left_alone() {
        // A browser meeting: the mic is "You" already.
        let mut f = room(0.9);
        assert!(!label_you(&mut f, "browser:meet.google.com", &you()));
        // System audio + mic: the mic's lines are "You".
        let mut f = room(0.9);
        f.segments.push(Segment {
            id: 9,
            channel: Channel::Mic,
            speaker_id: Some(YOU_ID.into()),
            ..Default::default()
        });
        assert!(!single_channel(&f, "system"));
        assert!(!label_you(&mut f, "system", &you()));
        // System audio without a separate mic: single channel.
        let mut f = room(0.9);
        f.segments
            .iter_mut()
            .for_each(|s| s.channel = Channel::System);
        assert!(single_channel(&f, "system"));
        assert!(label_you(&mut f, "system", &you()));
    }

    #[test]
    fn a_bad_profile_labels_nothing() {
        let mut f = room(0.9);
        assert!(!label_you(&mut f, "mic", &[1.0; 3]));
        assert!(!label_you(&mut f, "mic", &vec![f32::NAN; EMBEDDING_DIM]));
    }

    // ---- never over the user's labels ----

    #[test]
    fn a_voice_the_user_named_or_linked_is_never_you() {
        let mut f = room(0.9);
        rename_speaker(&mut f, "voice:2", "Francesco").unwrap();
        assert!(!label_you(&mut f, "mic", &you()));
        assert_eq!(sp(&f, "voice:2").label, "Francesco");
        // Nor does the next best voice get it instead.
        assert!(f.speakers.iter().all(|s| s.own_voice.is_none()));

        let mut f = room(0.9);
        f.speakers[1].person_id = Some("p-1".into());
        f.speakers[1].label = "Anna".into();
        assert!(!label_you(&mut f, "mic", &you()));
        assert_eq!(sp(&f, "voice:2").label, "Anna");
    }

    #[test]
    fn taking_you_off_is_remembered() {
        let mut f = room(0.9);
        label_you(&mut f, "mic", &you());
        // The user gives the voice its "Voice 2" name back.
        rename_speaker(&mut f, "voice:2", "").unwrap();
        let v2 = sp(&f, "voice:2");
        assert_eq!((v2.label.as_str(), v2.own_voice), ("Voice 2", Some(false)));
        assert_eq!(v2.color, voice_color(2));
        assert!(
            !label_you(&mut f, "mic", &you()),
            "never again in this document"
        );

        // Renaming it keeps the name; linking it to someone gives "Voice N" back
        // before the person's name.
        let mut f = room(0.9);
        label_you(&mut f, "mic", &you());
        rename_speaker(&mut f, "voice:2", "Me").unwrap();
        assert_eq!(sp(&f, "voice:2").label, "Me");
        assert!(!label_you(&mut f, "mic", &you()));
    }

    #[test]
    fn linking_an_automatic_you_to_a_person() {
        use crate::archive::people::Person;
        use crate::archive::ItemMeta;
        let mut f = room(0.9);
        label_you(&mut f, "mic", &you());
        let anna = Person {
            id: "p-anna".into(),
            name: "Anna".into(),
            ..Default::default()
        };
        let mut meta = ItemMeta::default();
        crate::speakers::doc::link_speaker(&mut f, &mut meta, "voice:2", &anna).unwrap();
        let v2 = sp(&f, "voice:2").clone();
        assert_eq!(v2.label, "Anna");
        assert_eq!(v2.own_voice, Some(false));
        assert_eq!(v2.label_before_link.as_deref(), Some("Voice 2"));
        crate::speakers::doc::unlink_speaker(&mut f, "voice:2").unwrap();
        assert_eq!(sp(&f, "voice:2").label, "Voice 2");
        assert!(!label_you(&mut f, "mic", &you()));
    }

    // ---- the file ----

    fn store() -> (tempfile::TempDir, OwnVoiceStore) {
        let tmp = tempfile::tempdir().unwrap();
        let s = OwnVoiceStore::in_app_data(tmp.path());
        (tmp, s)
    }

    #[test]
    fn enrol_toggle_and_forget() {
        let (_tmp, s) = store();
        assert!(!s.status().enrolled);
        assert!(s.labelling_voice().is_none());
        assert!(
            s.set_label_as_you(false).is_err(),
            "nothing to turn off yet"
        );

        let st = s.enrol(&mut Fixed(0), &tone(30_000, 0.3)).unwrap();
        assert!(st.enrolled && st.label_as_you);
        assert_eq!(st.speech_ms, 30_000);
        assert!(s.labelling_voice().is_some());

        assert!(!s.set_label_as_you(false).unwrap().label_as_you);
        assert!(s.labelling_voice().is_none(), "off: no automatic labels");
        assert!(s.centroid().is_some(), "still enrolled");

        assert!(s.forget().unwrap());
        assert!(!s.dir.join(OWN_VOICE_FILE).exists());
        assert!(!s.status().enrolled);
        assert!(s.centroid().is_none());
        assert!(!s.forget().unwrap());
    }

    #[test]
    fn a_failed_re_enrolment_keeps_the_old_profile() {
        let (_tmp, s) = store();
        s.enrol(&mut Fixed(0), &tone(30_000, 0.3)).unwrap();
        assert!(s.enrol(&mut Fixed(0), &tone(5_000, 0.3)).is_err());
        assert_eq!(s.status().speech_ms, 30_000);
    }

    #[cfg(unix)]
    #[test]
    fn the_file_is_private() {
        use std::os::unix::fs::PermissionsExt;
        let (_tmp, s) = store();
        s.enrol(&mut Fixed(0), &tone(30_000, 0.3)).unwrap();
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&s.dir.join(OWN_VOICE_FILE)), 0o600);
        assert_eq!(mode(&s.dir), 0o700);
    }

    #[test]
    fn people_profiles_ignore_it_and_forget_all_deletes_it() {
        use crate::speakers::voices::VoiceStore;
        let (tmp, s) = store();
        s.enrol(&mut Fixed(0), &tone(30_000, 0.3)).unwrap();
        let people = VoiceStore::in_app_data(tmp.path());
        assert!(people.statuses().is_empty(), "never a person's profile");
        let archive = tmp.path().join("archive");
        std::fs::create_dir_all(&archive).unwrap();
        assert!(people.rebuild_all(&archive).unwrap().is_empty());
        assert!(s.status().enrolled, "a rebuild leaves it alone");
        people.forget_all().unwrap();
        assert!(!s.status().enrolled);
    }
}
