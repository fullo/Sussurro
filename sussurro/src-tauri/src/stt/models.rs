use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

pub fn model_url(file: &str) -> String {
    format!("https://huggingface.co/ggerganov/whisper.cpp/resolve/main/{file}")
}

pub fn model_exists(models_dir: &Path, file: &str) -> bool {
    models_dir.join(file).exists()
}

/// Download the GGML model if missing. Blocking — callers must run this off
/// the async runtime (spawn_blocking) and off the UI thread.
pub fn ensure_model(models_dir: &Path, file: &str) -> Result<PathBuf> {
    let path = models_dir.join(file);
    if path.exists() {
        return Ok(path);
    }
    std::fs::create_dir_all(models_dir)?;
    let tmp = models_dir.join(format!("{file}.part"));

    let client = reqwest::blocking::Client::builder()
        .timeout(None) // large file, no overall timeout
        .build()?;
    let mut resp = client
        .get(model_url(file))
        .send()
        .context("model download request failed")?
        .error_for_status()?;
    let mut out = std::fs::File::create(&tmp)?;
    std::io::copy(&mut resp, &mut out).context("model download interrupted")?;
    drop(out);
    std::fs::rename(&tmp, &path)?;
    Ok(path)
}

/// Parakeet ships as a tar.gz containing the model directory.
pub fn parakeet_exists(models_dir: &Path) -> bool {
    models_dir
        .join(crate::stt::parakeet::PARAKEET_DIR)
        .join("nemo128.onnx")
        .exists()
}

/// Download + verify + extract the Parakeet archive. Blocking — run off the
/// async runtime.
pub fn ensure_parakeet(models_dir: &Path) -> Result<PathBuf> {
    use crate::stt::parakeet::{PARAKEET_DIR, PARAKEET_SHA256, PARAKEET_URL};

    let dir = models_dir.join(PARAKEET_DIR);
    if parakeet_exists(models_dir) {
        return Ok(dir);
    }
    std::fs::create_dir_all(models_dir)?;
    let tmp = models_dir.join("parakeet-v3-int8.tar.gz.part");

    let client = reqwest::blocking::Client::builder().timeout(None).build()?;
    let mut resp = client
        .get(PARAKEET_URL)
        .send()
        .context("parakeet download request failed")?
        .error_for_status()?;
    let mut out = std::fs::File::create(&tmp)?;
    std::io::copy(&mut resp, &mut out).context("parakeet download interrupted")?;
    drop(out);

    let digest = sha256_hex(&tmp)?;
    if digest != PARAKEET_SHA256 {
        let _ = std::fs::remove_file(&tmp);
        anyhow::bail!("parakeet archive checksum mismatch (got {digest}) — retry the download");
    }

    let tar_gz = std::fs::File::open(&tmp)?;
    let decoder = flate2::read::GzDecoder::new(tar_gz);
    tar::Archive::new(decoder)
        .unpack(models_dir)
        .context("failed to extract parakeet archive")?;
    let _ = std::fs::remove_file(&tmp);

    if !parakeet_exists(models_dir) {
        anyhow::bail!("parakeet archive extracted but {PARAKEET_DIR}/nemo128.onnx is missing");
    }
    Ok(dir)
}

pub fn sha256_hex(path: &Path) -> Result<String> {
    use sha2::{Digest, Sha256};
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    std::io::copy(&mut file, &mut hasher)?;
    Ok(format!("{:x}", hasher.finalize()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_matches_known_vector() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("x.txt");
        std::fs::write(&f, b"abc").unwrap();
        assert_eq!(
            sha256_hex(&f).unwrap(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn parakeet_missing_on_empty_dir() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!parakeet_exists(dir.path()));
    }

    #[test]
    fn model_url_points_at_ggerganov_repo() {
        assert_eq!(
            model_url("ggml-base.en.bin"),
            "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-base.en.bin"
        );
    }

    #[test]
    fn ensure_model_returns_existing_file_without_network() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("ggml-tiny.bin");
        std::fs::write(&file, b"fake model").unwrap();
        let got = ensure_model(dir.path(), "ggml-tiny.bin").unwrap();
        assert_eq!(got, file);
    }

    #[test]
    fn missing_model_reports_not_downloaded_yet() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!model_exists(dir.path(), "ggml-tiny.bin"));
    }
}
