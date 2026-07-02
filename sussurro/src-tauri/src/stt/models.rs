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

#[cfg(test)]
mod tests {
    use super::*;

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
