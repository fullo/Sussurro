//! The read-aloud models on disk (#255, P24): where they live, whether
//! they are there, downloading them (pinned, verified, fail-closed,
//! cancellable, with progress) and deleting them.
//!
//! Layout, under the models folder:
//!
//! ```text
//! pocket-tts/<bundle>/bundle.json, tokenizer.model, *.onnx
//! pocket-tts/<bundle>/voices/<voice>.safetensors
//! ```
//!
//! **Nothing here runs on its own**: every download starts from the
//! user's click in Models → Voices while the module is on (checked by the
//! command, P24) — never at install, onboarding, first use of another
//! feature or in the background. A file counts as present when it has its
//! pinned size; its SHA-256 is checked while it downloads, and a mismatch
//! deletes it (as #90). Partial downloads are `*.part` files, removed when
//! a download fails or is cancelled.

use super::catalog::{Language, PinnedFile, Voice, ROOT_DIR};
use anyhow::{bail, Context, Result};
use serde::Serialize;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

/// Folder of a language's model files.
pub fn language_dir(models_dir: &Path, lang: &Language) -> PathBuf {
    models_dir.join(ROOT_DIR).join(lang.bundle)
}

/// Folder of a language's voices.
pub fn voices_dir(models_dir: &Path, lang: &Language) -> PathBuf {
    language_dir(models_dir, lang).join("voices")
}

pub fn voice_path(models_dir: &Path, lang: &Language, voice: &Voice) -> PathBuf {
    voices_dir(models_dir, lang).join(voice.file.name)
}

/// Present with its pinned size (regular file, not a link).
fn present(path: &Path, f: &PinnedFile) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_file() && m.len() == f.bytes)
}

/// Every model file of `lang` is on disk.
pub fn model_present(models_dir: &Path, lang: &Language) -> bool {
    let dir = language_dir(models_dir, lang);
    lang.files.iter().all(|f| present(&dir.join(f.name), f))
}

pub fn voice_present(models_dir: &Path, lang: &Language, voice: &Voice) -> bool {
    present(&voice_path(models_dir, lang, voice), &voice.file)
}

/// Bytes of this module's files on disk (all languages, `.part` included).
pub fn bytes_on_disk(models_dir: &Path) -> u64 {
    fn walk(dir: &Path) -> u64 {
        let Ok(rd) = std::fs::read_dir(dir) else {
            return 0;
        };
        rd.flatten()
            .map(|e| match e.file_type() {
                Ok(t) if t.is_dir() => walk(&e.path()),
                Ok(t) if t.is_file() => e.metadata().map(|m| m.len()).unwrap_or(0),
                _ => 0,
            })
            .sum()
    }
    walk(&models_dir.join(ROOT_DIR))
}

/// What a download is doing, for the UI.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Progress {
    /// Language code.
    pub language: String,
    /// Voice being fetched, if the job is a voice only.
    pub voice: Option<String>,
    /// File being fetched now.
    pub file: String,
    pub done_bytes: u64,
    pub total_bytes: u64,
}

/// Where the bytes of a download come from: the network in the app, a
/// closure in tests. It opens `url` for reading.
pub trait Fetch {
    fn open(&self, url: &str) -> Result<Box<dyn Read>>;
}

/// The app's fetcher: the model-download client (connect 10 s, 60 s per
/// read), HTTPS to huggingface.co only.
pub struct HttpFetch {
    client: reqwest::blocking::Client,
}

impl HttpFetch {
    pub fn new() -> Result<Self> {
        Ok(HttpFetch {
            client: crate::stt::models::download_client_builder().build()?,
        })
    }
}

impl Fetch for HttpFetch {
    fn open(&self, url: &str) -> Result<Box<dyn Read>> {
        if !url.starts_with("https://huggingface.co/") {
            bail!("refusing to download from {url}");
        }
        let resp = self
            .client
            .get(url)
            .send()
            .context("download request failed")?
            .error_for_status()
            .context("download refused by the server")?;
        Ok(Box::new(resp))
    }
}

