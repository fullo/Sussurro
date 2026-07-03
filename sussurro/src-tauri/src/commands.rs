use crate::history::{self, HistoryEntry};
use crate::hotkey;
use crate::settings::Settings;
use crate::state::AppState;
use crate::stt::models;
use tauri::{AppHandle, Manager, State};
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
        let cleaned = crate::cleanup::ollama::cleanup(&settings, None, &raw);
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
    // Retention rides the refresh: cheap no-op unless something expired.
    let days = state.settings.lock().unwrap().history_retention_days;
    let _ = history::prune_older_than(&state.paths.history_file, days);
    history::read_last(&state.paths.history_file, n)
}

/// Full-text search over the whole history (raw + cleaned), newest first.
#[tauri::command]
pub fn search_history(state: State<'_, AppState>, query: String, n: usize) -> Vec<HistoryEntry> {
    history::search(&state.paths.history_file, &query, n)
}

#[tauri::command]
pub fn clear_history(state: State<'_, AppState>) -> Result<(), String> {
    history::clear(&state.paths.history_file).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn list_input_devices() -> Vec<String> {
    crate::audio::recorder::list_input_devices()
}

/// Start the recorder purely to feed the mic-test VU meter (no transcription).
#[tauri::command]
pub fn start_mic_test(state: State<'_, AppState>) -> Result<(), String> {
    let mut recorder = state.recorder.lock().unwrap();
    if recorder.is_recording() {
        return Err("already recording".to_string());
    }
    let device = state.settings.lock().unwrap().input_device.clone();
    recorder.start(&device).map_err(|e| e.to_string())?;
    state.mic_test.store(true, std::sync::atomic::Ordering::Relaxed);
    Ok(())
}

#[tauri::command]
pub fn stop_mic_test(state: State<'_, AppState>) -> Result<(), String> {
    if !state.mic_test.swap(false, std::sync::atomic::Ordering::Relaxed) {
        return Ok(()); // dictation took over (or never started) — nothing to stop
    }
    let _ = state.recorder.lock().unwrap().stop(); // samples discarded
    Ok(())
}

/// Live input level (RMS of the last ~100 ms), 0.0 when not recording.
#[tauri::command]
pub fn mic_level(state: State<'_, AppState>) -> f32 {
    state.recorder.lock().unwrap().level().unwrap_or(0.0)
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

/// Write the portable config (dictionary, snippets, app styles) to `path`.
#[tauri::command]
pub fn export_config(state: State<'_, AppState>, path: String) -> Result<(), String> {
    let settings = state.settings.lock().unwrap().clone();
    crate::config_io::export_to(std::path::Path::new(&path), &settings)
        .map_err(|e| e.to_string())
}

/// Merge a config bundle from `path` into settings. Returns a summary string.
#[tauri::command]
pub fn import_config(state: State<'_, AppState>, path: String) -> Result<String, String> {
    let bundle = crate::config_io::load_bundle(std::path::Path::new(&path))
        .map_err(|e| format!("could not read config: {e:#}"))?;
    let (w, sn, st) = {
        let mut settings = state.settings.lock().unwrap();
        let counts = bundle.merge_into(&mut settings);
        settings
            .save(&state.paths.settings_file)
            .map_err(|e| e.to_string())?;
        counts
    };
    Ok(format!("Imported {w} words, {sn} snippets, {st} app styles"))
}

/// Transcribe an audio file's bytes (from a file input) and clean the result
/// with the current settings. Appends a history entry; returns (raw, cleaned).
#[tauri::command]
pub async fn transcribe_audio_file(
    app: AppHandle,
    bytes: Vec<u8>,
    ext: String,
) -> Result<HistoryEntry, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let samples = crate::audio::decode::decode_bytes_16k_mono(bytes, &ext)
            .map_err(|e| format!("{e:#}"))?;
        let state = app.state::<AppState>();
        let (raw, cleaned) =
            crate::pipeline::transcribe_batch(&state, &samples).map_err(|e| format!("{e:#}"))?;
        Ok(HistoryEntry {
            timestamp: chrono::Utc::now().to_rfc3339(),
            raw,
            cleaned,
        })
    })
    .await
    .map_err(|e| e.to_string())?
}

