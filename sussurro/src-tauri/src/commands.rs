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
    hotkey::apply(&app, &settings.hotkey, &settings.command_hotkey).map_err(|e| e.to_string())?;
    // Only touch the OS launch entry when the state actually changes:
    // disabling a never-registered entry fails with os error 2 on Windows.
    let autolaunch = app.autolaunch();
    let currently_enabled = autolaunch.is_enabled().unwrap_or(false);
    if settings.autostart && !currently_enabled {
        autolaunch.enable().map_err(|e| e.to_string())?;
    } else if !settings.autostart && currently_enabled {
        autolaunch.disable().map_err(|e| e.to_string())?;
    }
    settings
        .save(&state.paths.settings_file)
        .map_err(|e| e.to_string())?;
    let model_changed = {
        let mut current = state.settings.lock().unwrap();
        let changed = current.whisper_model != settings.whisper_model
            || current.engine != settings.engine
            || current.models_dir != settings.models_dir;
        *current = settings;
        changed
    };
    if model_changed {
        *state.transcriber.lock().unwrap() = None; // reload lazily with the new engine/model
    }
    Ok(())
}

/// Drive dictation from the in-app Dictate button: mirrors the global hotkey
/// press/release, so push-to-talk vs toggle behaves identically.
#[tauri::command]
pub fn trigger_dictation(app: AppHandle, pressed: bool) {
    crate::pipeline::handle_trigger(&app, pressed, false);
}

#[tauri::command]
pub fn copy_text(text: String) -> Result<(), String> {
    arboard::Clipboard::new()
        .and_then(|mut c| c.set_text(text))
        .map_err(|e| e.to_string())
}

/// Re-run cleanup on a past raw transcript with the CURRENT settings
/// (level/model/dictionary). Appends the result as a new history entry.
#[tauri::command]
pub async fn reclean(state: State<'_, AppState>, raw: String) -> Result<HistoryEntry, String> {
    let (settings, history_file) = {
        let s = state.settings.lock().unwrap().clone();
        (s, state.paths.history_file.clone())
    };
    tauri::async_runtime::spawn_blocking(move || {
        // Re-clean has no target app: no per-app style.
        let cleaned = crate::cleanup::ollama::cleanup(
            &settings.ollama_url,
            &settings.ollama_model,
            &settings.cleanup_level,
            &settings.dictionary,
            None,
            &raw,
        );
        let entry = HistoryEntry {
            timestamp: chrono::Utc::now().to_rfc3339(),
            raw,
            cleaned,
        };
        let _ = history::append(&history_file, &entry);
        Ok(entry)
    })
    .await
    .map_err(|e| e.to_string())?
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
pub fn list_input_devices() -> Vec<String> {
    crate::audio::recorder::list_input_devices()
}

#[tauri::command]
pub fn model_is_downloaded(state: State<'_, AppState>) -> bool {
    let settings = state.settings.lock().unwrap();
    let models_dir = crate::state::resolve_models_dir(&state.paths, &settings);
    match settings.engine {
        crate::settings::SttEngine::Whisper => {
            models::model_exists(&models_dir, &settings.whisper_model)
        }
        crate::settings::SttEngine::Parakeet => models::parakeet_exists(&models_dir),
    }
}

/// Save a user correction of a past transcript: new words are auto-added to
/// the personal dictionary (Wispr-style learning) and the corrected text is
/// appended to history. Returns the words learned.
#[tauri::command]
pub fn learn_correction(
    state: State<'_, AppState>,
    raw: String,
    original: String,
    corrected: String,
) -> Result<Vec<String>, String> {
    let learned = {
        let mut settings = state.settings.lock().unwrap();
        let learned = crate::snippets::learned_words(&original, &corrected, &settings.dictionary);
        if !learned.is_empty() {
            settings.dictionary.extend(learned.iter().cloned());
            settings
                .save(&state.paths.settings_file)
                .map_err(|e| e.to_string())?;
        }
        learned
    };
    history::append(
        &state.paths.history_file,
        &HistoryEntry {
            timestamp: chrono::Utc::now().to_rfc3339(),
            raw,
            cleaned: corrected,
        },
    )
    .map_err(|e| e.to_string())?;
    Ok(learned)
}

/// Models available on the configured Ollama server. Errors when unreachable —
/// the frontend falls back to a free-text field.
#[tauri::command]
pub async fn list_ollama_models(state: State<'_, AppState>) -> Result<Vec<String>, String> {
    let url = { state.settings.lock().unwrap().ollama_url.clone() };
    tauri::async_runtime::spawn_blocking(move || {
        crate::cleanup::ollama::list_models(&url).map_err(|e| format!("{e:#}"))
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Blocking download (~0.5–3 GB) run off the async runtime.
#[tauri::command]
pub async fn download_model(state: State<'_, AppState>) -> Result<String, String> {
    let (dir, file, engine) = {
        let settings = state.settings.lock().unwrap();
        (
            crate::state::resolve_models_dir(&state.paths, &settings),
            settings.whisper_model.clone(),
            settings.engine.clone(),
        )
    };
    tauri::async_runtime::spawn_blocking(move || {
        let result = match engine {
            crate::settings::SttEngine::Whisper => models::ensure_model(&dir, &file),
            crate::settings::SttEngine::Parakeet => models::ensure_parakeet(&dir),
        };
        result
            .map(|p| p.display().to_string())
            .map_err(|e| format!("{e:#}"))
    })
    .await
    .map_err(|e| e.to_string())?
}
