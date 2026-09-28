//! The watermark layer of generated audio (#257, P21, E17): Meta's
//! AudioSeal 0.2, 16-bit base models, in our own ONNX export
//! ([`super::catalog::WATERMARK_REPO`]) through the app's `ort`.
//!
//! **Marking ("M16")**: the models work at 16 kHz, speech is written at
//! 24 kHz. [`M16`] resamples the audio to 16 kHz, lets the generator compute
//! the watermark there, resamples the watermark back to the audio's rate
//! and adds it — the audio itself is never resampled. So one 16 kHz
//! detector reads every file the app writes (spike #240 measured it through
//! the app's own Opus: 22/22 clips with the payload). The payload is
//! [`PAYLOAD`], one fixed Sussurro code — never per user or per install: it
//! must not identify anyone.
//!
//! **Windows**: the graphs take any length that is a multiple of [`HOP`]
//! (320 samples, the SEANet stride); shorter pieces are zero-padded and the
//! padding cut off the output. Memory grows with the window (~260 MB for
//! the generator at 10 s, ~1 GB for a minute), so both models run on
//! windows of at most [`WINDOW`] (10 s at 16 kHz).
//!
//! **Detection** ([`Detection`]): per window, `prob` gives a probability
//! per sample and `bits` the probability of each payload bit being 1 (the
//! sigmoid of the window's mean logit). Over a whole file the frame score
//! is the share of samples with `prob > 0.5`, and each bit is the sign of
//! the length-weighted mean logit — the same as one pass over the whole
//! file. [`Verdict`] follows E17: found = at least half the frames **and**
//! at most two of the 16 bits wrong; a high frame score with the wrong
//! payload is *inconclusive* (pure tones reach the frame score, 4.7).

use super::resample::Resampler;
use anyhow::{anyhow, bail, Context, Result};
use serde::Serialize;
use std::path::Path;

/// The models' sample rate.
pub const RATE: u32 = 16_000;
/// Input lengths must be a multiple of this (8·5·4·2).
pub const HOP: usize = 320;
/// Longest window a model runs on: 10 s at 16 kHz (a multiple of [`HOP`]).
pub const WINDOW: usize = 160_000;
/// Payload bits.
pub const BITS: usize = 16;
/// **The Sussurro code**, written into every generated file: the same for
/// every user and install (E17). Most significant bit first, it is the
/// message the export script checks parity with.
pub const PAYLOAD: u16 = 0xB2E5;
/// A frame counts as marked above this probability.
pub const FRAME_THRESHOLD: f32 = 0.5;
/// "Found" needs at least this share of marked frames.
pub const FOUND_FRACTION: f64 = 0.5;
/// …and at most this many payload bits wrong.
pub const MAX_BIT_ERRORS: u32 = 2;
/// Intra-op threads of each model (the spike's measurements: marking
/// 3.2–3.4 s, detection 1.2–2.0 s per minute of audio on an M-series Mac).
const THREADS: usize = 2;

/// [`PAYLOAD`] as the generator's `message` input, most significant bit
/// first.
pub fn payload_bits() -> [i64; BITS] {
    std::array::from_fn(|i| i64::from((PAYLOAD >> (BITS - 1 - i)) & 1))
}

/// `n` rounded up to a multiple of [`HOP`].
pub fn padded_len(n: usize) -> usize {
    n.div_ceil(HOP) * HOP
}

/// `[start, end)` windows of at most [`WINDOW`] covering `0..n`.
pub fn windows(n: usize) -> Vec<(usize, usize)> {
    (0..n)
        .step_by(WINDOW)
        .map(|s| (s, (s + WINDOW).min(n)))
        .collect()
}

/// `audio` zero-padded to a multiple of [`HOP`].
fn padded(audio: &[f32]) -> Vec<f32> {
    let mut v = audio.to_vec();
    v.resize(padded_len(audio.len()), 0.0);
    v
}

// ---- the models, behind seams tests fill ------------------------------------

