//! Speech segmentation (plan §4.2): per-frame speech probabilities from a
//! [`SpeechDetector`] drive a small state machine that cuts the stream into
//! segments ending at the end of speech or at a hard cap (30 s).
//!
//! Everything here except the detectors is pure and runs on synthetic audio
//! in the tests. Memory: the segmenter holds at most one open segment
//! (≤ `max_segment_ms`) plus a `pad_ms` pre-roll; the aligner holds less
//! than one frame between calls.

use std::collections::VecDeque;

/// Samples per VAD frame: Silero's window at 16 kHz (32 ms).
pub const FRAME: usize = 512;
/// The engine's sample rate (16 kHz mono everywhere).
pub const RATE: u64 = 16_000;

pub fn ms_to_samples(ms: u32) -> usize {
    (ms as u64 * RATE / 1000) as usize
}

pub fn samples_to_ms(samples: u64) -> u64 {
    samples * 1000 / RATE
}

/// Speech probability per frame of audio.
pub trait SpeechDetector: Send {
    /// One probability in `[0, 1]` for each `FRAME`-sized frame of
    /// `samples` (`samples.len()` is a multiple of `FRAME`). Called with
    /// consecutive chunks of one stream.
    fn probabilities(&mut self, samples: &[f32]) -> anyhow::Result<Vec<f32>>;
    /// Short name for logs and the #106 notes: `silero` | `energy`.
    fn name(&self) -> &'static str;
}

/// Fallback detector when the Silero model is unavailable: frame RMS
/// against a fixed threshold (the dictation path's silence threshold),
/// mapped so that `rms == threshold` is probability 0.5.
pub struct EnergyDetector {
    pub threshold: f32,
}

impl Default for EnergyDetector {
    fn default() -> Self {
        Self { threshold: 0.01 }
    }
}

impl SpeechDetector for EnergyDetector {
    fn probabilities(&mut self, samples: &[f32]) -> anyhow::Result<Vec<f32>> {
        let thr = self.threshold.max(1e-6);
        Ok(samples
            .chunks(FRAME)
            .map(|f| (crate::audio::resample::rms(f) / thr * 0.5).min(1.0))
            .collect())
    }

    fn name(&self) -> &'static str {
        "energy"
    }
}

/// Segmenter tuning. Defaults are the 0.7 engine's (see the #106 notes).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SegmenterParams {
    /// A frame at or above this probability is speech (Silero's 0.5).
    pub threshold: f32,
    /// Below this a frame is silence; in between keeps the current state
    /// (hysteresis, as in Silero's reference `get_speech_timestamps`).
    pub neg_threshold: f32,
    /// Segments with less speech than this are dropped as noise.
    pub min_speech_ms: u32,
    /// Silence this long ends a segment ("end of speech") once the segment
    /// holds at least `min_segment_ms` of audio.
    pub min_silence_ms: u32,
    /// Below this length a segment only ends on `long_silence_ms`: rhetorical
    /// pauses ("Ask not! … what your country…") would otherwise leave 1–2 s
    /// fragments that STT transcribes with too little context.
    pub min_segment_ms: u32,
    /// Silence this long always ends a segment, however short.
    pub long_silence_ms: u32,
    /// Audio kept before the first and after the last speech frame.
    pub pad_ms: u32,
    /// Hard cap on a segment's length (30 s: every engine's comfort zone).
    pub max_segment_ms: u32,
    /// At the cap, cut at the quietest frame within this last stretch.
    pub cap_search_ms: u32,
}

impl Default for SegmenterParams {
    fn default() -> Self {
        Self {
            threshold: 0.5,
            neg_threshold: 0.35,
            min_speech_ms: 250,
            min_silence_ms: 800,
            min_segment_ms: 5_000,
            long_silence_ms: 2_000,
            pad_ms: 200,
            max_segment_ms: 30_000,
            cap_search_ms: 5_000,
        }
    }
}

/// A finished segment: 16 kHz mono samples starting at `start` on the
/// source's sample clock.
#[derive(Debug, Clone, PartialEq)]
pub struct SegmentAudio {
    pub start: u64,
    pub samples: Vec<f32>,
}

impl SegmentAudio {
    pub fn end(&self) -> u64 {
        self.start + self.samples.len() as u64
    }
}

fn frames(ms: u32) -> usize {
    ms_to_samples(ms).div_ceil(FRAME)
}

