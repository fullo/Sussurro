//! Speaker labels while a session runs: each finished segment of a
//! clustered channel is cut into ≤ 3 s windows, every window is embedded
//! and assigned by the online clusterer, and the segment takes the voice
//! holding most of its windows. The segment keeps the mean of its window
//! embeddings (P8: stored for "Re-detect" and later voice recognition).
//!
//! A browser meeting's remote channel also has the page's names (#131,
//! [`super::names`]): a line the page attributes to a name is `meet:<name>`
//! instead of a voice (it still gets its embedding); the others keep their
//! "Voice N". At the end of the run [`Tracker::finish_names`] redoes the
//! attribution with every event the page sent (a name learned late applies
//! to the earlier lines too).
//!
//! With the overlap model (#244, [`Tracker::with_overlap`]) each line of a
//! clustered channel is also checked for overlapping speech; the spans go
//! to `Segment.overlap` and the second speakers are filled in once the
//! voices are final ([`super::overlap::assign_second_speakers`]).

use super::cluster::{mean_embedding, OnlineClusterer};
use super::doc::{
    is_meet, meet_id, meet_speaker, sync_named_speakers, voice_speaker, you_speaker, YOU_ID,
};
use super::model::SpeakerEmbedder;
use super::names::{Attribution, SharedNames};
use super::overlap::{OverlapLoader, OverlapModel};
use super::{LIVE_WINDOW_MS, MIN_EMBED_MS, ONLINE_THRESHOLD};
use crate::archive::{Channel, DocSpeaker, SegmentsFile};
use anyhow::Result;
use std::ops::Range;

/// Which channels a session labels with voices (engine option, off by
/// default).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SpeakerOptions {
    /// Channels whose segments are embedded and clustered into "Voice N".
    pub cluster: Vec<Channel>,
    /// The session records the mic on its own channel next to the others
    /// (browser, system audio): the mic is always "You" and never
    /// clustered.
    pub two_channel: bool,
    /// The meeting page's speaker timeline (browser meetings, #131): names
    /// for the remote channel's lines, before the voices.
    pub names: Option<SharedNames>,
}

impl SpeakerOptions {
    /// Voice clustering on these channels.
    pub fn clustering(channels: &[Channel]) -> Self {
        Self {
            cluster: channels.to_vec(),
            ..Default::default()
        }
    }

    /// Whether segments of `channel` are clustered.
    pub fn clusters(&self, channel: Channel) -> bool {
        !(self.two_channel && channel == Channel::Mic) && self.cluster.contains(&channel)
    }

    /// Whether segments of `channel` can take a name from the page: the
    /// remote side of a two-channel (browser) session with a timeline.
    pub fn names_on(&self, channel: Channel) -> bool {
        self.names.is_some() && self.two_channel && channel == Channel::Remote
    }

    /// Whether the options do anything at all.
    pub fn is_active(&self) -> bool {
        self.two_channel || !self.cluster.is_empty()
    }
}

/// Loads the embedder the first time a segment needs it (the model may
/// have to be downloaded).
pub type EmbedderLoader = Box<dyn FnOnce() -> Result<Box<dyn SpeakerEmbedder>> + Send>;

enum Embedder {
    Pending(EmbedderLoader),
    Ready(Box<dyn SpeakerEmbedder>),
    /// Loading failed: the session goes on without voices.
    Failed,
}

/// The overlap model, loaded like the embedder; without it (none given, or
/// it failed to load) lines simply carry no overlap.
enum Overlap {
    None,
    Pending(OverlapLoader),
    Ready(Box<dyn OverlapModel>),
}

/// The models a run's speaker labels use: the embedder, and the overlap
/// model when the run checks for overlapping speech (#244). Tests give an
/// [`EmbedderLoader`] alone (`into()`): no overlap model, no download.
pub struct SpeakerModels {
    pub embedder: EmbedderLoader,
    pub overlap: Option<OverlapLoader>,
}