/// Computes the watermark of 16 kHz audio (length a multiple of [`HOP`],
/// at most [`WINDOW`]), same length, to be added to it.
pub trait WatermarkModel: Send {
    fn watermark(&mut self, audio: &[f32]) -> Result<Vec<f32>>;
}

/// Reads 16 kHz audio (length a multiple of [`HOP`], at most [`WINDOW`]):
/// the per-sample probability of the mark and each payload bit's
/// probability of being 1.
pub trait DetectorModel {
    fn detect(&mut self, audio: &[f32]) -> Result<(Vec<f32>, [f32; BITS])>;
}

/// The watermark of `audio` (any length ≤ [`WINDOW`]): padded to the hop,
/// run, the padding cut off.
pub fn watermark_window(model: &mut dyn WatermarkModel, audio: &[f32]) -> Result<Vec<f32>> {
    if audio.is_empty() {
        return Ok(Vec::new());
    }
    if audio.len() > WINDOW {
        bail!("watermark windows are at most {WINDOW} samples");
    }
    let mut w = model.watermark(&padded(audio))?;
    if w.len() != padded_len(audio.len()) {
        bail!(
            "the watermark model returned {} samples for {}",
            w.len(),
            padded_len(audio.len())
        );
    }
    w.truncate(audio.len());
    Ok(w)
}

fn session(path: &Path, what: &str) -> Result<ort::session::Session> {
    ort::session::Session::builder()
        .map_err(|e| anyhow!("{what}: {e}"))?
        .with_intra_threads(THREADS)
        .map_err(|e| anyhow!("{what}: {e}"))?
        .with_inter_threads(1)
        .map_err(|e| anyhow!("{what}: {e}"))?
        .commit_from_file(path)
        .map_err(|e| anyhow!("loading the {what} {}: {e}", path.display()))
}

/// The AudioSeal generator through `ort`.
pub struct AudioSealGenerator {
    session: ort::session::Session,
}

impl AudioSealGenerator {
    pub fn load(path: &Path) -> Result<Self> {
        Ok(Self {
            session: session(path, "watermark model")?,
        })
    }
}

impl WatermarkModel for AudioSealGenerator {
    fn watermark(&mut self, audio: &[f32]) -> Result<Vec<f32>> {
        let t = audio.len();
        if t == 0 || !t.is_multiple_of(HOP) || t > WINDOW {
            bail!("watermark model input of {t} samples");
        }
        let x = ort::value::Tensor::from_array(([1usize, 1, t], audio.to_vec()))
            .map_err(|e| anyhow!("watermark model input: {e}"))?;
        let msg = ort::value::Tensor::from_array(([1usize, BITS], payload_bits().to_vec()))
            .map_err(|e| anyhow!("watermark model input: {e}"))?;
        let out = self
            .session
            .run(ort::inputs!["audio" => x, "message" => msg])
            .map_err(|e| anyhow!("watermark model: {e}"))?;
        let (shape, w) = out["watermark"]
            .try_extract_tensor::<f32>()
            .map_err(|e| anyhow!("watermark model output: {e}"))?;
        let dims: Vec<i64> = shape.iter().copied().collect();
        if dims != [1, 1, t as i64] {
            bail!("watermark model returned shape {dims:?}, expected [1, 1, {t}]");
        }
        Ok(w.to_vec())
    }
}

/// The AudioSeal detector through `ort`.
pub struct AudioSealDetector {
    session: ort::session::Session,
}

impl AudioSealDetector {
    pub fn load(path: &Path) -> Result<Self> {
        Ok(Self {
            session: session(path, "watermark detector")?,
        })
    }
}

