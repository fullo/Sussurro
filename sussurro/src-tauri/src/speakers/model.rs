//! The speaker-embedding model (E8, chosen in #107): WeSpeaker ResNet34-LM
//! (VoxCeleb2, 256-d), run through the app's own `ort` — never sherpa-onnx,
//! which would bundle a second ONNX Runtime.
//!
//! The ONNX file is downloaded on first use into the models folder and
//! checked against the SHA-256 pinned here, fail-closed (as #90): a file
//! that doesn't match is deleted and never loaded. It is published by the
//! WeSpeaker project under CC-BY-4.0; the attribution is in the About
//! dialog's licence list (`scripts/gen-licenses.mjs`).

use super::fbank::{wespeaker_features, Fbank, NUM_MEL};
use anyhow::{anyhow, bail, Context, Result};
use std::path::{Path, PathBuf};

/// File name in the models folder.
pub const MODEL_FILE: &str = "wespeaker-voxceleb-resnet34-LM.onnx";
/// Upstream file at a pinned revision of the official WeSpeaker repo.
pub const MODEL_URL: &str = "https://huggingface.co/Wespeaker/wespeaker-voxceleb-resnet34-LM/resolve/f0c48c298fd835726c27956a5d617bad7115627e/voxceleb_resnet34_LM.onnx";
/// SHA-256 of `voxceleb_resnet34_LM.onnx` (26 530 309 bytes), as measured
/// in #107 and published by HuggingFace (`lfs.oid`).
pub const MODEL_SHA256: &str = "7bb2f06e9df17cdf1ef14ee8a15ab08ed28e8d0ef5054ee135741560df2ec068";
/// Embedding size.
pub const EMBEDDING_DIM: usize = 256;
/// Intra-op threads: ≥ 4 keeps a 3 s window under ~50 ms on an M1 Pro
/// (#107); one thread is ~170 ms, still far faster than real time.
const MAX_THREADS: usize = 4;

/// Turns 16 kHz mono speech into an L2-normalised speaker embedding.
/// The app uses [`WeSpeaker`]; tests pass fakes.
pub trait SpeakerEmbedder: Send {
    fn embed(&mut self, samples: &[f32]) -> Result<Vec<f32>>;
}

pub fn model_exists(models_dir: &Path) -> bool {
    models_dir.join(MODEL_FILE).is_file()
}

/// Whether the file at `path` has the pinned hash.
fn verify(path: &Path, expected: &str) -> Result<()> {
    let actual = crate::stt::models::sha256_hex(path)?;
    if !actual.eq_ignore_ascii_case(expected) {
        bail!(
            "{} SHA-256 mismatch (expected {expected}, got {actual})",
            path.display()
        );
    }
    Ok(())
}

/// The model file, downloaded and verified if missing. A file already in
/// place is verified too (26 MB hash, a few tens of ms); one that doesn't
/// match is deleted and downloaded again. Blocking.
pub fn ensure_model(models_dir: &Path) -> Result<PathBuf> {
    ensure_pinned(models_dir, MODEL_FILE, MODEL_SHA256, |dest| {
        let client = crate::stt::models::download_client_builder().build()?;
        let mut resp = client
            .get(MODEL_URL)
            .send()
            .context("speaker model download request failed")?
            .error_for_status()?;
        let mut out = std::fs::File::create(dest)?;
        std::io::copy(&mut resp, &mut out).context("speaker model download interrupted")?;
        Ok(())
    })
}

/// [`ensure_model`] with the download injected (tests never touch the
/// network): `fetch` writes the file's bytes to the path it is given.
pub(crate) fn ensure_pinned(
    models_dir: &Path,
    file: &str,
    sha256: &str,
    fetch: impl FnOnce(&Path) -> Result<()>,
) -> Result<PathBuf> {
    let path = crate::stt::models::resolve_model_path(models_dir, file)?;
    if path.is_file() {
        match verify(&path, sha256) {
            Ok(()) => return Ok(path),
            Err(e) => {
                eprintln!("speaker model: {e:#} — downloading it again");
                std::fs::remove_file(&path)
                    .with_context(|| format!("removing {}", path.display()))?;
            }
        }
    }
    std::fs::create_dir_all(models_dir)
        .with_context(|| format!("creating models dir {}", models_dir.display()))?;
    let tmp = models_dir.join(format!("{file}.part"));
    let fetched = fetch(&tmp).and_then(|()| verify(&tmp, sha256));
    if let Err(e) = fetched {
        let _ = std::fs::remove_file(&tmp);
        return Err(e.context(format!("{file} not downloaded — nothing was kept")));
    }
    std::fs::rename(&tmp, &path).with_context(|| format!("moving {file} into place"))?;
    eprintln!("speaker model {file} downloaded and verified");
    Ok(path)
}

