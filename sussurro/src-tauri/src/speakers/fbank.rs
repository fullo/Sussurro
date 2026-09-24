//! 80-dim Kaldi-style log mel filterbank ("fbank") over `rustfft`, as the
//! WeSpeaker model expects it (#107): 25 ms frames every 10 ms, no dither,
//! DC removal, pre-emphasis 0.97, Hamming window, 512-point power spectrum,
//! 80 triangular mel bins from 20 Hz to Nyquist, natural log, `snip_edges`.
//!
//! Validated against `kaldi-native-fbank` (the C++ port of Kaldi's
//! `compute-fbank-feats`): the spike measured max |diff| ≈ 2e-4 on 10 s of
//! AMI audio, and the unit test below checks a synthetic signal against a
//! reference computed by that library.

use rustfft::{num_complex::Complex32, Fft, FftPlanner};
use std::sync::Arc;

/// Sample rate the features (and the engine's audio) are at.
pub const SAMPLE_RATE: usize = 16_000;
/// 25 ms.
pub const FRAME_LEN: usize = 400;
/// 10 ms.
pub const FRAME_SHIFT: usize = 160;
const NFFT: usize = 512;
/// Mel bins per frame.
pub const NUM_MEL: usize = 80;
const LOW_FREQ: f64 = 20.0;
const PREEMPH: f32 = 0.97;

fn mel_scale(f: f64) -> f64 {
    1127.0 * (1.0 + f / 700.0).ln()
}

/// Number of frames `samples` samples give (`snip_edges`: only whole frames).
pub fn num_frames(samples: usize) -> usize {
    if samples < FRAME_LEN {
        0
    } else {
        1 + (samples - FRAME_LEN) / FRAME_SHIFT
    }
}

/// A reusable fbank extractor (window, mel weights and FFT plan are built
/// once).
pub struct Fbank {
    window: [f32; FRAME_LEN],
    /// Per mel bin: first FFT index and the triangle's weights from there.
    mel: Vec<(usize, Vec<f32>)>,
    fft: Arc<dyn Fft<f32>>,
}

impl Default for Fbank {
    fn default() -> Self {
        Self::new()
    }
}

impl Fbank {
    pub fn new() -> Self {
        let a = 2.0 * std::f64::consts::PI / (FRAME_LEN - 1) as f64;
        let mut window = [0f32; FRAME_LEN];
        for (i, w) in window.iter_mut().enumerate() {
            *w = (0.54 - 0.46 * (a * i as f64).cos()) as f32;
        }
        // Kaldi MelBanks: bins over FFT indices 0..NFFT/2 (Nyquist excluded).
        let width = SAMPLE_RATE as f64 / NFFT as f64;
        let (lo, hi) = (mel_scale(LOW_FREQ), mel_scale(SAMPLE_RATE as f64 / 2.0));
        let delta = (hi - lo) / (NUM_MEL + 1) as f64;
        let mel = (0..NUM_MEL)
            .map(|b| {
                let (l, c, r) = (
                    lo + b as f64 * delta,
                    lo + (b + 1) as f64 * delta,
                    lo + (b + 2) as f64 * delta,
                );
                let mut first = None;
                let mut weights = Vec::new();
                for i in 0..NFFT / 2 {
                    let m = mel_scale(width * i as f64);
                    if m > l && m < r {
                        first.get_or_insert(i);
                        let w = if m <= c { (m - l) / (c - l) } else { (r - m) / (r - c) };
                        weights.push(w as f32);
                    }
                }
                (first.unwrap_or(0), weights)
            })
            .collect();
        let fft = FftPlanner::<f32>::new().plan_fft_forward(NFFT);
        Self { window, mel, fft }
    }

    /// Log mel energies, one row of [`NUM_MEL`] per frame. `x` is in the
    /// scale the model was trained on (WeSpeaker: int16 range, see
    /// [`wespeaker_features`]).
    pub fn compute(&self, x: &[f32]) -> Vec<[f32; NUM_MEL]> {
        let n = num_frames(x.len());
        let mut out = Vec::with_capacity(n);
        let mut buf = vec![Complex32::new(0.0, 0.0); NFFT];
        let mut scratch = vec![Complex32::new(0.0, 0.0); self.fft.get_inplace_scratch_len()];
        let mut fr = [0f32; FRAME_LEN];
        for f in 0..n {
            fr.copy_from_slice(&x[f * FRAME_SHIFT..f * FRAME_SHIFT + FRAME_LEN]);
            let mean = fr.iter().sum::<f32>() / FRAME_LEN as f32;
            fr.iter_mut().for_each(|v| *v -= mean);
            for i in (1..FRAME_LEN).rev() {
                fr[i] -= PREEMPH * fr[i - 1];
            }
            fr[0] -= PREEMPH * fr[0];
            for (i, b) in buf.iter_mut().enumerate() {
                let v = if i < FRAME_LEN { fr[i] * self.window[i] } else { 0.0 };
                *b = Complex32::new(v, 0.0);
            }
            self.fft.process_with_scratch(&mut buf, &mut scratch);
            let mut row = [0f32; NUM_MEL];
            for (r, (first, w)) in row.iter_mut().zip(&self.mel) {
                let e: f32 = w
                    .iter()
                    .enumerate()
                    .map(|(j, wt)| wt * buf[first + j].norm_sqr())
                    .sum();
                *r = e.max(f32::EPSILON).ln();
            }
            out.push(row);
        }
        out
    }
}