impl DetectorModel for AudioSealDetector {
    fn detect(&mut self, audio: &[f32]) -> Result<(Vec<f32>, [f32; BITS])> {
        let t = audio.len();
        if t == 0 || !t.is_multiple_of(HOP) || t > WINDOW {
            bail!("watermark detector input of {t} samples");
        }
        let x = ort::value::Tensor::from_array(([1usize, 1, t], audio.to_vec()))
            .map_err(|e| anyhow!("watermark detector input: {e}"))?;
        let out = self
            .session
            .run(ort::inputs!["audio" => x])
            .map_err(|e| anyhow!("watermark detector: {e}"))?;
        let (shape, prob) = out["prob"]
            .try_extract_tensor::<f32>()
            .map_err(|e| anyhow!("watermark detector output: {e}"))?;
        let dims: Vec<i64> = shape.iter().copied().collect();
        if dims != [1, t as i64] {
            bail!("watermark detector returned prob {dims:?}, expected [1, {t}]");
        }
        let prob = prob.to_vec();
        let (shape, bits) = out["bits"]
            .try_extract_tensor::<f32>()
            .map_err(|e| anyhow!("watermark detector output: {e}"))?;
        let dims: Vec<i64> = shape.iter().copied().collect();
        if dims != [1, BITS as i64] {
            bail!("watermark detector returned bits {dims:?}, expected [1, {BITS}]");
        }
        let bits: [f32; BITS] = bits
            .try_into()
            .context("watermark detector: wrong number of bits")?;
        Ok((prob, bits))
    }
}

// ---- marking: M16 ---------------------------------------------------------------

/// Streaming "M16" marking of audio at `rate` (see the module docs):
/// [`Self::push`] any blocks, [`Self::finish`] at the end. The output is the
/// input plus the watermark, sample for sample, only later: up to one
/// [`WINDOW`] of audio (plus the resamplers' look-ahead) waits for its
/// watermark. Every input sample comes out exactly once.
pub struct M16 {
    model: Box<dyn WatermarkModel>,
    /// Audio → 16 kHz, and the watermark back to the audio's rate.
    down: Resampler,
    up: Resampler,
    /// 16 kHz audio waiting for a full window.
    pending: Vec<f32>,
    /// Input not returned yet, and the watermark computed for it so far.
    audio: Vec<f32>,
    marks: Vec<f32>,
}

impl M16 {
    pub fn new(model: Box<dyn WatermarkModel>, rate: u32) -> Self {
        Self {
            model,
            down: Resampler::new(rate, RATE),
            up: Resampler::new(RATE, rate),
            pending: Vec::new(),
            audio: Vec::new(),
            marks: Vec::new(),
        }
    }

    fn mark_pending(&mut self, all: bool) -> Result<()> {
        while self.pending.len() >= WINDOW || (all && !self.pending.is_empty()) {
            let n = self.pending.len().min(WINDOW);
            let w = watermark_window(self.model.as_mut(), &self.pending[..n])?;
            self.pending.drain(..n);
            self.marks.extend(self.up.push(&w));
        }
        Ok(())
    }

    /// The marked audio whose watermark is known.
    fn take_ready(&mut self) -> Vec<f32> {
        let n = self.audio.len().min(self.marks.len());
        let out = self
            .audio
            .drain(..n)
            .zip(self.marks.drain(..n))
            .map(|(a, w)| a + w)
            .collect();
        out
    }

    /// Feed a block; returns the marked audio that became ready (maybe
    /// none).
    pub fn push(&mut self, pcm: &[f32]) -> Result<Vec<f32>> {
        self.audio.extend_from_slice(pcm);
        let down = self.down.push(pcm);
        self.pending.extend(down);
        self.mark_pending(false)?;
        Ok(self.take_ready())
    }

    /// The rest of the marked audio.
    pub fn finish(&mut self) -> Result<Vec<f32>> {
        let tail = self.down.flush();
        self.pending.extend(tail);
        self.mark_pending(true)?;
        let tail = self.up.flush();
        self.marks.extend(tail);
        // The watermark is at least as long as the audio (both resamplers
        // round up); pad defensively, never drop audio.
        if self.marks.len() < self.audio.len() {
            self.marks.resize(self.audio.len(), 0.0);
        }
        let out = self.take_ready();
        self.marks.clear();
        Ok(out)
    }
}

