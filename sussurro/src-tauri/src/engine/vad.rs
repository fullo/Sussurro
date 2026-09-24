//! Silero VAD through whisper.cpp's built-in `whisper_vad` (E7), exposed by
//! `whisper-rs` 0.16 as [`WhisperVadContext`].
//!
//! Streaming notes (#106): `whisper_vad_detect_speech` computes one speech
//! probability per 512-sample window (32 ms at 16 kHz) but **resets the
//! Silero LSTM state on every call** (`ggml_backend_buffer_clear` at the top
//! of the function). Feeding a stream in 1 s pushes would therefore start
//! every push "cold". [`SileroDetector`] re-feeds the last
//! [`WARMUP_SAMPLES`] of the previous push in front of each new one and
//! discards their probabilities, so every reported frame has at least
//! ~0.5 s of recurrent context — at the price of re-running those frames
//! (≈1.5× the VAD work with 1 s pushes; Silero stays far below real time).
//! The segment logic itself (thresholds, min silence, 30 s cap) is ours
//! (`segmenter.rs`): whisper.cpp's `whisper_vad_segments_from_probs` only
//! works on a whole buffer, not on a stream.

use super::segmenter::{SpeechDetector, FRAME};
use anyhow::{anyhow, Context, Result};
use std::path::Path;
use whisper_rs::{WhisperVadContext, WhisperVadContextParams};

/// Audio re-fed before each push to warm the LSTM state (16 frames, 0.5 s).
pub const WARMUP_SAMPLES: usize = 16 * FRAME;

pub struct SileroDetector {
    ctx: WhisperVadContext,
    warm: Vec<f32>,
    /// Kept to load one more context for another channel ([`fork`]).
    ///
    /// [`fork`]: SpeechDetector::fork
    model: std::path::PathBuf,
}

impl SileroDetector {
    pub fn load(model: &Path) -> Result<Self> {
        let path = model
            .to_str()
            .context("VAD model path is not valid UTF-8")?;
        let mut params = WhisperVadContextParams::new();
        // Tiny model: CPU is faster than a GPU round trip, and leaves the
        // GPU to whisper.
        params.set_use_gpu(false);
        params.set_n_threads(2);
        let ctx = WhisperVadContext::new(path, params)
            .map_err(|e| anyhow!("failed to load the Silero VAD model: {e}"))?;
        Ok(Self {
            ctx,
            warm: Vec::new(),
            model: model.to_path_buf(),
        })
    }
}

impl SpeechDetector for SileroDetector {
    fn probabilities(&mut self, samples: &[f32]) -> Result<Vec<f32>> {
        if samples.is_empty() {
            return Ok(Vec::new());
        }
        let mut buf = Vec::with_capacity(self.warm.len() + samples.len());
        buf.extend_from_slice(&self.warm);
        buf.extend_from_slice(samples);
        self.ctx
            .detect_speech(&buf)
            .map_err(|e| anyhow!("Silero VAD failed: {e}"))?;
        let probs = self.ctx.probabilities();
        let skip = self.warm.len() / FRAME;
        let want = samples.len().div_ceil(FRAME);
        if probs.len() != skip + want {
            // A model with another window size would silently misalign
            // every timestamp — refuse instead.
            anyhow::bail!(
                "Silero VAD returned {} probabilities for {} samples (expected {})",
                probs.len(),
                buf.len(),
                skip + want
            );
        }
        let out = probs[skip..].to_vec();
        let keep = WARMUP_SAMPLES.min(buf.len());
        self.warm = buf.split_off(buf.len() - keep);
        Ok(out)
    }

    fn fork(&self) -> Result<Box<dyn SpeechDetector>> {
        Ok(Box::new(Self::load(&self.model)?))
    }

    fn name(&self) -> &'static str {
        "silero"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::segmenter::{Segmenter, SegmenterParams};

    /// Needs the Silero model and a speech WAV. Measures the VAD cost on a
    /// stream fed in 1 s pushes and prints the segments (#106 notes):
    /// SUSSURRO_TEST_VAD_MODEL=/path/ggml-silero-v5.1.2.bin
    /// SUSSURRO_TEST_WAV=/path/speech.wav
    /// cargo test silero_streaming -- --ignored --nocapture
    #[test]
    #[ignore]
    fn silero_streaming_boundaries_and_cost() {
        let model = std::env::var("SUSSURRO_TEST_VAD_MODEL").expect("set SUSSURRO_TEST_VAD_MODEL");
        let wav = std::env::var("SUSSURRO_TEST_WAV").expect("set SUSSURRO_TEST_WAV");
        let audio = crate::audio::decode::decode_to_16k_mono(Path::new(&wav)).unwrap();
        let mut vad = SileroDetector::load(Path::new(&model)).unwrap();
        let mut seg = Segmenter::new(SegmenterParams::default());
        let started = std::time::Instant::now();
        let mut segs = Vec::new();
        let push = 32 * FRAME;
        for chunk in audio.chunks(push) {
            let mut chunk = chunk.to_vec();
            chunk.resize(chunk.len().div_ceil(FRAME) * FRAME, 0.0);
            let probs = vad.probabilities(&chunk).unwrap();
            for (frame, p) in chunk.chunks(FRAME).zip(probs) {
                segs.extend(seg.push(frame, p));
            }
        }
        segs.extend(seg.finish());
        let secs = audio.len() as f64 / 16_000.0;
        let took = started.elapsed().as_secs_f64();
        println!(
            "silero: {secs:.1} s of audio in {took:.3} s ({:.2} ms per audio second, {:.0}x real time)",
            took * 1000.0 / secs,
            secs / took
        );
        for s in &segs {
            println!(
                "segment {:>8.2} → {:>8.2} s ({:.2} s)",
                s.start as f64 / 16_000.0,
                s.end() as f64 / 16_000.0,
                s.samples.len() as f64 / 16_000.0
            );
        }
        assert!(!segs.is_empty());
    }
}
