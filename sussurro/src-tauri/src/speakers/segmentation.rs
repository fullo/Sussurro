//! The overlap model (0.11, #244; spike #237): pyannote segmentation-3.0,
//! in the ONNX export of `onnx-community/pyannote-segmentation-3.0` (fp32,
//! 6 MB), run through the app's own `ort` next to WeSpeaker.
//!
//! Like the speaker model it is downloaded on first use into the models
//! folder and checked against the SHA-256 pinned here, at a pinned
//! revision of that third-party repo, fail-closed (a file that doesn't
//! match is deleted and never loaded). The export repo is not gated (the
//! upstream `pyannote/segmentation-3.0` asks for a contact form; its
//! licence is MIT either way). MIT, "Copyright (c) 2023 CNRS"; the
//! attribution is in the About dialog's licence list
//! (`scripts/gen-licenses.mjs`).
//!
//! I/O: `input_values` f32 `[batch, 1, samples]` (raw 16 kHz waveform,
//! normalised inside the model) → `logits` f32 `[batch, frames, 7]`,
//! log-softmax over the powerset classes ([`super::overlap`]).

use super::overlap::{decode_frames, OverlapModel, CLASSES, WINDOW_SAMPLES};
use anyhow::{anyhow, bail, Context, Result};
use std::path::{Path, PathBuf};

/// File name in the models folder.
pub const SEGMENTATION_FILE: &str = "pyannote-segmentation-3.0.onnx";
/// `onnx/model.onnx` (fp32) at a pinned revision of the export repo.
pub const SEGMENTATION_URL: &str = "https://huggingface.co/onnx-community/pyannote-segmentation-3.0/resolve/733a93b6473d019a773298e08cefa686894b1854/onnx/model.onnx";
/// SHA-256 of that file (5 986 908 bytes), as measured in #237.
pub const SEGMENTATION_SHA256: &str =
    "057ee564753071c0b09b5b611648b50ac188d50846bff5f01e9f7bbf1591ea25";
/// Intra-op threads: 2 run a batch of eight 10 s windows in ~35 ms per
/// window on an M1 Pro (#237), ~30 s per hour of lines.
const THREADS: usize = 2;

pub fn model_exists(models_dir: &Path) -> bool {
    models_dir.join(SEGMENTATION_FILE).is_file()
}

/// The model file, downloaded and verified if missing (a file in place
/// that doesn't match is replaced). Blocking.
pub fn ensure_model(models_dir: &Path) -> Result<PathBuf> {
    super::model::ensure_pinned(models_dir, SEGMENTATION_FILE, SEGMENTATION_SHA256, |dest| {
        let client = crate::stt::models::download_client_builder().build()?;
        let mut resp = client
            .get(SEGMENTATION_URL)
            .send()
            .context("overlap model download request failed")?
            .error_for_status()?;
        let mut out = std::fs::File::create(dest)?;
        std::io::copy(&mut resp, &mut out).context("overlap model download interrupted")?;
        Ok(())
    })
}

/// pyannote segmentation-3.0 through `ort`.
pub struct Segmentation {
    session: ort::session::Session,
    input: String,
}

impl Segmentation {
    /// Load the model file (see [`ensure_model`]).
    pub fn load(path: &Path) -> Result<Self> {
        let session = ort::session::Session::builder()
            .map_err(|e| anyhow!("overlap model: {e}"))?
            .with_intra_threads(THREADS)
            .map_err(|e| anyhow!("overlap model: {e}"))?
            .commit_from_file(path)
            .map_err(|e| anyhow!("loading the overlap model {}: {e}", path.display()))?;
        let input = session
            .inputs()
            .first()
            .map(|i| i.name().to_string())
            .context("the overlap model has no input")?;
        Ok(Self { session, input })
    }
}

