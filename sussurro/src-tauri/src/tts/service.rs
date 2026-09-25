//! The read-aloud module at run time (#255): the loaded engine (at most
//! one language at a time, dropped after [`IDLE_UNLOAD`] unused like the
//! transcriber), the download job, the preview files and what the UI is
//! told. Commands in `commands.rs` are thin wrappers over this.
//!
//! **P24**: nothing here starts on its own. Downloads, loads and previews
//! all begin with a user action while `Settings.tts_enabled` is on (the
//! commands refuse otherwise); turning the module off cancels a download
//! and unloads the engine.

use super::catalog::{self, Language, Voice, LANGUAGES};
use super::engine::{self, TtsEngine};
use super::models::{self, Progress};
use super::pocket::{PocketOptions, PocketTts};
use super::text::{self, Lang, PrepOptions};
use anyhow::{bail, Context, Result};
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

/// A loaded engine holds 1–2.5 GB of RAM (the Italian 24-layer model is
/// the bigger one) and reloads in a few seconds: drop it after this long
/// unused. Shorter than the transcriber's 15 min — reading is file
/// generation, never latency-critical.
pub const IDLE_UNLOAD: Duration = Duration::from_secs(5 * 60);
/// Folder of preview files, under the app data folder.
pub const PREVIEW_DIR: &str = "tts-preview";
/// Scheme path prefix of preview files (`sussurro-audio:`).
pub const PREVIEW_PREFIX: &str = "tts-preview/";
/// Longest preview text accepted from the UI.
pub const MAX_PREVIEW_CHARS: usize = 400;

struct Loaded {
    models_dir: PathBuf,
    lang: &'static str,
    engine: PocketTts,
}

/// The module's shared state.
pub struct Service {
    engine: Mutex<Option<Loaded>>,
    last_used: Mutex<Option<Instant>>,
    job: Mutex<Option<Progress>>,
    cancel: AtomicBool,
    preview_seq: Mutex<u64>,
}

pub fn global() -> &'static Service {
    static S: OnceLock<Service> = OnceLock::new();
    S.get_or_init(|| Service {
        engine: Mutex::new(None),
        last_used: Mutex::new(None),
        job: Mutex::new(None),
        cancel: AtomicBool::new(false),
        preview_seq: Mutex::new(0),
    })
}

/// A voice as the UI lists it.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct VoiceInfo {
    pub id: String,
    pub label: String,
    pub source: String,
    pub bytes: u64,
    pub downloaded: bool,
    /// The language's default voice (the user's pick, else the catalog's).
    pub selected: bool,
}

/// A language as the UI lists it.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct LanguageInfo {
    pub code: String,
    pub label: String,
    pub variant: String,
    pub model_bytes: u64,
    pub model_downloaded: bool,
    pub voices: Vec<VoiceInfo>,
}

/// What Models → Voices shows.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct TtsStatus {
    pub enabled: bool,
    pub engine: String,
    pub licence: String,
    pub attribution: String,
    pub languages: Vec<LanguageInfo>,
    /// Bytes of this module's files on disk.
    pub bytes_on_disk: u64,
    /// The download in progress, if any.
    pub downloading: Option<Progress>,
    /// Language whose model is loaded now.
    pub loaded: Option<String>,
}

/// The voice to read `lang` with: the user's pick if the catalog has it,
/// else the language's default.
pub fn voice_for(voices: &BTreeMap<String, String>, lang: &Language) -> &'static Voice {
    voices
        .get(lang.code)
        .and_then(|id| lang.voice(id))
        .or_else(|| lang.voice(lang.default_voice))
        .unwrap_or(&lang.voices[0])
}

/// Keep only picks of known languages and voices (settings.json is user
/// editable). Returns whether anything was dropped.
pub fn normalize_voices(voices: &mut BTreeMap<String, String>) -> bool {
    let before = voices.len();
    voices.retain(|code, id| {
        catalog::language(code).is_some_and(|l| l.code == code && l.voice(id).is_some())
    });
    voices.len() != before
}

fn lang(code: &str) -> Result<&'static Language> {
    catalog::language(code).with_context(|| format!("no read-aloud model for '{code}'"))
}