/// The WeSpeaker model's input for 16 kHz mono samples in `[-1, 1]`: fbank
/// on int16-scaled samples, then per-utterance mean normalisation (CMN),
/// flattened row-major (`frames × 80`). Returns `(frames, data)`.
pub fn wespeaker_features(fbank: &Fbank, samples: &[f32]) -> (usize, Vec<f32>) {
    let scaled: Vec<f32> = samples.iter().map(|v| v * 32768.0).collect();
    let rows = fbank.compute(&scaled);
    let t = rows.len();
    if t == 0 {
        return (0, Vec::new());
    }
    let mut mean = [0f32; NUM_MEL];
    for r in &rows {
        for (m, v) in mean.iter_mut().zip(r) {
            *m += v / t as f32;
        }
    }
    let mut flat = Vec::with_capacity(t * NUM_MEL);
    for r in &rows {
        flat.extend(r.iter().zip(&mean).map(|(v, m)| v - m));
    }
    (t, flat)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The synthetic test signal (int16 values, reproducible bit for bit in
    /// Python): LCG noise plus two sawtooths, 0.3 s.
    fn test_signal() -> Vec<f32> {
        let mut state: u64 = 12345;
        (0..4800u64)
            .map(|n| {
                state = (state * 1_103_515_245 + 12_345) % (1 << 31);
                let noise = ((state >> 8) % 4001) as i64 - 2000;
                let saw = (((n * 523) % 1600) as i64 - 800) * 8;
                let slow = ((n % 160) as i64 - 80) * 50;
                (noise + saw + slow).clamp(-32768, 32767) as f32 / 32768.0
            })
            .collect()
    }

    /// Reference: `kaldi-native-fbank` 1.22.3 on [`test_signal`] × 32768
    /// (`dither=0`, `window_type=hamming`, `snip_edges=True`,
    /// `num_bins=80`), 28 frames × 80 little-endian f32. Generated with:
    ///
    /// ```python
    /// o = knf.FbankOptions(); o.frame_opts.dither = 0.0
    /// o.frame_opts.window_type = 'hamming'; o.mel_opts.num_bins = 80
    /// f = knf.OnlineFbank(o); f.accept_waveform(16000, (x * 32768.0).tolist())
    /// f.input_finished()
    /// ```
    const REFERENCE: &[u8] = include_bytes!("testdata/fbank_ref_hamming.f32");

    #[test]
    fn fbank_matches_kaldi_native_fbank() {
        let x: Vec<f32> = test_signal().iter().map(|v| v * 32768.0).collect();
        let ours = Fbank::new().compute(&x);
        let reference: Vec<f32> = REFERENCE
            .chunks_exact(4)
            .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
            .collect();
        assert_eq!(ours.len(), 28);
        assert_eq!(reference.len(), ours.len() * NUM_MEL);
        let mut max = 0f32;
        for (row, want) in ours.iter().zip(reference.chunks_exact(NUM_MEL)) {
            for (a, b) in row.iter().zip(want) {
                max = max.max((a - b).abs());
            }
        }
        // Log energies are ~10–25 here; the spike saw ≤ 4.4e-4 on speech.
        assert!(max < 1e-3, "max |diff| {max}");
    }

    #[test]
    fn frame_count_follows_snip_edges() {
        assert_eq!(num_frames(0), 0);
        assert_eq!(num_frames(399), 0);
        assert_eq!(num_frames(400), 1);
        assert_eq!(num_frames(559), 1);
        assert_eq!(num_frames(560), 2);
        assert_eq!(num_frames(16_000), 98);
    }

    #[test]
    fn wespeaker_features_are_mean_normalised() {
        let fb = Fbank::new();
        let (t, flat) = wespeaker_features(&fb, &test_signal());
        assert_eq!(t, 28);
        assert_eq!(flat.len(), 28 * NUM_MEL);
        for k in 0..NUM_MEL {
            let mean: f32 = (0..t).map(|i| flat[i * NUM_MEL + k]).sum::<f32>() / t as f32;
            assert!(mean.abs() < 1e-4, "bin {k} mean {mean}");
        }
        assert_eq!(wespeaker_features(&fb, &[0.0; 100]), (0, Vec::new()));
    }

    /// The spike's own check (#107) on 10 s of AMI speech, when its files
    /// are around: `SUSSURRO_FBANK_REF=<dir>` holding
    /// `ES2004a_5min.Mix-Headset.wav`-derived `ref_fbank_hamming_32768.f32`
    /// and the raw 16-bit samples as `wav10s.s16`.
    #[test]
    #[ignore = "needs the spike's AMI reference files"]
    fn fbank_matches_the_spike_reference_on_speech() {
        let dir = std::path::PathBuf::from(std::env::var("SUSSURRO_FBANK_REF").unwrap());
        let raw = std::fs::read(dir.join("wav10s.s16")).unwrap();
        let x: Vec<f32> = raw
            .chunks_exact(2)
            .map(|c| i16::from_le_bytes([c[0], c[1]]) as f32)
            .collect();
        let ours = Fbank::new().compute(&x);
        let reference: Vec<f32> = std::fs::read(dir.join("ref_fbank_hamming_32768.f32"))
            .unwrap()
            .chunks_exact(4)
            .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
            .collect();
        assert_eq!(reference.len(), ours.len() * NUM_MEL);
        let max = ours
            .iter()
            .flatten()
            .zip(&reference)
            .map(|(a, b)| (a - b).abs())
            .fold(0f32, f32::max);
        eprintln!("max |diff| {max:.2e} over {} frames", ours.len());
        assert!(max < 1e-3);
    }
}