/// Download one pinned file to `dest` unless it is already there: into
/// `dest.part`, hashed on the way, at most its pinned size, then renamed
/// when size and SHA-256 match. Anything else leaves nothing behind.
/// `on_bytes` gets each block's length; `cancel` is checked between
/// blocks.
fn fetch_pinned(
    fetch: &dyn Fetch,
    url: &str,
    f: &PinnedFile,
    dest: &Path,
    cancel: &AtomicBool,
    on_bytes: &mut dyn FnMut(u64),
) -> Result<()> {
    use sha2::{Digest, Sha256};
    if present(dest, f) {
        on_bytes(f.bytes);
        return Ok(());
    }
    let dir = dest.parent().context("no parent folder")?;
    std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    let part = dest.with_file_name(format!("{}.part", f.name));
    let result = (|| -> Result<()> {
        let mut src = fetch.open(url)?;
        let mut out =
            std::fs::File::create(&part).with_context(|| format!("creating {}", part.display()))?;
        let mut hasher = Sha256::new();
        let mut buf = vec![0u8; 256 * 1024];
        let mut total = 0u64;
        loop {
            if cancel.load(Ordering::Relaxed) {
                bail!("download cancelled");
            }
            let n = src.read(&mut buf).context("download interrupted")?;
            if n == 0 {
                break;
            }
            total += n as u64;
            if total > f.bytes {
                bail!("{} is larger than its pinned size — refused", f.name);
            }
            hasher.update(&buf[..n]);
            out.write_all(&buf[..n])
                .with_context(|| format!("writing {}", part.display()))?;
            on_bytes(n as u64);
        }
        out.sync_all().ok();
        drop(out);
        if total != f.bytes {
            bail!("{} is incomplete ({total} of {} bytes)", f.name, f.bytes);
        }
        let actual = format!("{:x}", hasher.finalize());
        if !actual.eq_ignore_ascii_case(f.sha256) {
            bail!(
                "{} SHA-256 mismatch (expected {}, got {actual})",
                f.name,
                f.sha256
            );
        }
        std::fs::rename(&part, dest).with_context(|| format!("moving {} into place", f.name))?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&part);
    }
    result
}

/// Download `lang`'s model files and the voices in `voices` (ids of the
/// catalog), skipping what is already present. `progress` gets a snapshot
/// after every block. Blocking.
pub fn download(
    fetch: &dyn Fetch,
    models_dir: &Path,
    lang: &Language,
    voices: &[&Voice],
    with_model: bool,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(&Progress),
) -> Result<()> {
    let dir = language_dir(models_dir, lang);
    let mut jobs: Vec<(String, &PinnedFile, PathBuf)> = Vec::new();
    if with_model {
        for f in lang.files {
            jobs.push((lang.file_url(f), f, dir.join(f.name)));
        }
    }
    for v in voices {
        jobs.push((lang.voice_url(v), &v.file, voice_path(models_dir, lang, v)));
    }
    let mut state = Progress {
        language: lang.code.to_string(),
        voice: if with_model {
            None
        } else {
            voices.first().map(|v| v.id.to_string())
        },
        file: String::new(),
        done_bytes: 0,
        total_bytes: jobs.iter().map(|(_, f, _)| f.bytes).sum(),
    };
    for (url, f, dest) in jobs {
        state.file = f.name.to_string();
        progress(&state);
        fetch_pinned(fetch, &url, f, &dest, cancel, &mut |n| {
            state.done_bytes += n;
            progress(&state);
        })
        .with_context(|| format!("{} not downloaded — nothing was kept", f.name))?;
    }
    Ok(())
}

/// Delete one language's models and voices.
pub fn delete_language(models_dir: &Path, lang: &Language) -> Result<()> {
    remove_dir_confined(models_dir, &language_dir(models_dir, lang))
}

