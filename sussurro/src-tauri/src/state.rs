use crate::audio::recorder::Recorder;
use crate::settings::Settings;
use crate::stt::AnyTranscriber;
use std::path::PathBuf;
use std::sync::Mutex;
use tauri::{AppHandle, Manager};

pub struct AppPaths {
    pub settings_file: PathBuf,
    pub models_dir: PathBuf,
    pub history_file: PathBuf,
    pub stats_file: PathBuf,
    /// SQLite search index of the archive (derived data, app data dir).
    pub archive_index: PathBuf,
    /// OS Documents folder, `None` when the platform can't tell (Linux
    /// without XDG user dirs) — the archive then falls back to `$HOME`.
    pub documents_dir: Option<PathBuf>,
    pub home_dir: Option<PathBuf>,
}

impl AppPaths {
    pub fn from_app(app: &AppHandle) -> Self {
        let config = app.path().app_config_dir().expect("app config dir");
        let data = app.path().app_data_dir().expect("app data dir");
        Self {
            settings_file: config.join("settings.json"),
            models_dir: data.join("models"),
            history_file: data.join("history.jsonl"),
            stats_file: data.join("stats.json"),
            archive_index: data.join(crate::archive::INDEX_FILE),
            documents_dir: app.path().document_dir().ok(),
            home_dir: app.path().home_dir().ok(),
        }
    }
}

/// The models directory honouring the user override (empty = app data default).
pub fn resolve_models_dir(paths: &AppPaths, settings: &Settings) -> PathBuf {
    let custom = settings.models_dir.trim();
    if custom.is_empty() {
        paths.models_dir.clone()
    } else {
        PathBuf::from(custom)
    }
}

/// The archive folder honouring the user override (empty = `<Documents>/Sussurro`).
/// Pure path resolution: nothing is created or touched on disk, so asking for
/// it never triggers the macOS Documents permission prompt.
pub fn resolve_archive_dir(paths: &AppPaths, settings: &Settings) -> anyhow::Result<PathBuf> {
    crate::archive::resolve_archive_dir(
        paths.documents_dir.clone(),
        paths.home_dir.clone(),
        &settings.archive_dir,
    )
}

/// What a transcriber was loaded for. When the settings no longer match,
/// whoever next takes the transcriber lock reloads it (see
/// `pipeline::lock_transcriber`) — so changing the model never has to wait
/// for the lock (#154).
#[derive(Debug, Clone, PartialEq)]
pub struct ModelKey {
    pub engine: crate::settings::SttEngine,
    /// Whisper model file name; empty for Parakeet (one model).
    pub whisper_model: String,
    pub models_dir: PathBuf,
}

impl ModelKey {
    pub fn of(paths: &AppPaths, settings: &Settings) -> Self {
        use crate::settings::SttEngine;
        Self {
            engine: settings.engine.clone(),
            whisper_model: match settings.engine {
                SttEngine::Whisper => settings.whisper_model.clone(),
                SttEngine::Parakeet => String::new(),
            },
            models_dir: resolve_models_dir(paths, settings),
        }
    }
}

/// A loaded model and the settings it was loaded for.
pub struct Loaded<T> {
    pub key: ModelKey,
    pub model: T,
}

pub struct AppState {
    pub recorder: Mutex<Recorder>,
    /// Lazily loaded on first dictation; reloaded by the next user when the
    /// engine/model settings change, and dropped again after idle time (see
    /// pipeline's idle unload).
    pub transcriber: Mutex<Option<Loaded<AnyTranscriber>>>,
    /// When the transcriber was last loaded or used — drives the idle unload.
    /// Written while holding the `transcriber` lock so load/refresh stays
    /// atomic with respect to the unloader.
    pub transcriber_last_used: Mutex<Option<std::time::Instant>>,
    pub settings: Mutex<Settings>,
    pub paths: AppPaths,
    /// True while the recorder is running only to feed the mic-test VU meter.
    pub mic_test: std::sync::atomic::AtomicBool,
    /// Streaming-injection progress for the recording in flight.
    pub stream: Mutex<StreamState>,
    /// Long-form engine sessions (mic and file, #113).
    pub engine: crate::engine::session::Sessions,
    /// A hotkey dictation is recording or waiting for its final pass: the
    /// engine starts no new segment meanwhile (#154).
    pub dictation: crate::engine::priority::DictationGate,
    /// Recipe runs in flight, one per archive item (#120).
    pub recipe_runs: crate::recipes::run::Runs,
    /// Ask panel answers not saved yet, in memory only (#121).
    pub recipe_answers: crate::recipes::answer::Answers,
    /// One-time confirmations for runs on external LLM profiles (#122).
    pub consents: crate::llm::consent::ConsentStore,
}

/// What streaming injection has already done for the current recording.
#[derive(Default)]
pub struct StreamState {
    /// Prefix of the RAW transcript already consumed (matched against new partials).
    pub raw_consumed: String,
    /// Text actually typed into the target app (cleaned when cleanup is on).
    pub injected: String,
    /// App focused when recording started — used for per-app styles mid-stream.
    pub target_app: String,
}

impl StreamState {
    pub fn reset(&mut self, target_app: String) {
        self.raw_consumed.clear();
        self.injected.clear();
        self.target_app = target_app;
    }
}