/// The VAD state machine. Feed it one frame at a time with its probability.
pub struct Segmenter {
    p: SegmenterParams,
    /// Clock position of the next frame's first sample.
    clock: u64,
    /// Idle pre-roll: the last `pad_ms` of non-speech audio.
    pre: VecDeque<f32>,
    active: bool,
    cur: Vec<f32>,
    cur_probs: Vec<f32>,
    cur_start: u64,
    speech_frames: usize,
    /// Frames since the current silence began (0 while speaking).
    silence_run: usize,
}

impl Segmenter {
    pub fn new(p: SegmenterParams) -> Self {
        Self {
            p,
            clock: 0,
            pre: VecDeque::new(),
            active: false,
            cur: Vec::new(),
            cur_probs: Vec::new(),
            cur_start: 0,
            speech_frames: 0,
            silence_run: 0,
        }
    }

    /// Clock position after everything pushed so far.
    pub fn clock(&self) -> u64 {
        self.clock
    }

    /// Start of the open segment, if one is being collected.
    pub fn open_start(&self) -> Option<u64> {
        self.active.then_some(self.cur_start)
    }

    fn max_samples(&self) -> usize {
        ms_to_samples(self.p.max_segment_ms).max(2 * FRAME)
    }

    /// Push one frame (`FRAME` samples; a shorter final frame is fine).
    /// Returns the segments it completed (usually none, at most two).
    pub fn push(&mut self, frame: &[f32], prob: f32) -> Vec<SegmentAudio> {
        let mut out = Vec::new();
        let speech = prob >= self.p.threshold;
        if !self.active {
            if speech {
                self.active = true;
                self.cur_start = self.clock - self.pre.len() as u64;
                self.cur = self.pre.drain(..).collect();
                self.cur_probs = vec![0.0; self.cur.len().div_ceil(FRAME)];
                self.speech_frames = 0;
                self.silence_run = 0;
            } else {
                self.pre.extend(frame.iter().copied());
                let keep = frames(self.p.pad_ms) * FRAME;
                while self.pre.len() > keep {
                    self.pre.pop_front();
                }
                self.clock += frame.len() as u64;
                return out;
            }
        }

        if self.cur.len() + frame.len() > self.max_samples() {
            out.push(self.cut_at_cap());
        }
        self.cur.extend_from_slice(frame);
        self.cur_probs.push(prob);
        self.clock += frame.len() as u64;
        if speech {
            self.speech_frames += 1;
            self.silence_run = 0;
        } else if prob < self.p.neg_threshold || self.silence_run > 0 {
            // Silence starts on a clearly quiet frame; ambiguous frames
            // after that keep counting (Silero's `temp_end` rule).
            self.silence_run += 1;
        }
        if self.silence_run > 0 && self.silence_run >= self.silence_to_close() {
            out.extend(self.close());
        }
        out
    }

    /// Frames of silence that end the open segment: the short rule once it
    /// holds `min_segment_ms` of audio before the silence, the long one
    /// before that.
    fn silence_to_close(&self) -> usize {
        let content = self.cur_probs.len().saturating_sub(self.silence_run);
        if content >= frames(self.p.min_segment_ms) {
            frames(self.p.min_silence_ms)
        } else {
            frames(self.p.long_silence_ms).max(frames(self.p.min_silence_ms))
        }
    }

    /// End of stream: the open segment, if it holds enough speech.
    pub fn finish(&mut self) -> Option<SegmentAudio> {
        if self.active {
            self.close()
        } else {
            None
        }
    }

    /// Close the open segment at the end of speech: drop the trailing
    /// silence beyond `pad_ms` (it becomes the next pre-roll).
    fn close(&mut self) -> Option<SegmentAudio> {
        let trailing = self.silence_run.saturating_sub(frames(self.p.pad_ms));
        let keep_frames = self.cur_probs.len().saturating_sub(trailing);
        let keep = (keep_frames * FRAME).min(self.cur.len());
        let tail = self.cur.split_off(keep);
        let enough = self.speech_frames >= frames(self.p.min_speech_ms);
        let seg = SegmentAudio {
            start: self.cur_start,
            samples: std::mem::take(&mut self.cur),
        };
        self.active = false;
        self.cur_probs.clear();
        self.speech_frames = 0;
        self.silence_run = 0;
        self.pre = tail.into_iter().collect();
        let keep_pre = frames(self.p.pad_ms) * FRAME;
        while self.pre.len() > keep_pre {
            self.pre.pop_front();
        }
        (enough && !seg.samples.is_empty()).then_some(seg)
    }