// ---- detection ----------------------------------------------------------------

/// A file's watermark evidence, window by window.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Detection {
    /// Samples read (padding excluded) and those marked.
    pub samples: u64,
    pub marked: u64,
    /// Σ window length · logit(bit probability), per bit.
    logit_sum: [f64; BITS],
}

fn logit(p: f32) -> f64 {
    let p = f64::from(p).clamp(1e-6, 1.0 - 1e-6);
    (p / (1.0 - p)).ln()
}

impl Detection {
    /// Add one window's model output (`prob` may carry padding beyond
    /// `len`).
    pub fn add(&mut self, len: usize, prob: &[f32], bits: &[f32; BITS]) {
        let len = len.min(prob.len());
        if len == 0 {
            return;
        }
        self.samples += len as u64;
        self.marked += prob[..len].iter().filter(|&&p| p > FRAME_THRESHOLD).count() as u64;
        for (s, &b) in self.logit_sum.iter_mut().zip(bits) {
            *s += len as f64 * logit(b);
        }
    }

    /// Run the detector on a window of 16 kHz audio (≤ [`WINDOW`]).
    pub fn run_window(&mut self, model: &mut dyn DetectorModel, audio: &[f32]) -> Result<()> {
        if audio.is_empty() {
            return Ok(());
        }
        if audio.len() > WINDOW {
            bail!("detector windows are at most {WINDOW} samples");
        }
        let (prob, bits) = model.detect(&padded(audio))?;
        if prob.len() != padded_len(audio.len()) {
            bail!("the watermark detector returned {} frames", prob.len());
        }
        self.add(audio.len(), &prob, &bits);
        Ok(())
    }

    /// Share of marked samples.
    pub fn fraction(&self) -> f64 {
        if self.samples == 0 {
            0.0
        } else {
            self.marked as f64 / self.samples as f64
        }
    }

    /// The payload read, most significant bit first.
    pub fn payload(&self) -> u16 {
        self.logit_sum
            .iter()
            .fold(0u16, |acc, &s| (acc << 1) | u16::from(s > 0.0))
    }

    /// Bits that differ from [`PAYLOAD`].
    pub fn bit_errors(&self) -> u32 {
        (self.payload() ^ PAYLOAD).count_ones()
    }

    pub fn verdict(&self) -> Verdict {
        verdict(self.fraction(), self.bit_errors())
    }
}

/// What the watermark layer says (E17 wording rules: never "human", never
/// "another tool's mark").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    /// Half the frames or more marked, payload Sussurro's (≤ 2 bits off).
    Found,
    /// Half the frames or more marked, payload not Sussurro's.
    Inconclusive,
    /// No Sussurro mark found (which says nothing about who made it).
    NotFound,
}

