//! Speaker labels while a session runs: each finished segment of a
//! clustered channel is cut into ≤ 3 s windows, every window is embedded
//! and assigned by the online clusterer, and the segment takes the voice
//! holding most of its windows. The segment keeps the mean of its window
//! embeddings (P8: stored for "Re-detect" and later voice recognition).

use super::cluster::{mean_embedding, OnlineClusterer};
use super::doc::{voice_speaker, you_speaker, YOU_ID};
use super::model::SpeakerEmbedder;
use super::{LIVE_WINDOW_MS, MIN_EMBED_MS, ONLINE_THRESHOLD};
use crate::archive::{Channel, DocSpeaker};
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
}

impl SpeakerOptions {
    /// Voice clustering on these channels.
    pub fn clustering(channels: &[Channel]) -> Self {
        Self {
            cluster: channels.to_vec(),
            two_channel: false,
        }
    }

    /// Whether segments of `channel` are clustered.
    pub fn clusters(&self, channel: Channel) -> bool {
        !(self.two_channel && channel == Channel::Mic) && self.cluster.contains(&channel)
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

/// A segment's speaker, as decided by the [`Tracker`].
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Labelled {
    pub speaker_id: Option<String>,
    /// Stored in `segments.json` (segments of ≥ 1 s on clustered channels).
    pub embedding: Option<Vec<f32>>,
    /// A speaker the document doesn't list yet.
    pub new_speaker: Option<DocSpeaker>,
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
    /// One clusterer per clustered channel, with the voice number each of
    /// its clusters got (`None` until a segment is labelled with it).
    channels: Vec<(Channel, OnlineClusterer, Vec<Option<u32>>)>,
    next_voice: u32,
    you_listed: bool,
}

impl Tracker {
    pub fn new(options: SpeakerOptions, load: EmbedderLoader) -> Self {
        Self {
            options,
            embedder: Embedder::Pending(load),
            channels: Vec::new(),
            next_voice: 1,
            you_listed: false,
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
        if self.options.two_channel && channel == Channel::Mic {
            let new_speaker = (!self.you_listed).then(you_speaker);
            self.you_listed = true;
            return Labelled {
                speaker_id: Some(YOU_ID.to_string()),
                embedding: None,
                new_speaker,
            };
        }
        if !self.options.clusters(channel) {
            return Labelled::default();
        }
        let pieces = windows(
            samples.len(),
            ms_to_samples(LIVE_WINDOW_MS),
            ms_to_samples(MIN_EMBED_MS),
        );
        if pieces.is_empty() {
            return Labelled::default();
        }
        let Some(embedder) = self.embedder() else {
            return Labelled::default();
        };
        let mut embedded: Vec<(Range<usize>, Vec<f32>)> = Vec::new();
        for r in pieces {
            match embedder.embed(&samples[r.clone()]) {
                Ok(e) => embedded.push((r, e)),
                Err(e) => eprintln!("speakers: window not embedded ({e:#})"),
            }
        }
        let Some(embedding) =
            mean_embedding(embedded.iter().map(|(r, e)| (e.as_slice(), r.len() as f32)))
        else {
            return Labelled::default();
        };

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
        let (number, new_speaker) = match numbers[cluster] {
            Some(n) => (n, None),
            None => {
                let n = self.next_voice;
                self.next_voice += 1;
                numbers[cluster] = Some(n);
                (n, Some(voice_speaker(n)))
            }
        };
        Labelled {
            speaker_id: Some(super::doc::voice_id(number)),
            embedding: Some(embedding),
            new_speaker,
        }
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
}