    /// The open segment reached the cap mid-speech: emit it up to the
    /// quietest frame of its last `cap_search_ms`, keep the rest open.
    fn cut_at_cap(&mut self) -> SegmentAudio {
        let n = self.cur_probs.len();
        let search = frames(self.p.cap_search_ms).clamp(1, n.max(1));
        let from = n.saturating_sub(search);
        let quietest = (from..n)
            .rev()
            .min_by(|a, b| self.cur_probs[*a].total_cmp(&self.cur_probs[*b]))
            .unwrap_or(n.saturating_sub(1));
        // Keep at least one frame in the emitted part.
        let cut_frames = (quietest + 1).max(1);
        let cut = (cut_frames * FRAME).min(self.cur.len());
        let rest = self.cur.split_off(cut);
        let seg = SegmentAudio {
            start: self.cur_start,
            samples: std::mem::replace(&mut self.cur, rest),
        };
        self.cur_start += cut as u64;
        self.cur_probs.drain(..cut_frames.min(self.cur_probs.len()));
        self.speech_frames = self
            .cur_probs
            .iter()
            .filter(|p| **p >= self.p.threshold)
            .count();
        self.silence_run = self
            .cur_probs
            .iter()
            .rev()
            .take_while(|p| **p < self.p.threshold)
            .count();
        seg
    }
}

/// Buffers arbitrary chunks into whole `FRAME`s for the detector.
#[derive(Default)]
pub struct FrameAligner {
    buf: Vec<f32>,
}

impl FrameAligner {
    pub fn push(&mut self, samples: &[f32]) {
        self.buf.extend_from_slice(samples);
    }

    /// Whole frames available (at least `min_frames` of them), removed from
    /// the buffer; `None` if fewer are buffered.
    pub fn take(&mut self, min_frames: usize) -> Option<Vec<f32>> {
        let whole = self.buf.len() / FRAME;
        if whole == 0 || whole < min_frames {
            return None;
        }
        let rest = self.buf.split_off(whole * FRAME);
        Some(std::mem::replace(&mut self.buf, rest))
    }