impl From<EmbedderLoader> for SpeakerModels {
    fn from(embedder: EmbedderLoader) -> Self {
        Self {
            embedder,
            overlap: None,
        }
    }
}

/// A segment's speaker, as decided by the [`Tracker`].
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Labelled {
    pub speaker_id: Option<String>,
    /// Stored in `segments.json` (segments of ≥ 1 s on clustered channels).
    pub embedding: Option<Vec<f32>>,
    /// A speaker the document doesn't list yet.
    pub new_speaker: Option<DocSpeaker>,
    /// Overlapping speech in the line (#244), in ms from its start; empty
    /// unless the line counts as overlapped
    /// ([`super::overlap::line_spans`]).
    pub overlap: Vec<(u64, u64)>,
}

impl Labelled {
    /// The overlap spans on the session clock, for a line starting at
    /// `start_ms` (second speakers are filled in at the end of the run).
    pub fn overlap_at(&self, start_ms: u64) -> Vec<crate::archive::OverlapSpan> {
        super::overlap::spans_at(start_ms, &self.overlap)
    }
}

/// Equal pieces of at most `max` samples covering `len`, each at least
/// `min` long (a stretch shorter than `min` gives none). Same cut as the
/// spike (#107).
pub fn windows(len: usize, max: usize, min: usize) -> Vec<Range<usize>> {
    if len < min.max(1) || max == 0 {
        return Vec::new();
    }
    let n = len.div_ceil(max);
    (0..n)
        .map(|i| (i * len / n)..((i + 1) * len / n))
        .filter(|r| r.len() >= min)
        .collect()
}

fn ms_to_samples(ms: u64) -> usize {
    (ms * crate::engine::segmenter::RATE / 1000) as usize
}

/// Per-session speaker state (lives on the engine's worker thread).
pub struct Tracker {
    options: SpeakerOptions,
    embedder: Embedder,
    overlap: Overlap,
    /// One clusterer per clustered channel, with the voice number each of
    /// its clusters got (`None` until a segment is labelled with it).
    channels: Vec<(Channel, OnlineClusterer, Vec<Option<u32>>)>,
    next_voice: u32,
    you_listed: bool,
    /// Names from the page listed so far (`meet:<name>` ids).
    names_listed: Vec<String>,
    /// The cluster of each line that could take a name — `(channel, start
    /// ms) → (channel index, cluster)` — so the end-of-run pass can give a
    /// line the page no longer names its voice back.
    clustered: Vec<(Channel, u64, usize, usize)>,
    /// The user's enrolled voice, when *Label my voice as You* is on
    /// (#243): the end of the run labels the best-matching voice "You" in
    /// a single-channel document ([`super::own_voice::label_you`]).
    own_voice: Option<Vec<f32>>,
}

impl Tracker {
    pub fn new(options: SpeakerOptions, load: EmbedderLoader) -> Self {
        Self {
            options,
            embedder: Embedder::Pending(load),
            overlap: Overlap::None,
            channels: Vec::new(),
            next_voice: 1,
            you_listed: false,
            names_listed: Vec::new(),
            clustered: Vec::new(),
            own_voice: None,
        }
    }

    /// Label the user's voice "You" at the end of the run (#243); `None`
    /// leaves the voices as they are.
    pub fn with_own_voice(mut self, you: Option<Vec<f32>>) -> Self {
        self.own_voice = you;
        self
    }

    /// Check each clustered line for overlapping speech with this model
    /// (loaded the first time a line needs it; a model that fails to load
    /// only turns the check off).
    pub fn with_overlap(mut self, load: Option<OverlapLoader>) -> Self {
        self.overlap = load.map_or(Overlap::None, Overlap::Pending);
        self
    }

    /// A tracker for `models` (the run's embedder and overlap model).
    pub fn with_models(options: SpeakerOptions, models: SpeakerModels) -> Self {
        Self::new(options, models.embedder).with_overlap(models.overlap)
    }