/// Delete one voice file.
pub fn delete_voice(models_dir: &Path, lang: &Language, voice: &Voice) -> Result<()> {
    let p = voice_path(models_dir, lang, voice);
    match std::fs::remove_file(&p) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e).with_context(|| format!("deleting {}", p.display())),
    }
}

/// Delete everything this module downloaded.
pub fn delete_all(models_dir: &Path) -> Result<()> {
    remove_dir_confined(models_dir, &models_dir.join(ROOT_DIR))
}

/// Remove `dir` (and what is in it) only if it is a real folder inside the
/// models folder — never through a link.
fn remove_dir_confined(models_dir: &Path, dir: &Path) -> Result<()> {
    let meta = match std::fs::symlink_metadata(dir) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e).with_context(|| format!("reading {}", dir.display())),
    };
    if !meta.file_type().is_dir() {
        bail!("{} is not a folder — not deleted", dir.display());
    }
    let base = models_dir.canonicalize()?;
    if !dir.canonicalize()?.starts_with(&base) {
        bail!(
            "{} is outside the models folder — not deleted",
            dir.display()
        );
    }
    std::fs::remove_dir_all(dir).with_context(|| format!("deleting {}", dir.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tts::catalog::{ENGLISH, ITALIAN};
    use std::collections::HashMap;
    use std::sync::Mutex;

    /// Serves fixed bytes per URL and records what was asked.
    struct FakeFetch {
        bodies: HashMap<String, Vec<u8>>,
        asked: Mutex<Vec<String>>,
    }

    impl Fetch for FakeFetch {
        fn open(&self, url: &str) -> Result<Box<dyn Read>> {
            self.asked.lock().unwrap().push(url.to_string());
            match self.bodies.get(url) {
                Some(b) => Ok(Box::new(std::io::Cursor::new(b.clone()))),
                None => bail!("404 {url}"),
            }
        }
    }

    const ABC_SHA: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";

    fn abc_file() -> PinnedFile {
        PinnedFile {
            remote: "x/abc.bin",
            name: "abc.bin",
            bytes: 3,
            sha256: ABC_SHA,
        }
    }

    fn fetch_one(body: &[u8], f: &PinnedFile, dir: &Path) -> Result<()> {
        let fake = FakeFetch {
            bodies: HashMap::from([("u".to_string(), body.to_vec())]),
            asked: Mutex::new(Vec::new()),
        };
        fetch_pinned(
            &fake,
            "u",
            f,
            &dir.join(f.name),
            &AtomicBool::new(false),
            &mut |_| {},
        )
    }

    #[test]
    fn only_the_pinned_bytes_are_kept() {
        let dir = tempfile::tempdir().unwrap();
        let f = abc_file();
        // Wrong content of the right size.
        let err = fetch_one(b"abd", &f, dir.path()).unwrap_err();
        assert!(format!("{err:#}").contains("mismatch"), "{err:#}");
        // Too long: refused before the hash, nothing written beyond.
        let err = fetch_one(b"abcd", &f, dir.path()).unwrap_err();
        assert!(format!("{err:#}").contains("larger"), "{err:#}");
        // Too short.
        let err = fetch_one(b"ab", &f, dir.path()).unwrap_err();
        assert!(format!("{err:#}").contains("incomplete"), "{err:#}");
        assert_eq!(
            std::fs::read_dir(dir.path()).unwrap().count(),
            0,
            "no .part left"
        );
        fetch_one(b"abc", &f, dir.path()).unwrap();
        assert_eq!(std::fs::read(dir.path().join("abc.bin")).unwrap(), b"abc");
    }

    #[test]
    fn a_cancelled_download_leaves_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let fake = FakeFetch {
            bodies: HashMap::from([("u".to_string(), b"abc".to_vec())]),
            asked: Mutex::new(Vec::new()),
        };
        let f = abc_file();
        let err = fetch_pinned(
            &fake,
            "u",
            &f,
            &dir.path().join("abc.bin"),
            &AtomicBool::new(true),
            &mut |_| {},
        )
        .unwrap_err();
        assert!(err.to_string().contains("cancelled"));
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    #[test]
    fn download_fetches_model_and_voices_at_the_pinned_revisions() {
        let dir = tempfile::tempdir().unwrap();
        // Every URL answers garbage: the first file fails, nothing is kept,
        // and only pinned huggingface.co URLs were asked.
        let fake = FakeFetch {
            bodies: HashMap::new(),
            asked: Mutex::new(Vec::new()),
        };
        let voice = ITALIAN.voice("giovanni").unwrap();
        let mut seen = Vec::new();
        let err = download(
            &fake,
            dir.path(),
            &ITALIAN,
            &[voice],
            true,
            &AtomicBool::new(false),
            &mut |p| seen.push(p.clone()),
        )
        .unwrap_err();
        assert!(format!("{err:#}").contains("nothing was kept"));
        let asked = fake.asked.lock().unwrap().clone();
        assert_eq!(asked.len(), 1);
        assert!(asked[0].contains(crate::tts::catalog::MODEL_REVISION));
        assert_eq!(
            seen[0].total_bytes,
            ITALIAN.model_bytes() + voice.file.bytes
        );
        assert!(!model_present(dir.path(), &ITALIAN));
    }

    #[test]
    fn presence_needs_the_pinned_size() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!model_present(dir.path(), &ENGLISH));
        let ldir = language_dir(dir.path(), &ENGLISH);
        std::fs::create_dir_all(&ldir).unwrap();
        for f in ENGLISH.files {
            let file = std::fs::File::create(ldir.join(f.name)).unwrap();
            file.set_len(f.bytes).unwrap();
        }
        assert!(model_present(dir.path(), &ENGLISH));
        // One byte short: not present.
        let last = &ENGLISH.files[1];
        std::fs::File::options()
            .write(true)
            .open(ldir.join(last.name))
            .unwrap()
            .set_len(last.bytes - 1)
            .unwrap();
        assert!(!model_present(dir.path(), &ENGLISH));
        let alba = ENGLISH.voice("alba").unwrap();
        assert!(!voice_present(dir.path(), &ENGLISH, alba));
        assert!(bytes_on_disk(dir.path()) > 300_000_000);
    }

    #[test]
    fn deleting_stays_inside_the_module_folder() {
        let dir = tempfile::tempdir().unwrap();
        let alba = ENGLISH.voice("alba").unwrap();
        let v = voice_path(dir.path(), &ENGLISH, alba);
        std::fs::create_dir_all(v.parent().unwrap()).unwrap();
        std::fs::write(&v, b"x").unwrap();
        std::fs::write(dir.path().join("ggml-base.bin"), b"whisper").unwrap();
        delete_voice(dir.path(), &ENGLISH, alba).unwrap();
        assert!(!v.exists());
        delete_voice(dir.path(), &ENGLISH, alba).unwrap(); // already gone: fine
        delete_language(dir.path(), &ENGLISH).unwrap();
        assert!(!language_dir(dir.path(), &ENGLISH).exists());
        delete_all(dir.path()).unwrap();
        assert!(!dir.path().join(ROOT_DIR).exists());
        delete_all(dir.path()).unwrap(); // nothing there: fine
        assert!(
            dir.path().join("ggml-base.bin").exists(),
            "other models untouched"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_linked_module_folder_is_not_followed() {
        let dir = tempfile::tempdir().unwrap();
        let elsewhere = tempfile::tempdir().unwrap();
        std::fs::write(elsewhere.path().join("keep.txt"), b"keep").unwrap();
        std::os::unix::fs::symlink(elsewhere.path(), dir.path().join(ROOT_DIR)).unwrap();
        assert!(delete_all(dir.path()).is_err());
        assert!(elsewhere.path().join("keep.txt").exists());
    }
}
