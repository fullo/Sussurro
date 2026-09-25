//! Band-limited rate conversion for generated speech (#256): Pocket TTS
//! speaks at 24 kHz, saved speech is 16 kHz Ogg Opus like every other audio
//! file of the archive (so the `sussurro-audio:` scheme, its Opus reader
//! and the virtual WAV of #248 play it unchanged).
//!
//! Why not the capture path's [`crate::audio::resample::StreamResampler`]:
//! it interpolates linearly with no low-pass, fine for a microphone going
//! to the recogniser but not for speech people listen to — 24 → 16 kHz
//! would fold the 8–12 kHz band (every "s") back onto 4–8 kHz. This is a
//! windowed-sinc polyphase resampler for any rational ratio: output sample
//! `n` sits at input position `n · from / to`; it is the sum of the
//! [`HALF_TAPS`] × 2 nearest inputs weighted by a Blackman-windowed sinc
//! whose cut-off is just under the lower Nyquist rate. One kernel per
//! phase (2 for 24 → 16 kHz), each normalised to unity gain at DC.
//! Streaming: `push` any number of samples, `flush` at the end; the output
//! is identical however the input is cut. Pure.

/// Input samples on each side of an output sample: a 128-tap kernel, a
/// transition band of about 1 kHz at 24 kHz.
pub const HALF_TAPS: usize = 64;
/// Cut-off as a fraction of the lower Nyquist rate (the transition band
/// ends near it, so little aliases back).
const CUTOFF: f64 = 0.94;

fn gcd(a: u64, b: u64) -> u64 {
    if b == 0 {
        a
    } else {
        gcd(b, a % b)
    }
}

/// Streaming rational resampler (see the module docs).
pub struct Resampler {
    /// Output rate / input rate = `up / down`, reduced.
    up: u64,
    down: u64,
    /// `kernels[p][j]` weighs input `i - HALF_TAPS + 1 + j` for an output at
    /// `i + p / up`.
    kernels: Vec<Vec<f32>>,
    /// Input not yet consumed; `buf[0]` is absolute input index `base`.
    buf: Vec<f32>,
    base: u64,
    /// Input samples received so far.
    received: u64,
    /// Index of the next output sample.
    next: u64,
    passthrough: bool,
}

impl Resampler {
    pub fn new(from_rate: u32, to_rate: u32) -> Self {
        let (from, to) = (u64::from(from_rate.max(1)), u64::from(to_rate.max(1)));
        let g = gcd(from, to);
        let (up, down) = (to / g, from / g);
        // Cut-off in cycles per input sample.
        let fc = 0.5 * CUTOFF * (up as f64 / down as f64).min(1.0);
        let h = HALF_TAPS as f64;
        let kernels = if up == down {
            Vec::new()
        } else {
            (0..up)
                .map(|p| {
                    let frac = p as f64 / up as f64;
                    let mut k: Vec<f64> = (0..2 * HALF_TAPS)
                        .map(|j| {
                            // Distance from the output point to this input.
                            let x = (j as f64 - h + 1.0) - frac;
                            let sinc = if x.abs() < 1e-12 {
                                1.0
                            } else {
                                let a = std::f64::consts::PI * 2.0 * fc * x;
                                a.sin() / a
                            };
                            let w = if x.abs() >= h {
                                0.0
                            } else {
                                let t = std::f64::consts::PI * x / h;
                                0.42 + 0.5 * t.cos() + 0.08 * (2.0 * t).cos()
                            };
                            sinc * w
                        })
                        .collect();
                    let sum: f64 = k.iter().sum();
                    for v in &mut k {
                        *v /= sum;
                    }
                    k.into_iter().map(|v| v as f32).collect()
                })
                .collect()
        };
        Self {
            up,
            down,
            kernels,
            buf: Vec::new(),
            base: 0,
            received: 0,
            next: 0,
            passthrough: from == to,
        }
    }

    /// Input position of output `n`: `(integer input index, phase)`.
    fn at(&self, n: u64) -> (u64, usize) {
        let t = n * self.down;
        (t / self.up, (t % self.up) as usize)
    }

    fn produce(&mut self, available: u64, out: &mut Vec<f32>) {
        loop {
            let (i, p) = self.at(self.next);
            // The last input this output needs.
            if i + HALF_TAPS as u64 > available {
                break;
            }
            let first = i as i64 - HALF_TAPS as i64 + 1;
            let mut acc = 0f32;
            for (j, w) in self.kernels[p].iter().enumerate() {
                let idx = first + j as i64;
                if idx < self.base as i64 {
                    continue; // before the start: silence
                }
                if let Some(x) = self.buf.get((idx - self.base as i64) as usize) {
                    acc += x * w;
                }
            }
            out.push(acc);
            self.next += 1;
        }
        // Drop input no later output can reach.
        let (i, _) = self.at(self.next);
        let keep_from = (i + 1).saturating_sub(HALF_TAPS as u64);
        if keep_from > self.base {
            let drop = ((keep_from - self.base) as usize).min(self.buf.len());
            self.buf.drain(..drop);
            self.base += drop as u64;
        }
    }