    /// Overlap spans of one line (ms from its start); empty without the
    /// model or when it fails on this line.
    fn detect_overlap(&mut self, samples: &[f32]) -> Vec<(u64, u64)> {
        if samples.len() < ms_to_samples(super::overlap::MIN_LINE_MS) {
            return Vec::new();
        }
        if let Overlap::Pending(_) = self.overlap {
            let Overlap::Pending(load) = std::mem::replace(&mut self.overlap, Overlap::None) else {
                unreachable!()
            };
            match load() {
                Ok(m) => self.overlap = Overlap::Ready(m),
                Err(e) => eprintln!("speakers: no overlap detection for this session ({e:#})"),
            }
        }
        let Overlap::Ready(model) = &mut self.overlap else {
            return Vec::new();
        };
        super::overlap::detect(model.as_mut(), samples).unwrap_or_else(|e| {
            eprintln!("speakers: overlap not checked on a line ({e:#})");
            Vec::new()
        })
    }

    /// End of the run, after the voices are final: the best match of the
    /// user's enrolled voice becomes "You" (single-channel documents only;
    /// a no-op without an enrolled voice). `source` is the item's
    /// frontmatter `source`.
    pub fn label_own_voice(&self, file: &mut SegmentsFile, source: &str) {
        if let Some(you) = &self.own_voice {
            super::own_voice::label_you(file, source, you);
        }
    }

    fn embedder(&mut self) -> Option<&mut Box<dyn SpeakerEmbedder>> {
        if let Embedder::Pending(_) = self.embedder {
            let Embedder::Pending(load) = std::mem::replace(&mut self.embedder, Embedder::Failed)
            else {
                unreachable!()
            };
            match load() {
                Ok(e) => self.embedder = Embedder::Ready(e),
                Err(e) => eprintln!("speakers: no voice labels for this session ({e:#})"),
            }
        }
        match &mut self.embedder {
            Embedder::Ready(e) => Some(e),
            _ => None,
        }
    }

    /// Speaker and embedding of one finished segment (16 kHz mono).
    pub fn label(&mut self, channel: Channel, samples: &[f32]) -> Labelled {
        self.label_at(channel, None, samples)
    }

    /// [`Self::label`] for a segment at `span` (start, end ms on the
    /// session clock): on a channel with the page's names, a line the page
    /// attributes to a name gets it instead of a voice.
    pub fn label_at(
        &mut self,
        channel: Channel,
        span: Option<(u64, u64)>,
        samples: &[f32],
    ) -> Labelled {
        if self.options.two_channel && channel == Channel::Mic {
            let new_speaker = (!self.you_listed).then(you_speaker);
            self.you_listed = true;
            return Labelled {
                speaker_id: Some(YOU_ID.to_string()),
                new_speaker,
                ..Default::default()
            };
        }
        if !self.options.clusters(channel) {
            return Labelled::default();
        }
        let name = match (span, &self.options.names) {
            (Some((s, e)), Some(names)) if self.options.names_on(channel) => {
                match names.attribute(s, e) {
                    Attribution::Named(n) => Some(n),
                    _ => None,
                }
            }
            _ => None,
        };
        let clustered = self.cluster(channel, samples);
        // Only lines that got voice data (long enough, embedder loaded).
        let overlap = if clustered.is_some() {
            self.detect_overlap(samples)
        } else {
            Vec::new()
        };
        if let (Some((s, _)), Some((pos, c, _))) = (span, &clustered) {
            if self.options.names_on(channel) {
                self.clustered.push((channel, s, *pos, *c));
            }
        }
        if let Some(name) = name {
            let (id, new_speaker) = self.name_speaker(&name);
            return Labelled {
                speaker_id: Some(id),
                embedding: clustered.map(|c| c.2),
                new_speaker,
                overlap,
            };
        }
        let Some((pos, cluster, embedding)) = clustered else {
            return Labelled {
                overlap,
                ..Default::default()
            };
        };
        let (number, new_speaker) = self.voice_of(pos, cluster);
        Labelled {
            speaker_id: Some(super::doc::voice_id(number)),
            embedding: Some(embedding),
            new_speaker,
            overlap,
        }
    }

