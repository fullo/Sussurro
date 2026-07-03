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
}

impl AppPaths {
    pub fn from_app(app: &AppHandle) -> Self {
        let config = app.path().app_config_dir().expect("app config dir");
        let data = app.path().app_data_dir().expect("app data dir");
        Self {
            settings_file: config.join("settings.json"),
            models_dir: data.join("models"),
            history_file: data.join("history.jsonl"),
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

pub struct AppState {
    pub recorder: Mutex<Recorder>,
    /// Lazily loaded on first dictation; reset to None when engine/model change.
    pub transcriber: Mutex<Option<AnyTranscriber>>,
    pub settings: Mutex<Settings>,
    pub paths: AppPaths,
    /// True while the current recording was started by the command hotkey.
    pub command_mode: std::sync::atomic::AtomicBool,
    /// Streaming-injection progress for the recording in flight.
    pub stream: Mutex<StreamState>,
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
