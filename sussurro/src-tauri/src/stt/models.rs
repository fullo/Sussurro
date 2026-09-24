use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

/// HuggingFace repo of the whisper.cpp GGML models.
pub const WHISPER_REPO: &str = "ggerganov/whisper.cpp";

/// Silero VAD for whisper.cpp's built-in `whisper_vad` (long-form engine,
/// #113). Published in its own repo, NOT in ggerganov/whisper.cpp (whose
/// listing has no `ggml-silero-*` file); whisper.cpp's own
/// `download-vad-model` script fetches it from here too. Its SHA-256 comes
/// from the repo's tree API (`lfs.oid`) like the whisper models, fail-closed.
pub const VAD_REPO: &str = "ggml-org/whisper-vad";
pub const VAD_MODEL_FILE: &str = "ggml-silero-v5.1.2.bin";

pub fn model_url(file: &str) -> String {
    hf_resolve_url(WHISPER_REPO, file)
}

fn hf_resolve_url(repo: &str, file: &str) -> String {
    format!("https://huggingface.co/{repo}/resolve/main/{file}")
}

/// Validate a configured whisper model name. `settings.json` is user-editable
/// and the value flows into `models_dir.join(name)` for download and load, so
/// it must be a simple file name: `[A-Za-z0-9._-]+` — no separators, no `..`,
/// no drive letters or absolute paths (which would write/read outside the
/// models dir).
pub fn validate_model_name(name: &str) -> Result<()> {
    let valid = !name.is_empty()
        && name != "."
        && name != ".."
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'));
    if valid {
        Ok(())
    } else {
        anyhow::bail!(
            "invalid model name '{name}' — use a simple file name like ggml-base.bin"
        )
    }
}

/// Join `name` onto `models_dir`, refusing anything that resolves outside it.
/// `validate_model_name` already guarantees the name has no separators or
/// `..`; this is the belt-and-suspenders check — once the directory exists,
/// verify the resolved path is still inside the (canonicalized) models dir.
pub fn resolve_model_path(models_dir: &Path, name: &str) -> Result<PathBuf> {
    validate_model_name(name)?;
    let path = models_dir.join(name);
    if let Ok(dir) = models_dir.canonicalize() {
        if path.exists() {
            let canon = path
                .canonicalize()
                .with_context(|| format!("resolving model path {}", path.display()))?;
            if !canon.starts_with(&dir) {
                anyhow::bail!("model name '{name}' resolves outside the models directory");
            }
            return Ok(canon);
        }
        // Fresh download: the validated name has no separators or `..`, so
        // dir.join(name) is inside dir by construction.
        return Ok(dir.join(name));
    }
    // Models dir doesn't exist yet — nothing to resolve against; the name
    // validation above is what keeps the join inside it.
    Ok(path)
}

/// HuggingFace repo tree listing. For LFS files (every ggml .bin) each entry
/// carries `lfs.oid` — the file's SHA-256 hex.
fn hf_tree_api(repo: &str) -> String {
    format!("https://huggingface.co/api/models/{repo}/tree/main")
}

/// Parse a HuggingFace tree listing and return the published SHA-256 of
/// `file` (the entry's `lfs.oid`). Pure — testable without network.
pub fn sha256_from_hf_tree(json: &str, file: &str) -> Result<String> {
    let entries: Vec<serde_json::Value> = serde_json::from_str(json)
        .context("HuggingFace tree API returned invalid JSON")?;
    let entry = entries
        .iter()
        .find(|e| e.get("path").and_then(|p| p.as_str()) == Some(file))
        .ok_or_else(|| anyhow::anyhow!("'{file}' not found in the HuggingFace repo listing"))?;
    let oid = entry
        .get("lfs")
        .and_then(|lfs| lfs.get("oid"))
        .and_then(|o| o.as_str())
        .map(str::to_string)
        .ok_or_else(|| {
            anyhow::anyhow!("no SHA-256 published for '{file}' — cannot verify integrity")
        })?;
    Ok(oid)
}

/// Fetch the expected SHA-256 of `file` from HuggingFace. Fails closed: an
/// unverifiable download is a failed download.
fn fetch_expected_sha256(
    client: &reqwest::blocking::Client,
    repo: &str,
    file: &str,
) -> Result<String> {
    let body = client
        .get(hf_tree_api(repo))
        .send()
        .context("model integrity check: HuggingFace tree API request failed")?
        .error_for_status()
        .context("model integrity check: HuggingFace tree API returned an error")?
        .text()
        .context("model integrity check: reading HuggingFace tree API response")?;
    sha256_from_hf_tree(&body, file)
}

