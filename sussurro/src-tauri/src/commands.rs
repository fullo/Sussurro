use crate::history::{self, HistoryEntry};
use crate::hotkey;
use crate::settings::Settings;
use crate::state::AppState;
use crate::stt::models;
use tauri::{AppHandle, State};
use tauri_plugin_autostart::ManagerExt;

#[tauri::command]
pub fn get_settings(state: State<'_, AppState>) -> Settings {
    state.settings.lock().unwrap().clone()
}

#[tauri::command]
pub fn set_settings(
    app: AppHandle,
    state: State<'_, AppState>,
    settings: Settings,
) -> Result<(), String> {
    hotkey::apply(&app, &settings.hotkey).map_err(|e| e.to_string())?;
    let autolaunch = app.autolaunch();
    if settings.autostart {
        autolaunch.enable().map_err(|e| e.to_string())?;
    } else {
        autolaunch.disable().map_err(|e| e.to_string())?;
    }
    settings
        .save(&state.paths.settings_file)
        .map_err(|e| e.to_string())?;
    let model_changed = {
        let mut current = state.settings.lock().unwrap();
        let changed = current.whisper_model != settings.whisper_model;
        *current = settings;
        changed
    };
    if model_changed {
        *state.transcriber.lock().unwrap() = None; // reload lazily with the new model
    }
    Ok(())
}

#[tauri::command]
pub fn get_history(state: State<'_, AppState>, n: usize) -> Vec<HistoryEntry> {
    history::read_last(&state.paths.history_file, n)
}

#[tauri::command]
pub fn clear_history(state: State<'_, AppState>) -> Result<(), String> {
    history::clear(&state.paths.history_file).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn model_is_downloaded(state: State<'_, AppState>) -> bool {
    let settings = state.settings.lock().unwrap();
    models::model_exists(&state.paths.models_dir, &settings.whisper_model)
}

/// Blocking download (~0.5–3 GB) run off the async runtime.
#[tauri::command]
pub async fn download_model(state: State<'_, AppState>) -> Result<String, String> {
    let (dir, file) = {
        let settings = state.settings.lock().unwrap();
        (state.paths.models_dir.clone(), settings.whisper_model.clone())
    };
    tauri::async_runtime::spawn_blocking(move || {
        models::ensure_model(&dir, &file)
            .map(|p| p.display().to_string())
            .map_err(|e| format!("{e:#}"))
    })
    .await
    .map_err(|e| e.to_string())?
}