fn voice(lang: &Language, id: &str) -> Result<&'static Voice> {
    lang.voices
        .iter()
        .find(|v| v.id == id)
        .with_context(|| format!("no voice '{id}' for {}", lang.label))
}

impl Service {
    pub fn status(
        &self,
        models_dir: &Path,
        enabled: bool,
        picks: &BTreeMap<String, String>,
    ) -> TtsStatus {
        let languages = LANGUAGES
            .iter()
            .map(|l| {
                let selected = voice_for(picks, l).id;
                LanguageInfo {
                    code: l.code.into(),
                    label: l.label.into(),
                    variant: l.variant.into(),
                    model_bytes: l.model_bytes(),
                    model_downloaded: models::model_present(models_dir, l),
                    voices: l
                        .voices
                        .iter()
                        .map(|v| VoiceInfo {
                            id: v.id.into(),
                            label: v.label.into(),
                            source: v.source.into(),
                            bytes: v.file.bytes,
                            downloaded: models::voice_present(models_dir, l, v),
                            selected: v.id == selected,
                        })
                        .collect(),
                }
            })
            .collect();
        let loaded = self
            .engine
            .try_lock()
            .ok()
            .and_then(|g| g.as_ref().map(|l| l.lang.to_string()));
        TtsStatus {
            enabled,
            engine: "Pocket TTS".into(),
            licence: catalog::MODEL_LICENCE.into(),
            attribution: catalog::ATTRIBUTION.into(),
            languages,
            bytes_on_disk: models::bytes_on_disk(models_dir),
            downloading: self.job.lock().unwrap().clone(),
            loaded,
        }
    }

    /// Download `code`'s model (when missing) and `voice_id` (default: the
    /// language's selected voice), or only the voice when `voice_only`.
    /// One download at a time. `progress` gets snapshots. Blocking.
    pub fn download(
        &self,
        fetch: &dyn models::Fetch,
        models_dir: &Path,
        code: &str,
        voice_id: &str,
        voice_only: bool,
        progress: &mut dyn FnMut(&Progress),
    ) -> Result<()> {
        let lang = lang(code)?;
        let voice = voice(lang, voice_id)?;
        {
            let mut job = self.job.lock().unwrap();
            if job.is_some() {
                bail!("another read-aloud download is running");
            }
            *job = Some(Progress {
                language: lang.code.into(),
                ..Progress::default()
            });
        }
        self.cancel.store(false, Ordering::Relaxed);
        let result = models::download(
            fetch,
            models_dir,
            lang,
            &[voice],
            !voice_only,
            &self.cancel,
            &mut |p| {
                *self.job.lock().unwrap() = Some(p.clone());
                progress(p);
            },
        );
        *self.job.lock().unwrap() = None;
        result
    }