/// Download client with sane timeouts: connect fast, and abort stalled
/// downloads. The blocking client's `timeout` bounds every connect/read/write
/// operation (an idle gap between bytes, not the total transfer), so multi-GB
/// models on slow links still complete while a malicious or broken server can't
/// hold the download open forever.
fn download_client() -> Result<reqwest::blocking::Client> {
    Ok(reqwest::blocking::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(10))
        .timeout(std::time::Duration::from_secs(60))
        .build()?)
}

pub fn model_exists(models_dir: &Path, file: &str) -> bool {
    models_dir.join(file).exists()
}

/// Filenames of GGML whisper models already present in the folder
/// (`ggml-*.bin`). Lets Sussurro reuse models shared with other whisper.cpp
/// tools — point the models folder at a shared directory instead of
/// re-downloading. Returns an empty list if the folder can't be read.
pub fn list_ggml_models(models_dir: &Path) -> Vec<String> {
    let mut out: Vec<String> = std::fs::read_dir(models_dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| n.starts_with("ggml-") && n.ends_with(".bin"))
        // The Silero VAD model shares the prefix but is not a whisper model.
        .filter(|n| !n.starts_with("ggml-silero"))
        .collect();
    out.sort();
    out
}

/// Download the GGML model if missing. Blocking — callers must run this off
/// the async runtime (spawn_blocking) and off the UI thread.
pub fn ensure_model(models_dir: &Path, file: &str) -> Result<PathBuf> {
    ensure_hf_file(models_dir, WHISPER_REPO, file)
}

pub fn vad_model_exists(models_dir: &Path) -> bool {
    models_dir.join(VAD_MODEL_FILE).is_file()
}

/// Download the Silero VAD model (~0.9 MB) if missing, verified like the
/// whisper models. Blocking.
pub fn ensure_vad_model(models_dir: &Path) -> Result<PathBuf> {
    ensure_hf_file(models_dir, VAD_REPO, VAD_MODEL_FILE)
}