    /// The `meet:` id of a page name, and its entry the first time.
    fn name_speaker(&mut self, name: &str) -> (String, Option<DocSpeaker>) {
        let id = meet_id(name);
        if self.names_listed.contains(&id) {
            return (id, None);
        }
        let sp = meet_speaker(name, self.names_listed.len());
        self.names_listed.push(id.clone());
        (id, Some(sp))
    }

    /// The voice number of `cluster` on channel index `pos` (numbered the
    /// first time a line is labelled with it) and its new entry, if new.
    fn voice_of(&mut self, pos: usize, cluster: usize) -> (u32, Option<DocSpeaker>) {
        let numbers = &mut self.channels[pos].2;
        if numbers.len() <= cluster {
            numbers.resize(cluster + 1, None);
        }
        match numbers[cluster] {
            Some(n) => (n, None),
            None => {
                let n = self.next_voice;
                self.next_voice += 1;
                numbers[cluster] = Some(n);
                (n, Some(voice_speaker(n)))
            }
        }
    }

    /// Embed the segment and assign its cluster: `(channel index,
    /// cluster, mean embedding)`, or `None` when it is too short or there
    /// is no model.
    fn cluster(&mut self, channel: Channel, samples: &[f32]) -> Option<(usize, usize, Vec<f32>)> {
        let pieces = windows(
            samples.len(),
            ms_to_samples(LIVE_WINDOW_MS),
            ms_to_samples(MIN_EMBED_MS),
        );
        if pieces.is_empty() {
            return None;
        }
        let embedder = self.embedder()?;
        let mut embedded: Vec<(Range<usize>, Vec<f32>)> = Vec::new();
        for r in pieces {
            match embedder.embed(&samples[r.clone()]) {
                Ok(e) => embedded.push((r, e)),
                Err(e) => eprintln!("speakers: window not embedded ({e:#})"),
            }
        }
        let embedding =
            mean_embedding(embedded.iter().map(|(r, e)| (e.as_slice(), r.len() as f32)))?;

        let pos = match self.channels.iter().position(|c| c.0 == channel) {
            Some(p) => p,
            None => {
                self.channels
                    .push((channel, OnlineClusterer::new(ONLINE_THRESHOLD), Vec::new()));
                self.channels.len() - 1
            }
        };
        let (_, clusterer, numbers) = &mut self.channels[pos];
        // Speech per cluster; ties go to the cluster seen first here.
        let mut share: Vec<(usize, usize)> = Vec::new();
        for (r, e) in &embedded {
            let c = clusterer.assign(e);
            match share.iter_mut().find(|(k, _)| *k == c) {
                Some(s) => s.1 += r.len(),
                None => share.push((c, r.len())),
            }
        }
        numbers.resize(clusterer.len(), None);
        let cluster = share
            .iter()
            .fold(share[0], |best, s| if s.1 > best.1 { *s } else { best })
            .0;
        Some((pos, cluster, embedding))
    }

    /// Names the page gave during the meeting (participants and named
    /// speakers), for the item's participants; empty without a timeline.
    pub fn participants(&self) -> Vec<String> {
        self.options
            .names
            .as_ref()
            .map(SharedNames::participants)
            .unwrap_or_default()
    }

