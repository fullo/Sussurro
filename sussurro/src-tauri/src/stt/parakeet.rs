use anyhow::{anyhow, Result};
use std::path::Path;
use transcribe_rs::onnx::parakeet::{ParakeetModel, ParakeetParams};
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
}