/// Download `file` from the HuggingFace `repo` into `models_dir` if missing,
/// verified against the SHA-256 the repo publishes (fail closed). Blocking.
fn ensure_hf_file(models_dir: &Path, repo: &str, file: &str) -> Result<PathBuf> {
    // settings.json is user-editable — validate before any filesystem work.
    let path = resolve_model_path(models_dir, file)?;
    if path.exists() {
        eprintln!("model {file} found on disk — not re-downloaded");
        return Ok(path);
    }
    std::fs::create_dir_all(models_dir)
        .with_context(|| format!("creating models dir {}", models_dir.display()))?;
    let tmp = models_dir.join(format!("{file}.part"));

    let client = download_client()?;
    let mut resp = client
        .get(hf_resolve_url(repo, file))
        .send()
        .context("model download request failed")?
        .error_for_status()?;
    let mut out = std::fs::File::create(&tmp)?;
    std::io::copy(&mut resp, &mut out).context("model download interrupted")?;
    drop(out);

    // Integrity check (fail closed): the upstream repo publishes each file's
    // SHA-256 — verify before keeping the downloaded bytes.
    let expected = fetch_expected_sha256(&client, repo, file)
        .with_context(|| format!("verifying {file} against huggingface.co"))?;
    let actual = sha256_hex(&tmp)?;
    if !actual.eq_ignore_ascii_case(&expected) {
        let _ = std::fs::remove_file(&tmp);
        anyhow::bail!(
            "{file} SHA-256 mismatch (expected {expected}, got {actual}) — download deleted, retry"
        );
    }
    std::fs::rename(&tmp, &path).with_context(|| format!("moving {file} into place"))?;
    eprintln!("model {file} downloaded and verified ({actual})");
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

    let client = download_client()?;
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
    fn list_ggml_models_finds_only_ggml_bins_sorted() {
        let dir = tempfile::tempdir().unwrap();
        for f in [
            "ggml-small.bin",
            "ggml-base.en.bin",
            "notes.txt",
            "model.pt",
            "ggml-x.gguf",
            "ggml-silero-v5.1.2.bin", // the VAD model is not a whisper model
        ] {
            std::fs::write(dir.path().join(f), b"x").unwrap();
        }
        assert_eq!(
            list_ggml_models(dir.path()),
            vec!["ggml-base.en.bin".to_string(), "ggml-small.bin".to_string()]
        );
    }

    #[test]
    fn list_ggml_models_empty_on_missing_dir() {
        assert!(list_ggml_models(Path::new("/no/such/dir/here")).is_empty());
    }

    #[test]
    fn model_url_points_at_ggerganov_repo() {
        assert_eq!(
            model_url("ggml-base.en.bin"),
            "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-base.en.bin"
        );
    }

    #[test]
    fn vad_model_lives_in_the_whisper_vad_repo() {
        assert_eq!(
            hf_resolve_url(VAD_REPO, VAD_MODEL_FILE),
            "https://huggingface.co/ggml-org/whisper-vad/resolve/main/ggml-silero-v5.1.2.bin"
        );
        assert_eq!(
            hf_tree_api(VAD_REPO),
            "https://huggingface.co/api/models/ggml-org/whisper-vad/tree/main"
        );
        assert!(validate_model_name(VAD_MODEL_FILE).is_ok());
    }

    #[test]
    fn ensure_vad_model_returns_existing_file_without_network() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!vad_model_exists(dir.path()));
        std::fs::write(dir.path().join(VAD_MODEL_FILE), b"fake vad").unwrap();
        assert!(vad_model_exists(dir.path()));
        let got = ensure_vad_model(dir.path()).unwrap();
        assert_eq!(got.file_name().and_then(|n| n.to_str()), Some(VAD_MODEL_FILE));
    }

    #[test]
    fn ensure_model_returns_existing_file_without_network() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("ggml-tiny.bin"), b"fake model").unwrap();
        // The path is canonicalized (symlink-free) — compare against that,
        // not the raw tempdir, which may sit behind a symlink (macOS).
        let got = ensure_model(dir.path(), "ggml-tiny.bin").unwrap();
        assert_eq!(got.file_name().and_then(|n| n.to_str()), Some("ggml-tiny.bin"));
        assert!(got.exists());
        if let Ok(canon_dir) = dir.path().canonicalize() {
            assert!(got.starts_with(&canon_dir));
        }
    }

    #[test]
    fn missing_model_reports_not_downloaded_yet() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!model_exists(dir.path(), "ggml-tiny.bin"));
    }

    #[test]
    fn validate_model_name_accepts_simple_file_names() {
        for name in [
            "ggml-base.bin",
            "ggml-large-v3-turbo-q5_0.bin",
            "model.tar.gz",
            "a.b-c_d9",
            ".hidden-model.bin", // a leading dot is fine — it stays in the dir
        ] {
            assert!(validate_model_name(name).is_ok(), "{name} should be valid");
        }
    }

    #[test]
    fn validate_model_name_rejects_traversal_and_paths() {
        for name in [
            "", // empty
            ".",
            "..",
            "../evil.bin",
            "..\\..\\ProgramData\\x\\mal.bin", // Windows traversal
            "a/b/c.bin", // forward separator
            "a\\b\\c.bin", // backslash separator
            "C:\\Windows\\mal.bin", // drive letter
            "/etc/passwd", // absolute path
            "//server/share/x", // UNC-style
            "..ggml/evil.bin", // separator after a dot-run
        ] {
            assert!(validate_model_name(name).is_err(), "{name:?} should be invalid");
        }
    }

    #[test]
    fn resolve_model_path_keeps_existing_files_inside_the_dir() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("ggml-tiny.bin"), b"x").unwrap();
        let got = resolve_model_path(dir.path(), "ggml-tiny.bin").unwrap();
        assert!(got.starts_with(dir.path().canonicalize().unwrap()));
    }

    #[test]
    fn resolve_model_path_accepts_missing_file_names() {
        // Fresh download: the file doesn't exist yet — still resolved inside.
        let dir = tempfile::tempdir().unwrap();
        let got = resolve_model_path(dir.path(), "ggml-new.bin").unwrap();
        assert_eq!(got, dir.path().canonicalize().unwrap().join("ggml-new.bin"));
    }

    #[test]
    fn resolve_model_path_rejects_traversal() {
        let dir = tempfile::tempdir().unwrap();
        for name in ["..", "../x", "..\\..\\x", "/abs", "C:/x"] {
            assert!(resolve_model_path(dir.path(), name).is_err(), "{name:?}");
        }
    }

    /// Realistic (abbreviated) HuggingFace tree listing: LFS files carry
    /// `lfs.oid` = SHA-256 hex; small files don't.
    const HF_TREE_JSON: &str = r#"[
        {"type": "file", "size": 139, "oid": "abc123", "path": ".gitattributes"},
        {"type": "file", "size": 747758625, "oid": "def456",
         "path": "ggml-base.bin",
         "lfs": {"oid": "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad", "size": 747758625, "pointerSize": 139}},
        {"type": "directory", "size": 0, "oid": "ghi789", "path": "models"}
    ]"#;

    #[test]
    fn hf_tree_lookup_finds_published_sha256() {
        assert_eq!(
            sha256_from_hf_tree(HF_TREE_JSON, "ggml-base.bin").unwrap(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn hf_tree_lookup_fails_closed_when_unverifiable() {
        // File not in the listing.
        assert!(sha256_from_hf_tree(HF_TREE_JSON, "ggml-nope.bin").is_err());
        // In the listing but no published SHA-256 (not LFS).
        assert!(sha256_from_hf_tree(HF_TREE_JSON, ".gitattributes").is_err());
        // Garbage API response.
        assert!(sha256_from_hf_tree("{not json", "ggml-base.bin").is_err());
    }
}