/// WeSpeaker ResNet34-LM through `ort`.
pub struct WeSpeaker {
    session: ort::session::Session,
    input: String,
    fbank: Fbank,
}

impl WeSpeaker {
    /// Load the model file (see [`ensure_model`]).
    pub fn load(path: &Path) -> Result<Self> {
        let threads = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1)
            .clamp(1, MAX_THREADS);
        let session = ort::session::Session::builder()
            .map_err(|e| anyhow!("speaker model: {e}"))?
            .with_intra_threads(threads)
            .map_err(|e| anyhow!("speaker model: {e}"))?
            .commit_from_file(path)
            .map_err(|e| anyhow!("loading the speaker model {}: {e}", path.display()))?;
        let input = session
            .inputs()
            .first()
            .map(|i| i.name().to_string())
            .context("the speaker model has no input")?;
        Ok(Self {
            session,
            input,
            fbank: Fbank::new(),
        })
    }
}

impl SpeakerEmbedder for WeSpeaker {
    fn embed(&mut self, samples: &[f32]) -> Result<Vec<f32>> {
        let (frames, feats) = wespeaker_features(&self.fbank, samples);
        if frames == 0 {
            bail!("too little audio for a speaker embedding");
        }
        let tensor = ort::value::Tensor::from_array(([1usize, frames, NUM_MEL], feats))
            .map_err(|e| anyhow!("speaker model input: {e}"))?;
        let out = self
            .session
            .run(ort::inputs![self.input.as_str() => tensor])
            .map_err(|e| anyhow!("speaker model: {e}"))?;
        let (_, data) = out[0]
            .try_extract_tensor::<f32>()
            .map_err(|e| anyhow!("speaker model output: {e}"))?;
        if data.len() != EMBEDDING_DIM {
            bail!(
                "speaker model returned {} values, expected {EMBEDDING_DIM}",
                data.len()
            );
        }
        Ok(super::cluster::l2_normalize(data.to_vec()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ABC_SHA: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";

    #[test]
    fn pinned_download_keeps_only_matching_bytes() {
        let dir = tempfile::tempdir().unwrap();
        // Wrong bytes: refused, nothing left behind.
        let err = ensure_pinned(dir.path(), "m.onnx", ABC_SHA, |p| {
            std::fs::write(p, b"not the model")?;
            Ok(())
        })
        .unwrap_err();
        assert!(format!("{err:#}").contains("mismatch"), "{err:#}");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
        // A failed fetch keeps nothing either.
        assert!(ensure_pinned(dir.path(), "m.onnx", ABC_SHA, |_| bail!("offline")).is_err());
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
        // Matching bytes: kept under the final name.
        let p = ensure_pinned(dir.path(), "m.onnx", ABC_SHA, |p| {
            std::fs::write(p, b"abc")?;
            Ok(())
        })
        .unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), b"abc");
        // Present and valid: no fetch at all.
        ensure_pinned(dir.path(), "m.onnx", ABC_SHA, |_| panic!("must not fetch")).unwrap();
    }

    #[test]
    fn a_tampered_model_on_disk_is_replaced() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("m.onnx"), b"tampered").unwrap();
        let p = ensure_pinned(dir.path(), "m.onnx", ABC_SHA, |p| {
            std::fs::write(p, b"abc")?;
            Ok(())
        })
        .unwrap();
        assert_eq!(std::fs::read(p).unwrap(), b"abc");
    }

    #[test]
    fn model_is_pinned_to_the_phase0_file() {
        assert_eq!(
            MODEL_SHA256,
            "7bb2f06e9df17cdf1ef14ee8a15ab08ed28e8d0ef5054ee135741560df2ec068"
        );
        assert!(MODEL_URL.starts_with("https://huggingface.co/Wespeaker/"));
        assert!(MODEL_URL.ends_with("/voxceleb_resnet34_LM.onnx"));
        assert!(crate::stt::models::validate_model_name(MODEL_FILE).is_ok());
    }
}