impl OverlapModel for Segmentation {
    fn frame_overlap(&mut self, windows: &[Vec<f32>]) -> Result<Vec<Vec<f32>>> {
        if windows.is_empty() {
            return Ok(Vec::new());
        }
        if windows.iter().any(|w| w.len() != WINDOW_SAMPLES) {
            bail!("overlap model windows must be {WINDOW_SAMPLES} samples");
        }
        let n = windows.len();
        let data: Vec<f32> = windows.concat();
        let tensor = ort::value::Tensor::from_array(([n, 1usize, WINDOW_SAMPLES], data))
            .map_err(|e| anyhow!("overlap model input: {e}"))?;
        let out = self
            .session
            .run(ort::inputs![self.input.as_str() => tensor])
            .map_err(|e| anyhow!("overlap model: {e}"))?;
        let (shape, logits) = out[0]
            .try_extract_tensor::<f32>()
            .map_err(|e| anyhow!("overlap model output: {e}"))?;
        let dims: Vec<i64> = shape.iter().copied().collect();
        if dims.len() != 3 || dims[0] as usize != n || dims[2] as usize != CLASSES {
            bail!("overlap model returned shape {dims:?}, expected [{n}, frames, {CLASSES}]");
        }
        let per_window = dims[1] as usize * CLASSES;
        Ok(logits.chunks_exact(per_window).map(decode_frames).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_is_pinned_to_the_spike_file() {
        assert_eq!(
            SEGMENTATION_SHA256,
            "057ee564753071c0b09b5b611648b50ac188d50846bff5f01e9f7bbf1591ea25"
        );
        assert!(SEGMENTATION_URL.starts_with(
            "https://huggingface.co/onnx-community/pyannote-segmentation-3.0/resolve/733a93b6473d019a773298e08cefa686894b1854/"
        ));
        assert!(SEGMENTATION_URL.ends_with("/onnx/model.onnx"));
        assert!(crate::stt::models::validate_model_name(SEGMENTATION_FILE).is_ok());
        assert_ne!(SEGMENTATION_FILE, super::super::model::MODEL_FILE);
    }

    /// Runs the real model on a WAV with overlapping speech (16 kHz mono,
    /// 16-bit) and checks that some overlap is found:
    ///
    /// ```text
    /// SUSSURRO_OVERLAP_MODEL=<pyannote-segmentation-3.0.onnx> \
    /// SUSSURRO_OVERLAP_WAV=<two voices, partly at once> \
    ///   cargo test speakers::segmentation::tests::real_model -- --ignored --nocapture
    /// ```
    ///
    /// The WAV is cut into ≤ 10 s "lines" as the tracker would see them;
    /// the spans of each are printed for a listen-check.
    #[test]
    #[ignore = "needs the model file and a WAV with overlapping speech"]
    fn real_model_finds_overlap_in_a_wav() {
        let model = PathBuf::from(std::env::var("SUSSURRO_OVERLAP_MODEL").unwrap());
        assert_eq!(
            crate::stt::models::sha256_hex(&model).unwrap(),
            SEGMENTATION_SHA256
        );
        let bytes = std::fs::read(std::env::var("SUSSURRO_OVERLAP_WAV").unwrap()).unwrap();
        let data = bytes
            .windows(4)
            .position(|w| w == b"data")
            .expect("no data chunk");
        let pcm: Vec<f32> = bytes[data + 8..]
            .chunks_exact(2)
            .map(|c| i16::from_le_bytes([c[0], c[1]]) as f32 / 32768.0)
            .collect();
        let mut seg = Segmentation::load(&model).unwrap();
        // Frame level first: the model's own view of the whole file.
        let (pieces, windows) = super::super::overlap::line_windows(&pcm);
        let probs = seg.frame_overlap(&windows).unwrap();
        assert!(probs.iter().all(|p| p.len() == 589), "10 s = 589 frames");
        let frames = super::super::overlap::stitch(&pieces, &probs, 0.5);
        let total_ms: usize = frames.iter().map(|(a, b)| (b - a) / 16).sum();
        eprintln!("overlap frames: {total_ms} ms in {} ms", pcm.len() / 16);
        // Then as lines.
        let mut found = 0;
        for (k, line) in pcm.chunks(160_000).enumerate() {
            let spans = super::super::overlap::detect(&mut seg, line).unwrap();
            eprintln!("line {k} ({} ms): {spans:?}", line.len() / 16);
            found += spans.len();
        }
        assert!(total_ms > 0, "no overlap frame at all");
        assert!(found > 0, "no overlapped line");
    }
}