    /// Feed input; returns every output sample that became computable.
    pub fn push(&mut self, input: &[f32]) -> Vec<f32> {
        self.received += input.len() as u64;
        if self.passthrough {
            self.next = self.received;
            return input.to_vec();
        }
        self.buf.extend_from_slice(input);
        let mut out = Vec::with_capacity(input.len() * self.up as usize / self.down as usize + 1);
        let available = self.base + self.buf.len() as u64;
        // `available` counts samples; the last index is `available - 1`.
        self.produce(available.saturating_sub(1), &mut out);
        out
    }

    /// The rest of the output, as if the input went on in silence: the
    /// total output is `ceil(input · to / from)` samples.
    pub fn flush(&mut self) -> Vec<f32> {
        if self.passthrough {
            return Vec::new();
        }
        let total = (self.received * self.up).div_ceil(self.down);
        let mut out = Vec::new();
        self.buf.extend(std::iter::repeat_n(0.0, HALF_TAPS + 1));
        let available = self.base + self.buf.len() as u64;
        self.produce(available - 1, &mut out);
        out.truncate(total.saturating_sub(self.next - out.len() as u64) as usize);
        self.next = total;
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(rate: u32, hz: f64, n: usize) -> Vec<f32> {
        (0..n)
            .map(|i| (0.5 * (2.0 * std::f64::consts::PI * hz * i as f64 / rate as f64).sin()) as f32)
            .collect()
    }

    fn rms(x: &[f32]) -> f32 {
        (x.iter().map(|v| v * v).sum::<f32>() / x.len().max(1) as f32).sqrt()
    }

    fn whole(from: u32, to: u32, input: &[f32]) -> Vec<f32> {
        let mut r = Resampler::new(from, to);
        let mut out = r.push(input);
        out.extend(r.flush());
        out
    }

    #[test]
    fn equal_rates_pass_through() {
        let x = tone(16_000, 440.0, 1000);
        assert_eq!(whole(16_000, 16_000, &x), x);
    }

    #[test]
    fn output_length_follows_the_ratio() {
        for (from, to, n) in [
            (24_000, 16_000, 24_000),
            (24_000, 16_000, 1),
            (24_000, 16_000, 1001),
            (1_000, 16_000, 37),
            (22_050, 16_000, 5000),
        ] {
            let out = whole(from, to, &vec![0.1; n]);
            let want = (n as u64 * to as u64).div_ceil(from as u64) as usize;
            assert_eq!(out.len(), want, "{from}->{to} n={n}");
        }
        assert!(whole(24_000, 16_000, &[]).is_empty());
    }

    #[test]
    fn dc_and_speech_band_are_kept() {
        let out = whole(24_000, 16_000, &vec![0.5; 4800]);
        // Away from the edges (where the input starts/ends against silence).
        for &v in &out[200..3000] {
            assert!((v - 0.5).abs() < 1e-3, "{v}");
        }
        let out = whole(24_000, 16_000, &tone(24_000, 1000.0, 24_000));
        let want = tone(16_000, 1000.0, 16_000);
        let (a, b) = (&out[500..15_000], &want[500..15_000]);
        let err: f32 = a.iter().zip(b).map(|(x, y)| (x - y).abs()).fold(0.0, f32::max);
        assert!(err < 0.01, "1 kHz must come through in phase: max error {err}");
    }

    #[test]
    fn content_above_the_new_nyquist_does_not_fold_back() {
        // 10 kHz at 24 kHz would alias to 6 kHz at 16 kHz.
        let out = whole(24_000, 16_000, &tone(24_000, 10_000.0, 24_000));
        let r = rms(&out[500..15_000]);
        assert!(r < 0.005, "aliased energy {r}");
        // 7 kHz (below 8 kHz, above the transition) mostly survives.
        let out = whole(24_000, 16_000, &tone(24_000, 7000.0, 24_000));
        assert!(rms(&out[500..15_000]) > 0.3);
    }

    #[test]
    fn chunked_input_gives_the_same_output() {
        let x = tone(24_000, 3000.0, 10_000);
        let ref_out = whole(24_000, 16_000, &x);
        let mut r = Resampler::new(24_000, 16_000);
        let mut out = Vec::new();
        for piece in x.chunks(333) {
            out.extend(r.push(piece));
        }
        out.extend(r.flush());
        assert_eq!(out, ref_out);
    }

    #[test]
    fn upsampling_works_too() {
        let out = whole(1_000, 16_000, &vec![0.5; 100]);
        assert_eq!(out.len(), 1600);
        assert!((out[800] - 0.5).abs() < 1e-3, "{}", out[800]);
    }
}