/// One-off translation of a past history entry: translate `text` into `lang`
/// via the translate-only LLM path (cleanup None + target language), append
/// the result as a new history entry and return it.
#[tauri::command]
pub async fn translate_entry(
    state: State<'_, AppState>,
    text: String,
    lang: String,
) -> Result<HistoryEntry, String> {
    let (mut settings, history_file) = {
        let s = state.settings.lock().unwrap().clone();
        (s, state.paths.history_file.clone())
    };
    settings.cleanup_level = crate::settings::CleanupLevel::None;
    settings.output_language = lang;
    tauri::async_runtime::spawn_blocking(move || {
        let translated = crate::cleanup::ollama::cleanup(&settings, None, &text);
        if translated == text {
            // cleanup() falls back to the input on any Ollama error.
            return Err("translation failed — is Ollama running?".to_string());
        }
        let entry = HistoryEntry {
            timestamp: chrono::Utc::now().to_rfc3339(),
            raw: text,
            cleaned: translated,
        };
        let _ = history::append(&history_file, &entry);
        Ok(entry)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Ollama environment status for the setup banner.
#[derive(serde::Serialize)]
pub struct OllamaStatus {
    /// Binary found on PATH (or the server answered — installed for sure).
    pub installed: bool,
    /// The HTTP server answered /api/tags.
    pub running: bool,
    /// The configured cleanup model is present on the server.
    pub has_model: bool,
}

fn ollama_binary_on_path() -> bool {
    let finder = if cfg!(windows) { "where" } else { "which" };
    std::process::Command::new(finder)
        .arg("ollama")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[tauri::command]
pub async fn ollama_status(state: State<'_, AppState>) -> Result<OllamaStatus, String> {
    let (url, model) = {
        let s = state.settings.lock().unwrap();
        (s.ollama_url.clone(), s.ollama_model.clone())
    };
    tauri::async_runtime::spawn_blocking(move || {
        let models = crate::cleanup::ollama::list_models(&url).ok();
        let running = models.is_some();
        let has_model = models
            .map(|ms| {
                ms.iter()
                    .any(|m| m == &model || m.starts_with(&format!("{model}:")))
            })
            .unwrap_or(false);
        Ok(OllamaStatus {
            installed: running || ollama_binary_on_path(),
            running,
            has_model,
        })
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Pull the configured cleanup model on the Ollama server (blocking, can take
/// minutes for a ~2 GB model).
#[tauri::command]
pub async fn pull_ollama_model(state: State<'_, AppState>) -> Result<(), String> {
    let (url, model) = {
        let s = state.settings.lock().unwrap();
        (s.ollama_url.clone(), s.ollama_model.clone())
    };
    tauri::async_runtime::spawn_blocking(move || {
        let client = reqwest::blocking::Client::builder()
            .timeout(None)
            .build()
            .map_err(|e| e.to_string())?;
        client
            .post(format!("{}/api/pull", url.trim_end_matches('/')))
            .json(&serde_json::json!({"name": model, "stream": false}))
            .send()
            .map_err(|e| e.to_string())?
            .error_for_status()
            .map_err(|e| e.to_string())?;
        Ok(())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Built-in cleanup instructions per level — shown as placeholders under the
/// user's prompt overrides so the two never drift apart.
#[tauri::command]
pub fn get_default_prompts() -> [String; 3] {
    [
        crate::cleanup::prompt::DEFAULT_LIGHT.to_string(),
        crate::cleanup::prompt::DEFAULT_MEDIUM.to_string(),
        crate::cleanup::prompt::DEFAULT_HIGH.to_string(),
    ]
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