/// E17's rule on a frame score and a payload distance. Pure.
pub fn verdict(fraction: f64, bit_errors: u32) -> Verdict {
    if fraction < FOUND_FRACTION {
        Verdict::NotFound
    } else if bit_errors <= MAX_BIT_ERRORS {
        Verdict::Found
    } else {
        Verdict::Inconclusive
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A stand-in generator: the watermark is a quarter of the input plus a
    /// constant, so tests can see it was computed on 16 kHz audio and
    /// resampled back. Records the lengths it was given.
    pub(crate) struct FakeWatermark {
        pub calls: std::sync::Arc<std::sync::Mutex<Vec<usize>>>,
    }

    impl FakeWatermark {
        pub(crate) fn boxed() -> Box<dyn WatermarkModel> {
            Box::new(Self {
                calls: Default::default(),
            })
        }
    }

    impl WatermarkModel for FakeWatermark {
        fn watermark(&mut self, audio: &[f32]) -> Result<Vec<f32>> {
            assert_eq!(audio.len() % HOP, 0, "padded to the hop");
            assert!(audio.len() <= WINDOW);
            self.calls.lock().unwrap().push(audio.len());
            Ok(audio.iter().map(|x| 0.25 * x + 0.001).collect())
        }
    }

    #[test]
    fn the_payload_is_one_fixed_code() {
        assert_eq!(
            payload_bits(),
            [1, 0, 1, 1, 0, 0, 1, 0, 1, 1, 1, 0, 0, 1, 0, 1],
            "the export script's parity message"
        );
    }

    #[test]
    fn padding_and_windows() {
        assert_eq!(padded_len(0), 0);
        assert_eq!(padded_len(1), 320);
        assert_eq!(padded_len(320), 320);
        assert_eq!(padded_len(321), 640);
        assert_eq!(WINDOW % HOP, 0);
        assert!(windows(0).is_empty());
        assert_eq!(windows(5), [(0, 5)]);
        assert_eq!(windows(WINDOW), [(0, WINDOW)]);
        assert_eq!(
            windows(2 * WINDOW + 7),
            [
                (0, WINDOW),
                (WINDOW, 2 * WINDOW),
                (2 * WINDOW, 2 * WINDOW + 7)
            ]
        );
    }

    #[test]
    fn a_window_is_padded_and_cut() {
        let fake = FakeWatermark {
            calls: Default::default(),
        };
        let calls = fake.calls.clone();
        let mut m: Box<dyn WatermarkModel> = Box::new(fake);
        let w = watermark_window(m.as_mut(), &[0.4; 1000]).unwrap();
        assert_eq!(w.len(), 1000);
        assert!((w[0] - 0.101).abs() < 1e-6);
        assert_eq!(*calls.lock().unwrap(), [1280]);
        assert!(watermark_window(m.as_mut(), &[]).unwrap().is_empty());
        assert!(watermark_window(m.as_mut(), &vec![0.0; WINDOW + 1]).is_err());
    }

    fn tone(rate: u32, hz: f64, n: usize) -> Vec<f32> {
        (0..n)
            .map(|i| {
                (0.3 * (2.0 * std::f64::consts::PI * hz * i as f64 / rate as f64).sin()) as f32
            })
            .collect()
    }

    /// M16 at 24 kHz: every sample comes out once, in order, whatever the
    /// blocks; the generator saw 16 kHz windows of at most 10 s; the added
    /// watermark is the fake's output resampled back to 24 kHz.
    #[test]
    fn m16_marks_24_khz_audio_through_16_khz_windows() {
        let n = 24_000 * 23 + 123; // 23 s and a bit: three windows
        let input = tone(24_000, 440.0, n);
        let fake = FakeWatermark {
            calls: Default::default(),
        };
        let calls = fake.calls.clone();
        let mut m = M16::new(Box::new(fake), 24_000);
        let mut out = Vec::new();
        for block in input.chunks(7_777) {
            out.extend(m.push(block).unwrap());
            assert!(out.len() <= input.len());
        }
        out.extend(m.finish().unwrap());
        assert_eq!(out.len(), input.len(), "no sample lost or added");
        let calls = calls.lock().unwrap().clone();
        assert_eq!(calls.len(), 3, "{calls:?}");
        assert_eq!(&calls[..2], [WINDOW, WINDOW]);
        let n16 = (n as u64 * 2).div_ceil(3) as usize;
        assert_eq!(calls[2], padded_len(n16 - 2 * WINDOW));
        // Away from the window edges the output is 1.25 × the tone + the
        // constant: computed at 16 kHz, back at 24 kHz, in phase.
        for i in (24_000..24_000 * 9).step_by(997) {
            let want = 1.25 * input[i] + 0.001;
            assert!((out[i] - want).abs() < 5e-3, "{i}: {} vs {want}", out[i]);
        }
    }

    #[test]
    fn m16_at_16_khz_and_short_or_empty_input() {
        let mut m = M16::new(FakeWatermark::boxed(), 16_000);
        let out = m.push(&[0.5; 100]).unwrap();
        assert!(out.is_empty(), "held until a window or the end");
        let out = m.finish().unwrap();
        assert_eq!(out.len(), 100);
        assert!((out[50] - (0.5 * 1.25 + 0.001)).abs() < 1e-6);
        let mut m = M16::new(FakeWatermark::boxed(), 24_000);
        assert!(m.finish().unwrap().is_empty());
    }

    #[test]
    fn m16_fails_closed_when_the_model_fails() {
        struct Broken;
        impl WatermarkModel for Broken {
            fn watermark(&mut self, _: &[f32]) -> Result<Vec<f32>> {
                bail!("no model")
            }
        }
        let mut m = M16::new(Box::new(Broken), 24_000);
        assert!(m.push(&[0.1; 24_000]).unwrap().is_empty());
        assert!(m.finish().is_err());
    }

    fn bits_for(code: u16, confidence: f32) -> [f32; BITS] {
        std::array::from_fn(|i| {
            if (code >> (BITS - 1 - i)) & 1 == 1 {
                confidence
            } else {
                1.0 - confidence
            }
        })
    }

    #[test]
    fn detection_reads_frames_and_payload_across_windows() {
        let mut d = Detection::default();
        // A marked window with the payload, padding beyond `len` ignored.
        let mut prob = vec![0.9; 320];
        prob.extend(vec![0.99; 320]);
        d.add(320, &prob, &bits_for(PAYLOAD, 0.9));
        assert_eq!((d.samples, d.marked), (320, 320));
        assert_eq!(d.payload(), PAYLOAD);
        assert_eq!(d.verdict(), Verdict::Found);
        // A longer unmarked window with weak, wrong bits: the frame score
        // halves… and the long window's bits outweigh.
        d.add(640, &[0.1; 640], &bits_for(!PAYLOAD, 0.6));
        assert!((d.fraction() - 1.0 / 3.0).abs() < 1e-9);
        assert_eq!(d.verdict(), Verdict::NotFound);
        assert_eq!(Detection::default().fraction(), 0.0);
        assert_eq!(Detection::default().verdict(), Verdict::NotFound);
    }

    #[test]
    fn verdict_rules() {
        assert_eq!(verdict(0.5, 0), Verdict::Found);
        assert_eq!(verdict(0.97, 2), Verdict::Found);
        assert_eq!(
            verdict(0.97, 3),
            Verdict::Inconclusive,
            "a tone's frame score"
        );
        assert_eq!(verdict(0.89, 16), Verdict::Inconclusive);
        assert_eq!(
            verdict(0.49, 0),
            Verdict::NotFound,
            "the payload alone is not enough"
        );
        assert_eq!(verdict(0.0, 16), Verdict::NotFound);
        let mut d = Detection::default();
        let mut code = PAYLOAD ^ 0b101; // two bits off
        d.add(320, &[0.8; 320], &bits_for(code, 0.7));
        assert_eq!((d.bit_errors(), d.verdict()), (2, Verdict::Found));
        code ^= 0x8000; // a third
        let mut d = Detection::default();
        d.add(320, &[0.8; 320], &bits_for(code, 0.7));
        assert_eq!((d.bit_errors(), d.verdict()), (3, Verdict::Inconclusive));
    }

    #[test]
    fn running_a_window_pads_it() {
        struct Echo;
        impl DetectorModel for Echo {
            fn detect(&mut self, audio: &[f32]) -> Result<(Vec<f32>, [f32; BITS])> {
                assert_eq!(audio.len() % HOP, 0);
                Ok((audio.to_vec(), bits_for(PAYLOAD, 0.8)))
            }
        }
        let mut d = Detection::default();
        d.run_window(&mut Echo, &[0.9; 500]).unwrap();
        assert_eq!((d.samples, d.marked), (500, 500), "padding not counted");
        d.run_window(&mut Echo, &[]).unwrap();
        assert!(d.run_window(&mut Echo, &vec![0.0; WINDOW + 1]).is_err());
    }
}