    /// End of stream: whatever is left, zero-padded to a whole frame.
    pub fn take_rest(&mut self) -> Option<Vec<f32>> {
        if self.buf.is_empty() {
            return None;
        }
        let mut rest = std::mem::take(&mut self.buf);
        rest.resize(rest.len().div_ceil(FRAME) * FRAME, 0.0);
        Some(rest)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Synthetic stream: (probability, frame count) runs → frames whose
    /// samples encode their clock position, so cuts can be checked exactly.
    fn run(p: SegmenterParams, runs: &[(f32, usize)]) -> (Vec<SegmentAudio>, Segmenter) {
        let mut seg = Segmenter::new(p);
        let mut out = Vec::new();
        let mut pos = 0u64;
        for &(prob, n) in runs {
            for _ in 0..n {
                let frame: Vec<f32> = (0..FRAME).map(|i| (pos + i as u64) as f32).collect();
                pos += FRAME as u64;
                out.extend(seg.push(&frame, prob));
            }
        }
        out.extend(seg.finish());
        (out, seg)
    }

    fn secs(s: f32) -> usize {
        (s * RATE as f32 / FRAME as f32).round() as usize
    }

    #[test]
    fn speech_between_silences_becomes_one_padded_segment() {
        let p = SegmenterParams::default();
        let (segs, _) = run(p, &[(0.0, secs(2.0)), (0.9, secs(3.0)), (0.0, secs(3.0))]);
        assert_eq!(segs.len(), 1);
        let s = &segs[0];
        let speech_start = (secs(2.0) * FRAME) as u64;
        let speech_end = speech_start + (secs(3.0) * FRAME) as u64;
        // Pre-roll of pad_ms before speech, pad after it.
        let pad = (frames(p.pad_ms) * FRAME) as u64;
        assert!(s.start <= speech_start && s.start >= speech_start - pad);
        assert!(s.end() >= speech_end && s.end() <= speech_end + pad + FRAME as u64);
        // Samples are contiguous and aligned with the clock.
        assert_eq!(s.samples[0], s.start as f32);
        assert_eq!(*s.samples.last().unwrap(), (s.end() - 1) as f32);
    }

    #[test]
    fn short_pauses_never_split() {
        let p = SegmenterParams::default();
        let (segs, _) = run(p, &[(0.9, secs(6.0)), (0.1, secs(0.4)), (0.9, secs(2.0))]);
        assert_eq!(segs.len(), 1);
    }

    #[test]
    fn a_pause_ends_a_long_segment_but_a_short_one_needs_a_long_silence() {
        let p = SegmenterParams::default();
        // ≥ min_segment of speech: a 1 s pause is the end of speech.
        let (segs, _) = run(p, &[(0.9, secs(6.0)), (0.1, secs(1.0)), (0.9, secs(1.0))]);
        assert_eq!(segs.len(), 2);
        assert!(segs[0].end() <= segs[1].start);
        // A 2 s utterance: the same pause stays inside (rhetorical pause)…
        let (segs, _) = run(p, &[(0.9, secs(2.0)), (0.1, secs(1.0)), (0.9, secs(2.0))]);
        assert_eq!(segs.len(), 1);
        // …but a long silence ends even a short segment.
        let (segs, _) = run(p, &[(0.9, secs(2.0)), (0.1, secs(2.5)), (0.9, secs(1.0))]);
        assert_eq!(segs.len(), 2);
    }

    #[test]
    fn ambiguous_frames_extend_a_started_silence_but_do_not_start_one() {
        let p = SegmenterParams::default();
        // Ambiguous (0.4) right after speech: not silence yet → one segment.
        let (segs, _) = run(p, &[(0.9, secs(1.0)), (0.4, secs(2.0)), (0.9, secs(1.0))]);
        assert_eq!(segs.len(), 1);
        // A quiet frame starts the silence; ambiguous ones keep counting.
        let (segs, _) = run(
            p,
            &[
                (0.9, secs(1.0)),
                (0.1, 1),
                (0.4, secs(2.5)),
                (0.9, secs(1.0)),
            ],
        );
        assert_eq!(segs.len(), 2);
    }

    #[test]
    fn noise_bursts_shorter_than_min_speech_are_dropped() {
        let p = SegmenterParams::default();
        let (segs, _) = run(p, &[(0.0, secs(1.0)), (0.9, 3), (0.0, secs(2.0))]);
        assert!(segs.is_empty());
    }

    #[test]
    fn continuous_speech_is_capped_at_max_segment_length() {
        let p = SegmenterParams::default();
        let max = ms_to_samples(p.max_segment_ms);
        // 95 s of uninterrupted speech.
        let (segs, _) = run(p, &[(0.9, secs(95.0))]);
        assert!(segs.len() >= 4, "got {} segments", segs.len());
        for s in &segs {
            assert!(
                s.samples.len() <= max,
                "segment of {} samples",
                s.samples.len()
            );
        }
        // Nothing lost, nothing duplicated: segments tile the stream.
        for w in segs.windows(2) {
            assert_eq!(w[0].end(), w[1].start);
        }
        assert_eq!(segs[0].start, 0);
        assert_eq!(segs.last().unwrap().end(), (secs(95.0) * FRAME) as u64);
    }

    #[test]
    fn the_cap_cuts_at_the_quietest_recent_frame() {
        let p = SegmenterParams::default();
        // 27 s speech, one dip (0.45, ambiguous) then more speech past 30 s.
        let (segs, _) = run(p, &[(0.9, secs(27.0)), (0.45, 1), (0.9, secs(10.0))]);
        assert_eq!(segs.len(), 2);
        let dip_end = ((secs(27.0) + 1) * FRAME) as u64;
        assert_eq!(segs[0].end(), dip_end);
        assert_eq!(segs[1].start, dip_end);
    }

    #[test]
    fn clock_and_open_start_track_the_stream() {
        let p = SegmenterParams::default();
        let mut seg = Segmenter::new(p);
        let frame = vec![0.0; FRAME];
        seg.push(&frame, 0.0);
        assert_eq!(seg.clock(), FRAME as u64);
        assert_eq!(seg.open_start(), None);
        seg.push(&frame, 0.9);
        // Pre-roll included: the open segment starts at the first frame.
        assert_eq!(seg.open_start(), Some(0));
        assert_eq!(seg.clock(), 2 * FRAME as u64);
    }

    #[test]
    fn energy_detector_maps_threshold_to_half() {
        let mut d = EnergyDetector { threshold: 0.01 };
        let mut samples = vec![0.0; FRAME];
        samples.extend(vec![0.01; FRAME]);
        samples.extend(vec![0.5; FRAME]);
        let probs = d.probabilities(&samples).unwrap();
        assert_eq!(probs.len(), 3);
        assert_eq!(probs[0], 0.0);
        assert!((probs[1] - 0.5).abs() < 1e-3);
        assert_eq!(probs[2], 1.0);
    }

    #[test]
    fn aligner_yields_whole_frames_and_pads_the_rest() {
        let mut a = FrameAligner::default();
        a.push(&vec![1.0; FRAME + 10]);
        assert!(a.take(2).is_none());
        assert_eq!(a.take(1).unwrap().len(), FRAME);
        a.push(&[1.0; 5]);
        let rest = a.take_rest().unwrap();
        assert_eq!(rest.len(), FRAME);
        assert_eq!(rest.iter().filter(|s| **s == 1.0).count(), 15);
        assert!(a.take_rest().is_none());
    }
}
