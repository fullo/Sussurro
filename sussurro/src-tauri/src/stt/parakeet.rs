use anyhow::{anyhow, Result};
use std::path::Path;
use transcribe_rs::onnx::parakeet::{ParakeetModel, ParakeetParams, TimestampGranularity};
use transcribe_rs::onnx::Quantization;

/// Directory (inside the app models dir) holding the extracted ONNX files.
pub const PARAKEET_DIR: &str = "parakeet-tdt-0.6b-v3-int8";
/// Same archive Handy ships: encoder/decoder int8 ONNX + nemo128 preprocessor.
pub const PARAKEET_URL: &str = "https://blob.handy.computer/parakeet-v3-int8.tar.gz";
pub const PARAKEET_SHA256: &str =
    "43d37191602727524a7d8c6da0eef11c4ba24320f5b4730f1a2497befc2efa77";

/// NVIDIA Parakeet TDT 0.6B v3 via transcribe-rs (ONNX Runtime, CPU).
/// Auto-detects 25 European languages; ignores prompts and language hints.
pub struct ParakeetTranscriber {
    model: ParakeetModel,
}

impl ParakeetTranscriber {
    pub fn load(model_dir: &Path) -> Result<Self> {
        let model = ParakeetModel::load(model_dir, &Quantization::Int8)
            .map_err(|e| anyhow!("failed to load parakeet model: {e}"))?;
        Ok(Self { model })
    }

    /// samples: 16 kHz mono f32 — same contract as the whisper path.
    pub fn transcribe(&mut self, samples: &[f32]) -> Result<String> {
        let result = self
            .model
            .transcribe_with(samples, &ParakeetParams::default())
            .map_err(|e| anyhow!("parakeet inference failed: {e}"))?;
        Ok(result.text.trim().to_string())
    }

    /// Long-form variant (engine, #113): the same inference asking
    /// transcribe-rs for word-level timestamps (TDT frame times, seconds,
    /// already shifted back past its 250 ms leading pad) → words in ms
    /// relative to the start of `samples`.
    pub fn transcribe_timed(&mut self, samples: &[f32]) -> Result<super::TimedTranscript> {
        let params = ParakeetParams {
            timestamp_granularity: Some(TimestampGranularity::Word),
            ..Default::default()
        };
        let result = self
            .model
            .transcribe_with(samples, &params)
            .map_err(|e| anyhow!("parakeet inference failed: {e}"))?;
        let words = result
            .segments
            .unwrap_or_default()
            .into_iter()
            .filter_map(|s| {
                let w = s.text.trim().to_string();
                (!w.is_empty()).then(|| crate::archive::Word {
                    w,
                    start_ms: secs_to_ms(s.start),
                    end_ms: secs_to_ms(s.end.max(s.start)),
                })
            })
            .collect();
        Ok(super::TimedTranscript {
            text: result.text.trim().to_string(),
            words,
            words_estimated: false,
            language: None,
        })
    }
}

fn secs_to_ms(s: f32) -> u64 {
    (s.max(0.0) * 1000.0).round() as u64
}