    /// End of the run, before the voices are folded: attribute every line
    /// of the named channel again with all the page's events (a name
    /// learned late, a lag the live pass could not see yet). A line the
    /// page now names takes the name; a named line it no longer names
    /// goes back to its voice (or to no speaker, if it had no voice data);
    /// voice lines the page does not name keep their voice. Then the
    /// speaker list follows the lines. Returns how many lines changed.
    pub fn finish_names(&mut self, file: &mut SegmentsFile) -> usize {
        let Some(names) = self.options.names.clone() else {
            return 0;
        };
        // Lines older than what the timeline still holds keep the name
        // the live pass gave them (#217: old intervals age out).
        let horizon = names.forgotten_before().unwrap_or(0);
        let mut changed = 0;
        for i in 0..file.segments.len() {
            let seg = &file.segments[i];
            if !self.options.names_on(seg.channel)
                || seg.stt_error.is_some()
                || seg.start_ms < horizon
            {
                continue;
            }
            let want = match names.attribute(seg.start_ms, seg.end_ms) {
                Attribution::Named(n) => Some(meet_id(&n)),
                _ => match seg.speaker_id.as_deref() {
                    Some(id) if is_meet(id) => {
                        let (channel, start) = (seg.channel, seg.start_ms);
                        self.clustered
                            .iter()
                            .find(|c| c.0 == channel && c.1 == start)
                            .map(|&(_, _, pos, cluster)| (pos, cluster))
                            .map(|(pos, cluster)| {
                                super::doc::voice_id(self.voice_of(pos, cluster).0)
                            })
                    }
                    other => other.map(str::to_string),
                },
            };
            if file.segments[i].speaker_id != want {
                file.segments[i].speaker_id = want;
                changed += 1;
            }
        }
        sync_named_speakers(file);
        changed
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::speakers::cluster::tests::{sample, voices, Lcg};

    /// Fake embedder: the window's peak level picks the voice (0.2 →
    /// voice 0, 0.4 → voice 1, …), plus noise.
    pub(crate) struct FakeEmbedder {
        pub voices: Vec<Vec<f32>>,
        pub rng: Lcg,
        pub calls: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    }

    impl SpeakerEmbedder for FakeEmbedder {
        fn embed(&mut self, samples: &[f32]) -> Result<Vec<f32>> {
            self.calls
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let peak = samples.iter().fold(0f32, |m, x| m.max(x.abs()));
            let v = ((peak * 5.0).round() as usize).saturating_sub(1);
            Ok(sample(
                &mut self.rng,
                &self.voices[v % self.voices.len()],
                0.9,
            ))
        }
    }

    pub(crate) fn fake_loader(
        seed: u64,
    ) -> (
        EmbedderLoader,
        std::sync::Arc<std::sync::atomic::AtomicUsize>,
    ) {
        let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let c = calls.clone();
        let loader: EmbedderLoader = Box::new(move || {
            let (rng, voices) = voices(3, seed);
            Ok(Box::new(FakeEmbedder {
                voices,
                rng,
                calls: c,
            }) as Box<dyn SpeakerEmbedder>)
        });
        (loader, calls)
    }

    fn audio(voice: usize, ms: u64) -> Vec<f32> {
        vec![(voice as f32 + 1.0) * 0.2; ms_to_samples(ms)]
    }

    #[test]
    fn windows_cut_equal_pieces_of_at_most_max() {
        assert!(windows(0, 3, 1).is_empty());
        assert!(windows(999, 3000, 1000).is_empty());
        assert_eq!(windows(1000, 3000, 1000), vec![0..1000]);
        assert_eq!(windows(3000, 3000, 1000), vec![0..3000]);
        assert_eq!(windows(3001, 3000, 1000), vec![0..1500, 1500..3001]);
        let w = windows(30_000, 3000, 1000);
        assert_eq!(w.len(), 10);
        assert!(w.iter().all(|r| r.len() == 3000));
    }

    #[test]
    fn labels_follow_voices_and_new_speakers_are_announced_once() {
        let (load, calls) = fake_loader(17);
        let mut t = Tracker::new(SpeakerOptions::clustering(&[Channel::Mic]), load);
        let got: Vec<Labelled> = [0, 1, 0, 1, 2, 0]
            .iter()
            .map(|&v| t.label(Channel::Mic, &audio(v, 4000)))
            .collect();
        let ids: Vec<&str> = got
            .iter()
            .map(|l| l.speaker_id.as_deref().unwrap())
            .collect();
        assert_eq!(
            ids,
            ["voice:1", "voice:2", "voice:1", "voice:2", "voice:3", "voice:1"]
        );
        let new: Vec<&str> = got
            .iter()
            .filter_map(|l| l.new_speaker.as_ref().map(|s| s.id.as_str()))
            .collect();
        assert_eq!(new, ["voice:1", "voice:2", "voice:3"]);
        assert!(got
            .iter()
            .all(|l| l.embedding.as_ref().is_some_and(|e| e.len() == 256)));
        // 4 s = two 2 s windows per segment.
        assert_eq!(calls.load(std::sync::atomic::Ordering::Relaxed), 12);
    }

    #[test]
    fn short_segments_and_other_channels_get_no_voice() {
        let (load, calls) = fake_loader(1);
        let mut t = Tracker::new(SpeakerOptions::clustering(&[Channel::Remote]), load);
        assert_eq!(
            t.label(Channel::Remote, &audio(0, 900)),
            Labelled::default()
        );
        assert_eq!(t.label(Channel::Mic, &audio(0, 4000)), Labelled::default());
        // Nothing needed the model: it was never loaded.
        assert_eq!(calls.load(std::sync::atomic::Ordering::Relaxed), 0);
        assert!(t
            .label(Channel::Remote, &audio(0, 1000))
            .speaker_id
            .is_some());
    }

    #[test]
    fn two_channel_mic_is_always_you_and_never_embedded() {
        let (load, calls) = fake_loader(1);
        let opts = SpeakerOptions {
            cluster: vec![Channel::Mic, Channel::Remote],
            two_channel: true,
            names: None,
        };
        assert!(!opts.clusters(Channel::Mic) && opts.clusters(Channel::Remote));
        let mut t = Tracker::new(opts, load);
        let a = t.label(Channel::Mic, &audio(0, 4000));
        assert_eq!(a.speaker_id.as_deref(), Some("you"));
        assert_eq!(a.new_speaker, Some(you_speaker()));
        assert!(a.embedding.is_none());
        let b = t.label(Channel::Mic, &audio(1, 4000));
        assert_eq!(
            (b.speaker_id.as_deref(), b.new_speaker),
            (Some("you"), None)
        );
        assert_eq!(calls.load(std::sync::atomic::Ordering::Relaxed), 0);
        assert_eq!(
            t.label(Channel::Remote, &audio(1, 4000))
                .speaker_id
                .as_deref(),
            Some("voice:1")
        );
    }

    #[test]
    fn clustered_lines_are_checked_for_overlap_when_the_model_is_given() {
        use crate::speakers::overlap::tests::LoudIsOverlap;
        let loads = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let l2 = loads.clone();
        let (load, _) = fake_loader(4);
        let opts = SpeakerOptions {
            cluster: vec![Channel::Mic, Channel::Remote],
            two_channel: true,
            names: None,
        };
        let mut t = Tracker::with_models(
            opts,
            SpeakerModels {
                embedder: load,
                overlap: Some(Box::new(move || {
                    l2.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    Ok(Box::new(LoudIsOverlap) as Box<dyn OverlapModel>)
                })),
            },
        );
        // The mic of a two-channel run is "You": never checked.
        assert!(t.label(Channel::Mic, &audio(2, 4000)).overlap.is_empty());
        assert_eq!(loads.load(std::sync::atomic::Ordering::Relaxed), 0);
        // A loud (fake-overlapping) remote line: one span over the line.
        let l = t.label(Channel::Remote, &audio(2, 4000));
        assert_eq!(l.overlap.len(), 1);
        assert!(
            l.overlap[0].0 == 0 && l.overlap[0].1 >= 3_900,
            "{:?}",
            l.overlap
        );
        assert!(l.speaker_id.is_some() && l.embedding.is_some());
        let at = l.overlap_at(10_000);
        assert_eq!((at[0].start_ms, at[0].speaker_id.clone()), (10_000, None));
        // A quiet line: none; the model was loaded once.
        assert!(t.label(Channel::Remote, &audio(0, 4000)).overlap.is_empty());
        assert_eq!(loads.load(std::sync::atomic::Ordering::Relaxed), 1);

        // No overlap model (tests, older callers): nothing, no load.
        let (load, _) = fake_loader(4);
        let mut plain =
            Tracker::with_models(SpeakerOptions::clustering(&[Channel::File]), load.into());
        assert!(plain
            .label(Channel::File, &audio(2, 4000))
            .overlap
            .is_empty());

        // A model that fails to load only turns the check off.
        let (load, _) = fake_loader(4);
        let mut broken = Tracker::new(SpeakerOptions::clustering(&[Channel::File]), load)
            .with_overlap(Some(Box::new(|| anyhow::bail!("offline"))));
        let l = broken.label(Channel::File, &audio(2, 4000));
        assert!(l.overlap.is_empty() && l.speaker_id.is_some());
    }

    #[test]
    fn a_model_that_fails_to_load_turns_voices_off_once() {
        let tries = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let t2 = tries.clone();
        let mut t = Tracker::new(
            SpeakerOptions::clustering(&[Channel::Mic]),
            Box::new(move || {
                t2.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                anyhow::bail!("offline")
            }),
        );
        assert_eq!(t.label(Channel::Mic, &audio(0, 4000)), Labelled::default());
        assert_eq!(t.label(Channel::Mic, &audio(0, 4000)), Labelled::default());
        assert_eq!(tries.load(std::sync::atomic::Ordering::Relaxed), 1);
        assert!(!SpeakerOptions::default().is_active());
    }

    mod names {
        use super::*;
        use crate::archive::meeting::{MeetingEvent, NameSource};
        use crate::archive::Segment;
        use crate::speakers::names::{AttributionParams, SharedNames};

        fn browser(names: &SharedNames) -> SpeakerOptions {
            SpeakerOptions {
                cluster: vec![Channel::Remote],
                two_channel: true,
                names: Some(names.clone()),
            }
        }

        fn rtp(names: &SharedNames, id: &str, from: u64, to: u64) {
            names.push(&MeetingEvent::SpeakerActive {
                at_ms: from,
                t_ms: from,
                name: None,
                id: Some(id.into()),
                source: NameSource::Rtp,
            });
            names.push(&MeetingEvent::SpeakerIdle {
                at_ms: to,
                t_ms: to,
                name: None,
                id: Some(id.into()),
            });
        }

        fn bind(names: &SharedNames, id: &str, name: Option<&str>) {
            names.push(&MeetingEvent::SpeakerName {
                at_ms: 0,
                id: id.into(),
                name: name.map(str::to_string),
            });
        }

        fn line(start: u64, end: u64, l: &Labelled) -> Segment {
            Segment {
                channel: Channel::Remote,
                start_ms: start,
                end_ms: end,
                text: "x".into(),
                speaker_id: l.speaker_id.clone(),
                embedding: l.embedding.clone(),
                ..Default::default()
            }
        }

        #[test]
        fn named_lines_take_the_page_name_and_the_rest_keep_voices() {
            let names = SharedNames::new(AttributionParams::default());
            bind(&names, "csrc:1", Some("Anna"));
            rtp(&names, "csrc:1", 0, 4_000);
            let (load, _) = fake_loader(3);
            let mut t = Tracker::new(browser(&names), load);

            let a = t.label_at(Channel::Remote, Some((0, 4_000)), &audio(0, 4_000));
            assert_eq!(a.speaker_id.as_deref(), Some("meet:Anna"));
            assert_eq!(
                a.new_speaker.as_ref().map(|s| s.label.as_str()),
                Some("Anna")
            );
            assert!(a.embedding.is_some(), "a named line keeps its voice data");
            // Nobody on the page: a voice, numbered from 1 (the named line
            // did not use up a number).
            let b = t.label_at(Channel::Remote, Some((5_000, 9_000)), &audio(1, 4_000));
            assert_eq!(b.speaker_id.as_deref(), Some("voice:1"));
            // A short line gets the name even without an embedding.
            rtp(&names, "csrc:1", 20_000, 20_800);
            let c = t.label_at(Channel::Remote, Some((20_000, 20_800)), &audio(0, 800));
            assert_eq!(
                (c.speaker_id.as_deref(), c.new_speaker, c.embedding),
                (Some("meet:Anna"), None, None)
            );
            // Without a span (or on the mic) the page is never asked.
            assert_eq!(
                t.label(Channel::Remote, &audio(1, 4_000))
                    .speaker_id
                    .as_deref(),
                Some("voice:1")
            );
            assert_eq!(
                t.label_at(Channel::Mic, Some((0, 4_000)), &audio(0, 4_000))
                    .speaker_id
                    .as_deref(),
                Some("you")
            );
        }

        #[test]
        fn the_end_of_run_pass_applies_late_names_and_gives_voices_back() {
            let names = SharedNames::new(AttributionParams::default());
            bind(&names, "csrc:1", Some("Anna"));
            rtp(&names, "csrc:1", 0, 4_000);
            let (load, _) = fake_loader(5);
            let mut t = Tracker::new(browser(&names), load);
            let spans = [(0, 4_000, 0), (5_000, 9_000, 1), (10_000, 14_000, 1)];
            let mut file = SegmentsFile::default();
            for (s, e, v) in spans {
                let l = t.label_at(Channel::Remote, Some((s, e)), &audio(v, 4_000));
                if let Some(sp) = l.new_speaker.clone() {
                    file.speakers.push(sp);
                }
                file.segments.push(line(s, e, &l));
            }
            let ids = |f: &SegmentsFile| -> Vec<Option<String>> {
                f.segments.iter().map(|s| s.speaker_id.clone()).collect()
            };
            assert_eq!(
                ids(&file),
                [
                    Some("meet:Anna".into()),
                    Some("voice:1".into()),
                    Some("voice:1".into())
                ]
            );

            // Later the page learned who spoke at 10 s, and took Anna back.
            rtp(&names, "csrc:2", 10_000, 14_000);
            bind(&names, "csrc:2", Some("Bo"));
            bind(&names, "csrc:1", None);
            assert_eq!(t.finish_names(&mut file), 2);
            assert_eq!(
                ids(&file),
                [
                    Some("voice:2".into()),
                    Some("voice:1".into()),
                    Some("meet:Bo".into())
                ]
            );
            let listed: Vec<&str> = file.speakers.iter().map(|s| s.id.as_str()).collect();
            assert_eq!(listed, ["meet:Bo", "voice:1", "voice:2"]);
            assert_eq!(t.participants(), ["Bo"]);
            // Idempotent.
            assert_eq!(t.finish_names(&mut file), 0);
        }

        #[test]
        fn without_names_the_end_of_run_pass_does_nothing() {
            let (load, _) = fake_loader(1);
            let mut t = Tracker::new(SpeakerOptions::clustering(&[Channel::Remote]), load);
            let l = t.label_at(Channel::Remote, Some((0, 4_000)), &audio(0, 4_000));
            let mut file = SegmentsFile::default();
            file.segments.push(line(0, 4_000, &l));
            assert_eq!(t.finish_names(&mut file), 0);
            assert!(file.speakers.is_empty());
            assert!(t.participants().is_empty());
        }
    }
}