    pub fn cancel_download(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    pub fn is_downloading(&self) -> bool {
        self.job.lock().unwrap().is_some()
    }

    /// Run `f` on the engine for `code` with `voice_id`, loading it (and
    /// dropping another language's) when needed. Blocking: waits for a
    /// render in progress.
    pub fn with_engine<R>(
        &self,
        models_dir: &Path,
        code: &str,
        voice_id: &str,
        opts: PocketOptions,
        f: impl FnOnce(&mut PocketTts) -> Result<R>,
    ) -> Result<R> {
        let lang = lang(code)?;
        let voice = voice(lang, voice_id)?;
        if !models::model_present(models_dir, lang) {
            bail!("the {} read-aloud model is not downloaded", lang.label);
        }
        let voice_path = models::voice_path(models_dir, lang, voice);
        if !models::voice_present(models_dir, lang, voice) {
            bail!("the voice {} is not downloaded", voice.label);
        }
        let mut slot = self.engine.lock().unwrap();
        let reuse = slot
            .as_ref()
            .is_some_and(|l| l.lang == lang.code && l.models_dir == models_dir);
        if !reuse {
            *slot = None; // free the old model before loading the new one
            let started = Instant::now();
            let engine =
                PocketTts::load(&models::language_dir(models_dir, lang), &voice_path, opts)?;
            eprintln!(
                "read-aloud: {} model loaded in {:.1} s",
                lang.label,
                started.elapsed().as_secs_f32()
            );
            *slot = Some(Loaded {
                models_dir: models_dir.to_path_buf(),
                lang: lang.code,
                engine,
            });
        }
        let loaded = slot.as_mut().expect("loaded above");
        if loaded.engine.voice_id() != voice.id {
            loaded.engine.set_voice(&voice_path)?;
        }
        *self.last_used.lock().unwrap() = Some(Instant::now());
        let r = f(&mut loaded.engine);
        *self.last_used.lock().unwrap() = Some(Instant::now());
        r
    }

    /// Drop the engine now (waits for a render in progress).
    pub fn unload(&self) {
        *self.engine.lock().unwrap() = None;
        *self.last_used.lock().unwrap() = None;
    }

    /// Drop the engine if unused for [`IDLE_UNLOAD`]; never waits.
    pub fn unload_if_idle(&self) -> bool {
        crate::pipeline::unload_if_idle(&self.engine, &self.last_used, IDLE_UNLOAD, false)
    }

    /// Render `text` (plain text or markdown) in `code` with `voice_id`
    /// into a new preview WAV in `preview_dir`, deleting older ones.
    /// Returns the file name (see [`preview_file_name`]).
    pub fn preview(
        &self,
        models_dir: &Path,
        preview_dir: &Path,
        code: &str,
        voice_id: &str,
        text: Option<&str>,
    ) -> Result<String> {
        let lang = lang(code)?;
        let text = match text.map(str::trim).filter(|t| !t.is_empty()) {
            Some(t) => t.chars().take(MAX_PREVIEW_CHARS).collect::<String>(),
            None => lang.preview.to_string(),
        };
        let chunks = text::prepare(&text, Lang::from_code(lang.code), &PrepOptions::default());
        if chunks.is_empty() {
            bail!("nothing to read in this text");
        }
        let cancel = AtomicBool::new(false);
        let (rate, audio) =
            self.with_engine(models_dir, code, voice_id, PocketOptions::default(), |e| {
                // The same sentence always sounds the same.
                e.reseed(PocketOptions::default().seed);
                let mut audio = Vec::new();
                engine::render(e, &chunks, &cancel, &mut |_, _| {}, &mut |pcm| {
                    audio.extend_from_slice(pcm);
                    Ok(())
                })?;
                Ok((e.sample_rate(), audio))
            })?;
        std::fs::create_dir_all(preview_dir)
            .with_context(|| format!("creating {}", preview_dir.display()))?;
        clear_previews(preview_dir);
        let n = {
            let mut seq = self.preview_seq.lock().unwrap();
            *seq += 1;
            *seq
        };
        let name = format!("preview-{n}.wav");
        engine::write_wav(&preview_dir.join(&name), rate, &audio)?;
        Ok(name)
    }
}

/// Delete every preview file (at startup, when the module is turned off,
/// before a new preview).
pub fn clear_previews(preview_dir: &Path) {
    let Ok(rd) = std::fs::read_dir(preview_dir) else {
        return;
    };
    for e in rd.flatten() {
        let name = e.file_name();
        let name = name.to_string_lossy();
        if name.starts_with("preview-") && (name.ends_with(".wav") || name.ends_with(".part")) {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

/// A `sussurro-audio:` path (decoded, no leading `/`) that names a preview
/// file → its file name. Only `tts-preview/preview-<digits>.wav`. Pure.
pub fn preview_file_name(path: &str) -> Option<&str> {
    let name = path.strip_prefix(PREVIEW_PREFIX)?;
    let digits = name.strip_prefix("preview-")?.strip_suffix(".wav")?;
    (!digits.is_empty() && digits.len() <= 12 && digits.bytes().all(|b| b.is_ascii_digit()))
        .then_some(name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tts::catalog::{ENGLISH, ITALIAN};

    #[test]
    fn a_voice_pick_falls_back_to_the_default() {
        let mut picks = BTreeMap::new();
        assert_eq!(voice_for(&picks, &ITALIAN).id, "giovanni");
        picks.insert("it".to_string(), "marius".to_string());
        assert_eq!(voice_for(&picks, &ITALIAN).id, "marius");
        picks.insert("en".to_string(), "nobody".to_string());
        assert_eq!(voice_for(&picks, &ENGLISH).id, "alba");
    }

    #[test]
    fn unknown_picks_are_dropped() {
        let mut picks = BTreeMap::from([
            ("it".to_string(), "giovanni".to_string()),
            ("en".to_string(), "jean".to_string()), // non-commercial, not offered
            ("fr".to_string(), "alba".to_string()),
            ("EN".to_string(), "alba".to_string()),
        ]);
        assert!(normalize_voices(&mut picks));
        assert_eq!(
            picks,
            BTreeMap::from([("it".to_string(), "giovanni".to_string())])
        );
        assert!(!normalize_voices(&mut picks));
    }

    #[test]
    fn status_lists_every_language_with_sizes_and_nothing_downloaded() {
        let dir = tempfile::tempdir().unwrap();
        let s = global().status(dir.path(), false, &BTreeMap::new());
        assert!(!s.enabled);
        assert_eq!(s.licence, "CC-BY-4.0");
        assert_eq!(s.languages.len(), 2);
        let it = &s.languages[0];
        assert_eq!(
            (it.code.as_str(), it.model_bytes),
            ("it", ITALIAN.model_bytes())
        );
        assert!(!it.model_downloaded);
        assert!(it.voices.iter().all(|v| !v.downloaded));
        assert_eq!(it.voices.iter().filter(|v| v.selected).count(), 1);
        assert_eq!(s.bytes_on_disk, 0);
    }

    #[test]
    fn loading_needs_the_files() {
        let dir = tempfile::tempdir().unwrap();
        let err = global()
            .with_engine(
                dir.path(),
                "it",
                "giovanni",
                PocketOptions::default(),
                |_| Ok(()),
            )
            .unwrap_err();
        assert!(err.to_string().contains("not downloaded"), "{err}");
        assert!(global()
            .with_engine(dir.path(), "fr", "alba", PocketOptions::default(), |_| Ok(
                ()
            ))
            .is_err());
        assert!(global()
            .with_engine(dir.path(), "en", "jean", PocketOptions::default(), |_| Ok(
                ()
            ))
            .is_err());
    }

    #[test]
    fn preview_paths_name_only_preview_files() {
        assert_eq!(
            preview_file_name("tts-preview/preview-3.wav"),
            Some("preview-3.wav")
        );
        for bad in [
            "tts-preview/preview-.wav",
            "tts-preview/preview-3.opus",
            "tts-preview/../settings.json",
            "tts-preview/preview-3.wav/x",
            "tts-preview/preview-1a.wav",
            "preview-3.wav",
            "2026/09/x/audio.wav",
            "tts-preview/preview-1234567890123.wav",
        ] {
            assert_eq!(preview_file_name(bad), None, "{bad}");
        }
    }

    #[test]
    fn old_previews_are_cleared_and_nothing_else() {
        let dir = tempfile::tempdir().unwrap();
        for f in ["preview-1.wav", "preview-2.wav.part", "keep.txt"] {
            std::fs::write(dir.path().join(f), b"x").unwrap();
        }
        clear_previews(dir.path());
        let left: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        assert_eq!(left, ["keep.txt"]);
        clear_previews(&dir.path().join("missing")); // no folder: fine
    }

    #[test]
    fn a_second_download_waits_for_the_first() {
        struct Never;
        impl models::Fetch for Never {
            fn open(&self, _: &str) -> Result<Box<dyn std::io::Read>> {
                bail!("offline")
            }
        }
        let s = Service {
            engine: Mutex::new(None),
            last_used: Mutex::new(None),
            job: Mutex::new(Some(Progress::default())),
            cancel: AtomicBool::new(false),
            preview_seq: Mutex::new(0),
        };
        let dir = tempfile::tempdir().unwrap();
        let err = s
            .download(&Never, dir.path(), "en", "alba", false, &mut |_| {})
            .unwrap_err();
        assert!(err.to_string().contains("another"), "{err}");
        *s.job.lock().unwrap() = None;
        assert!(s
            .download(&Never, dir.path(), "en", "alba", false, &mut |_| {})
            .is_err());
        assert!(!s.is_downloading(), "the job is cleared after a failure");
        assert!(s
            .download(&Never, dir.path(), "en", "nobody", true, &mut |_| {})
            .is_err());
        assert!(!s.unload_if_idle(), "nothing loaded");
    }
}
