use crate::audio::recorder::Recorder;
use crate::settings::Settings;
use crate::stt::whisper::Transcriber;
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

pub struct AppState {
    pub recorder: Mutex<Recorder>,
    /// Lazily loaded on first dictation; reset to None when the model changes.
    pub transcriber: Mutex<Option<Transcriber>>,
    pub settings: Mutex<Settings>,
    pub paths: AppPaths,
}
