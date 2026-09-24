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
    mut settings: Settings,
) -> Result<(), String> {
    // Valid LLM profiles and a cleanup selection that names one (#119).
    settings.normalize();
    // The model name flows into models_dir.join(name) for download and load —
    // reject traversal/absolute paths before anything touches the filesystem.
    models::validate_model_name(&settings.whisper_model).map_err(|e| e.to_string())?;
    hotkey::apply(&app, &settings.hotkey).map_err(|e| e.to_string())?;
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
    // Only the settings lock (never held long) is read here: the workspace
    // layout follows a ui_v2 toggle (#114).
    let workspace = {
        let current = state.settings.lock().unwrap();
        (current.ui_v2 != settings.ui_v2).then_some(settings.ui_v2)
    };
    // Main thread: must never wait for the transcriber (#154).
    crate::pipeline::swap_settings(&state, settings);
    if let Some(on) = workspace {
        crate::apply_main_window_layout(&app, on);
    }
    Ok(())
}

/// Drive dictation from the in-app Dictate button: mirrors the global hotkey
/// press/release, so push-to-talk vs toggle behaves identically.
#[tauri::command]
pub fn trigger_dictation(app: AppHandle, pressed: bool) {
    crate::pipeline::handle_trigger(&app, pressed);
}

#[tauri::command]
pub fn copy_text(text: String) -> Result<(), String> {
    // Shared with paste-injection so it persists on Linux (a transient arboard
    // set would silently vanish — see inject::set_clipboard).
    crate::inject::set_clipboard(&text).map_err(|e| format!("{e:#}"))
}

/// OS permission status (microphone, and accessibility for paste injection).
#[tauri::command]
pub fn check_permissions() -> crate::permissions::Permissions {
    crate::permissions::check()
}

/// Open the OS privacy pane for a permission ("microphone" / "accessibility").
#[tauri::command]
pub fn open_settings(target: String) -> Result<(), String> {
    crate::permissions::open_settings(&target).map_err(|e| e.to_string())
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

/// Export the whole history to a file: Markdown when the path ends in .md,
/// pretty JSON otherwise. Entries are written oldest-first (chronological).
#[tauri::command]
pub fn export_history(state: State<'_, AppState>, path: String) -> Result<String, String> {
    let mut entries = history::read_last(&state.paths.history_file, usize::MAX);
    entries.reverse(); // read_last is newest-first
    if entries.is_empty() {
        return Err("history is empty — nothing to export".to_string());
    }
    let body = if path.to_lowercase().ends_with(".md") {
        history::to_markdown(&entries)
    } else {
        serde_json::to_string_pretty(&entries).map_err(|e| e.to_string())?
    };
    std::fs::write(&path, body).map_err(|e| e.to_string())?;
    Ok(format!("{} entries exported.", entries.len()))
}

#[derive(serde::Serialize)]
pub struct UsageStats {
    pub total_dictations: u64,
    pub total_words: u64,
    pub today_dictations: u64,
    pub today_words: u64,
    pub week_dictations: u64,
    pub week_words: u64,
}

/// Persistent dictation counters — unaffected by history retention/clearing.
#[tauri::command]
pub fn usage_stats(state: State<'_, AppState>) -> UsageStats {
    let stats = crate::stats::load(&state.paths.stats_file);
    let now = chrono::Local::now();
    let today = now.format("%Y-%m-%d").to_string();
    let week_start = (now - chrono::Duration::days(6)).format("%Y-%m-%d").to_string();
    let t = stats.days.get(&today).cloned().unwrap_or_default();
    let (week_dictations, week_words) = stats
        .days
        .range(week_start..)
        .fold((0, 0), |(d, w), (_, v)| (d + v.dictations, w + v.words));
    UsageStats {
        total_dictations: stats.total_dictations,
        total_words: stats.total_words,
        today_dictations: t.dictations,
        today_words: t.words,
        week_dictations,
        week_words,
    }
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

/// GGML whisper models (`ggml-*.bin`) already present in the models folder, so
/// files shared with other whisper.cpp tools show up in the picker without a
/// re-download. Point Settings → Models folder at a shared directory to reuse.
#[tauri::command]
pub fn list_whisper_models(state: State<'_, AppState>) -> Vec<String> {
    let settings = state.settings.lock().unwrap();
    let models_dir = crate::state::resolve_models_dir(&state.paths, &settings);
    models::list_ggml_models(&models_dir)
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

/// Bulk import in Settings (#156): open the native file picker FROM RUST,
/// filtered to the one extension `kind` accepts (.txt dictionary / .csv
/// snippets), and return the picked file's name and text, or `None` if the
/// user cancelled. No path crosses IPC in either direction, so a compromised
/// webview can't point this at an arbitrary file; the frontend parses and
/// merges the text.
///
/// Only the main window may call it: the overlay shares the default
/// capability (and app commands aren't capability-scoped here), so the
/// calling window's label is checked. Tauri sets that label from the IPC
/// origin, the page can't forge it.
#[tauri::command]
pub async fn pick_import_file(
    window: tauri::WebviewWindow,
    kind: crate::config_io::ImportKind,
) -> Result<Option<crate::config_io::ImportFile>, String> {
    use tauri_plugin_dialog::DialogExt;

    crate::config_io::check_import_caller(window.label())?;
    let dialog = window
        .dialog()
        .file()
        .set_title(kind.dialog_title())
        .add_filter(kind.filter_name(), &[kind.extension()])
        .set_parent(&window);
    // The blocking picker must stay off the main thread and off the async
    // workers: it waits for the user.
    tauri::async_runtime::spawn_blocking(move || {
        let Some(picked) = dialog.blocking_pick_file() else {
            return Ok(None);
        };
        let path = picked
            .into_path()
            .map_err(|e| format!("could not read file: {e}"))?;
        crate::config_io::load_import_file(&path, kind)
            .map(Some)
            .map_err(|e| format!("could not read file: {e:#}"))
    })
    .await
    .map_err(|e| e.to_string())?
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

// ---- Long-form engine (0.7, #113): mic sessions and files → archive items ----

/// What a finished engine run produced (also sent as `engine-done`).
#[derive(serde::Serialize)]
pub struct EngineResult {
    /// The run's session id (as in its `engine-*` events; `engine-started`
    /// announces it while the run is still going).
    pub session_id: u64,
    pub item_id: String,
    pub item_type: crate::archive::ItemType,
    pub title: String,
    /// The cleaned transcript as plain text.
    pub text: String,
    pub segments: usize,
    pub duration_s: f64,
}

/// Transcribe an audio file from its path (streamed decode, VAD segments,
/// chunked cleanup) into an archive item of the chosen type — note by
/// default, or transcription (P10). Resolves when the item is written;
/// progress arrives as `engine-progress` / `engine-segment` events.
/// `language` / `cleanup_level`: this run only (#157); omitted = the
/// dictation settings, which are never modified.
#[tauri::command]
pub async fn transcribe_file(
    app: AppHandle,
    path: String,
    item_type: Option<crate::archive::ItemType>,
    title: Option<String>,
    language: Option<String>,
    cleanup_level: Option<crate::settings::CleanupLevel>,
) -> Result<EngineResult, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let r = crate::engine::session::transcribe_file(
            &app,
            std::path::Path::new(&path),
            item_type.unwrap_or_default(),
            title.unwrap_or_default(),
            crate::engine::session::RunOptions {
                language,
                cleanup_level,
            },
        )
        .map_err(|e| format!("{e:#}"))?;
        Ok(EngineResult {
            session_id: r.session_id,
            item_id: r.item_id,
            item_type: r.meta.item_type,
            title: r.meta.title,
            text: r.text,
            segments: r.segments,
            duration_s: r.duration_ms as f64 / 1000.0,
        })
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Start a long microphone session (independent of the dictation hotkey).
/// Returns the session id; the item arrives as `engine-done` after
/// `engine_stop_mic`. `defer`: transcribe only after the stop, for slow
/// machines. `language` / `cleanup_level`: this run only (#157); omitted =
/// the dictation settings, which are never modified.
#[tauri::command]
pub fn engine_start_mic(
    app: AppHandle,
    item_type: Option<crate::archive::ItemType>,
    title: Option<String>,
    defer: Option<bool>,
    language: Option<String>,
    cleanup_level: Option<crate::settings::CleanupLevel>,
) -> Result<u64, String> {
    crate::engine::session::start_mic(
        &app,
        item_type.unwrap_or_default(),
        title.unwrap_or_default(),
        defer.unwrap_or(false),
        crate::engine::session::RunOptions {
            language,
            cleanup_level,
        },
    )
    .map_err(|e| format!("{e:#}"))
}

/// Stop recording the mic session; the queued segments are still
/// transcribed and the item written. Returns the session id.
#[tauri::command]
pub fn engine_stop_mic(state: State<'_, AppState>) -> Result<u64, String> {
    state
        .engine
        .stop_mic()
        .ok_or_else(|| "no microphone session is running".to_string())
}

/// Abort a session (mic or file): nothing is written. False if unknown.
#[tauri::command]
pub fn engine_cancel(state: State<'_, AppState>, session_id: u64) -> bool {
    state.engine.cancel(session_id)
}

#[derive(serde::Serialize)]
pub struct EngineStatus {
    /// Sessions running (mic + files).
    pub active: usize,
    /// The running mic session, if any.
    pub mic_session: Option<u64>,
    /// Running file transcriptions, oldest first (#158): a UI mounted
    /// mid-run (window reload, `ui_v2` switched) adopts them, so a file
    /// started from the other UI can still be followed and cancelled.
    pub file_sessions: Vec<FileSessionStatus>,
}

#[derive(serde::Serialize)]
pub struct FileSessionStatus {
    pub session_id: u64,
    /// The file's name.
    pub label: String,
}

#[tauri::command]
pub fn engine_status(state: State<'_, AppState>) -> EngineStatus {
    EngineStatus {
        active: state.engine.active_count(),
        mic_session: state.engine.mic_session(),
        file_sessions: state
            .engine
            .file_sessions()
            .into_iter()
            .map(|(session_id, label)| FileSessionStatus { session_id, label })
            .collect(),
    }
}

/// One-off translation of a past history entry. Starts from the RAW
/// transcript — not from text a model already rewrote (double LLM passes
/// compound errors) — and runs the everyday dictation prompt: current
/// cleanup level with the target language forced. Like Re-clean, but into
/// another language. Appends the result as a new history entry.
#[tauri::command]
pub async fn translate_entry(
    state: State<'_, AppState>,
    raw: String,
    lang: String,
) -> Result<HistoryEntry, String> {
    let (mut settings, history_file) = {
        let s = state.settings.lock().unwrap().clone();
        (s, state.paths.history_file.clone())
    };
    settings.output_language = lang;
    tauri::async_runtime::spawn_blocking(move || {
        let translated = crate::cleanup::ollama::cleanup(&settings, None, &raw);
        if translated == raw {
            // cleanup() falls back to the input on any LLM error.
            return Err("translation failed — is the cleanup server running?".to_string());
        }
        let entry = HistoryEntry {
            timestamp: chrono::Utc::now().to_rfc3339(),
            raw,
            cleaned: translated,
        };
        let _ = history::append(&history_file, &entry);
        Ok(entry)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Cleanup server status for the setup banner, for the cleanup profile.
#[derive(serde::Serialize)]
pub struct OllamaStatus {
    /// Binary found on PATH (or the server answered — installed for sure).
    pub installed: bool,
    /// The profile's server answered its model listing.
    pub running: bool,
    /// The profile's model is present on the server.
    pub has_model: bool,
}

/// Whether `model` is in `models`, Ollama-style: `llama3.2` matches
/// `llama3.2:latest`.
fn model_listed(models: &[String], model: &str) -> bool {
    models
        .iter()
        .any(|m| m == model || m.starts_with(&format!("{model}:")))
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
    let profile = state.settings.lock().unwrap().cleanup_llm();
    tauri::async_runtime::spawn_blocking(move || {
        let models = crate::cleanup::ollama::list_models(&profile).ok();
        let running = models.is_some();
        let has_model = models.is_some_and(|ms| model_listed(&ms, &profile.model));
        Ok(OllamaStatus {
            installed: running || ollama_binary_on_path(),
            running,
            has_model,
        })
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Human-readable environment report for bug reports — the footer's
/// "Copy diagnostics" button puts this on the clipboard. Configuration only:
/// no history content, no dictionary words, no snippet texts.
#[tauri::command]
pub async fn diagnostics(state: State<'_, AppState>) -> Result<String, String> {
    let settings = state.settings.lock().unwrap().clone();
    let models_dir = crate::state::resolve_models_dir(&state.paths, &settings);
    let (engine, stt_model, model_ready) = match settings.engine {
        crate::settings::SttEngine::Whisper => (
            "whisper",
            settings.whisper_model.clone(),
            models::model_exists(&models_dir, &settings.whisper_model),
        ),
        crate::settings::SttEngine::Parakeet => (
            "parakeet",
            "parakeet-tdt-0.6b-v3-int8".to_string(),
            models::parakeet_exists(&models_dir),
        ),
    };
    tauri::async_runtime::spawn_blocking(move || {
        use std::fmt::Write as _;
        let profile = settings.cleanup_llm();
        let models = crate::cleanup::ollama::list_models(&profile).ok();
        let llm_running = models.is_some();
        let llm_has_model = models.is_some_and(|ms| model_listed(&ms, &profile.model));

        let mut r = String::new();
        let _ = writeln!(r, "Sussurro {} — diagnostics", env!("CARGO_PKG_VERSION"));
        let _ = writeln!(r, "OS: {} {}", std::env::consts::OS, std::env::consts::ARCH);
        let _ = writeln!(
            r,
            "STT: {engine} · model {stt_model} (downloaded: {model_ready}) · language {}",
            if settings.language.is_empty() { "auto" } else { &settings.language }
        );
        let _ = writeln!(
            r,
            "Models dir: {}",
            if settings.models_dir.trim().is_empty() { "(default)" } else { &settings.models_dir }
        );
        let _ = writeln!(
            r,
            "Microphone: {}",
            if settings.input_device.is_empty() { "(system default)" } else { &settings.input_device }
        );
        let _ = writeln!(
            r,
            "Hotkey: dictation {} ({})",
            settings.hotkey,
            if settings.push_to_talk { "push-to-talk" } else { "toggle" }
        );
        let _ = writeln!(
            r,
            "Cleanup: {:?} · profile \"{}\" ({:?}, {}) · {} @ {} (running: {llm_running}, model present: {llm_has_model}) · {} profile(s)",
            settings.cleanup_level,
            profile.name,
            profile.api,
            if profile.external { "external" } else { "local" },
            profile.model,
            profile.base_url,
            settings.llm_profiles.len()
        );
        let _ = writeln!(
            r,
            "Translate to: {}",
            if settings.output_language.is_empty() || settings.output_language == "same" {
                "(off)"
            } else {
                &settings.output_language
            }
        );
        let _ = writeln!(
            r,
            "Toggles: live_preview={} stream_injection={} voice_commands={} whisper_mode={} sound={} autostart={}",
            settings.live_preview,
            settings.stream_injection,
            settings.voice_commands,
            settings.whisper_mode,
            settings.sound_feedback,
            settings.autostart
        );
        let o = &settings.prompt_overrides;
        let _ = writeln!(
            r,
            "Personalization: {} dictionary words · {} snippets · {} app styles · custom prompts: {}",
            settings.dictionary.len(),
            settings.snippets.len(),
            settings.app_styles.len(),
            [&o.light, &o.medium, &o.high].iter().filter(|p| !p.trim().is_empty()).count()
        );
        let _ = writeln!(
            r,
            "History retention: {} · Local API: {} (port {})",
            if settings.history_retention_days == 0 {
                "forever".to_string()
            } else {
                format!("{} days", settings.history_retention_days)
            },
            if settings.api_enabled { "on" } else { "off" },
            settings.api_port
        );
        Ok(r)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Pull the cleanup profile's model on its Ollama server (blocking, can take
/// minutes for a ~2 GB model).
#[tauri::command]
pub async fn pull_ollama_model(state: State<'_, AppState>) -> Result<(), String> {
    let profile = state.settings.lock().unwrap().cleanup_llm();
    if profile.api != crate::settings::CleanupApi::Ollama {
        return Err("pulling a model needs an Ollama profile".to_string());
    }
    let (url, model) = (profile.base_url, profile.model);
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

/// Models available on the cleanup profile's server. Errors when
/// unreachable — the frontend falls back to a free-text field.
#[tauri::command]
pub async fn list_ollama_models(state: State<'_, AppState>) -> Result<Vec<String>, String> {
    let profile = state.settings.lock().unwrap().cleanup_llm();
    llm_list_models(profile).await
}

/// Models on any profile's server, saved or still being edited — the
/// profile editor's "Test connection" (#119). Errors when unreachable.
#[tauri::command]
pub async fn llm_list_models(profile: crate::llm::LlmProfile) -> Result<Vec<String>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        crate::cleanup::ollama::list_models(&profile).map_err(|e| format!("{e:#}"))
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

// ---- Archive (0.7): notes, meetings and transcriptions as markdown files ----

use crate::archive::{self, Item, ItemMeta, ItemSummary, SearchFilters};
use std::path::{Path, PathBuf};

/// Archive folder and index path for the current settings, cloned out of the
/// state so the blocking work doesn't hold the settings lock.
fn archive_paths(state: &AppState) -> Result<(PathBuf, PathBuf), String> {
    let settings = state.settings.lock().unwrap();
    let dir = crate::state::resolve_archive_dir(&state.paths, &settings)
        .map_err(|e| format!("{e:#}"))?;
    Ok((dir, state.paths.archive_index.clone()))
}

/// Filesystem and SQLite work runs off the async runtime.
async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> anyhow::Result<T> + Send + 'static,
) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(move || f().map_err(|e| format!("{e:#}")))
        .await
        .map_err(|e| e.to_string())?
}

/// Keep the index in step after the app changed an item. The files are
/// already written, so an index failure is only logged: the next search
/// re-syncs (or rebuilds) from the folder anyway.
fn reindex(
    root: &Path,
    db: &Path,
    f: impl FnOnce(&mut archive::Index) -> anyhow::Result<()>,
) {
    let result = archive::Index::open(root, db).and_then(|mut idx| f(&mut idx));
    if let Err(e) = result {
        eprintln!("archive index: update failed ({e:#})");
    }
}

/// The resolved archive folder (not created — just the path).
#[tauri::command]
pub fn archive_dir(state: State<'_, AppState>) -> Result<String, String> {
    archive_paths(&state).map(|(dir, _)| dir.display().to_string())
}

/// Every item, newest first, by scanning the archive folder.
#[tauri::command]
pub async fn archive_list(state: State<'_, AppState>) -> Result<Vec<ItemSummary>, String> {
    let (dir, _) = archive_paths(&state)?;
    blocking(move || Ok(archive::list_items(&dir))).await
}

/// Full-text search with facets over the index (synced with the folder
/// first). Empty query = all items passing the filters, newest first.
#[tauri::command]
pub async fn archive_search(
    state: State<'_, AppState>,
    query: String,
    filters: Option<SearchFilters>,
) -> Result<Vec<ItemSummary>, String> {
    let (dir, db) = archive_paths(&state)?;
    let filters = filters.unwrap_or_default();
    blocking(move || archive::with_index(&dir, &db, |idx| idx.search(&query, &filters))).await
}

#[tauri::command]
pub async fn archive_get(state: State<'_, AppState>, id: String) -> Result<Item, String> {
    let (dir, _) = archive_paths(&state)?;
    blocking(move || archive::read_item(&dir, &id)).await
}

/// Replace an item's frontmatter; returns the updated item.
#[tauri::command]
pub async fn archive_update_meta(
    state: State<'_, AppState>,
    id: String,
    meta: ItemMeta,
) -> Result<Item, String> {
    let (dir, db) = archive_paths(&state)?;
    blocking(move || {
        let item = archive::update_meta(&dir, &id, &meta)?;
        reindex(&dir, &db, |idx| idx.index_item(&id));
        Ok(item)
    })
    .await
}

/// Line editor: replace the text of one segment; returns the updated item.
/// Refused when `transcript.md` was edited outside the app.
#[tauri::command]
pub async fn archive_update_segment(
    state: State<'_, AppState>,
    id: String,
    segment_id: u32,
    text: String,
) -> Result<Item, String> {
    edit_segment_command(&state, id, segment_id, archive::SegmentEdit::Text(text)).await
}

/// Line editor: delete one segment; returns the updated item. Refused when
/// `transcript.md` was edited outside the app.
#[tauri::command]
pub async fn archive_delete_segment(
    state: State<'_, AppState>,
    id: String,
    segment_id: u32,
) -> Result<Item, String> {
    edit_segment_command(&state, id, segment_id, archive::SegmentEdit::Delete).await
}

async fn edit_segment_command(
    state: &AppState,
    id: String,
    segment_id: u32,
    edit: archive::SegmentEdit,
) -> Result<Item, String> {
    let (dir, db) = archive_paths(state)?;
    let journal = crate::engine::session::journal_path(state);
    blocking(move || {
        // A live item can't be line-edited (#158): the store refuses the
        // `recording` marker, this a session that still owns the item.
        crate::engine::session::ensure_not_live(&journal, &dir, &id)?;
        let item = archive::edit_segment(&dir, &id, segment_id, edit)?;
        reindex(&dir, &db, |idx| idx.index_item(&id));
        Ok(item)
    })
    .await
}

/// Move an item folder to the OS trash (never a hard delete). Refused for
/// an item a capture session is writing (#158), in this process or another
/// instance.
#[tauri::command]
pub async fn archive_delete(state: State<'_, AppState>, id: String) -> Result<(), String> {
    let (dir, db) = archive_paths(&state)?;
    let journal = crate::engine::session::journal_path(&state);
    blocking(move || {
        crate::engine::session::ensure_not_live(&journal, &dir, &id)?;
        archive::delete_item(&dir, &id)?;
        reindex(&dir, &db, |idx| idx.remove_from_index(&id));
        Ok(())
    })
    .await
}

/// Open the item's folder in the OS file manager — or, without an id, the
/// archive folder itself (created if missing, so the empty Library can show
/// the user where items will go).
#[tauri::command]
pub fn archive_reveal(
    app: AppHandle,
    state: State<'_, AppState>,
    id: Option<String>,
) -> Result<(), String> {
    use tauri_plugin_opener::OpenerExt;
    let (dir, _) = archive_paths(&state)?;
    let target = match id {
        Some(id) => {
            let item = archive::paths::item_dir(&dir, &id).map_err(|e| format!("{e:#}"))?;
            if !item.is_dir() {
                return Err(format!("no archive item '{id}'"));
            }
            item
        }
        None => {
            std::fs::create_dir_all(&dir)
                .map_err(|e| format!("creating {}: {e}", dir.display()))?;
            dir
        }
    };
    app.opener()
        .open_path(target.display().to_string(), None::<&str>)
        .map_err(|e| e.to_string())
}

/// Drop and rebuild the search index from the folder; returns the item count.
#[tauri::command]
pub async fn archive_rebuild_index(state: State<'_, AppState>) -> Result<usize, String> {
    let (dir, db) = archive_paths(&state)?;
    blocking(move || archive::rebuild_index(&dir, &db)).await
}

// ---- Recipes (0.8, #120): prompts that write companion documents ----

use crate::recipes::{self, Recipe};

/// Every recipe: the built-ins first, then the user's own.
#[tauri::command]
pub fn recipes_list(state: State<'_, AppState>) -> Vec<Recipe> {
    recipes::all_recipes(&state.settings.lock().unwrap().recipes)
}

/// The companion documents next to an item's transcript (`document.md`
/// first).
#[tauri::command]
pub async fn recipe_documents(
    state: State<'_, AppState>,
    id: String,
) -> Result<Vec<archive::companion::CompanionDoc>, String> {
    let (dir, _) = archive_paths(&state)?;
    blocking(move || archive::companion::list_companions(&dir, &id)).await
}

/// `recipe-progress` payload.
#[derive(serde::Serialize, Clone)]
pub struct RecipeProgressEvent {
    pub item_id: String,
    pub recipe_id: String,
    pub recipe_name: String,
    #[serde(flatten)]
    pub progress: recipes::engine::Progress,
}

/// How a recipe run ended (also sent as `recipe-finished`).
#[derive(serde::Serialize, Clone, Debug)]
pub struct RecipeFinished {
    pub item_id: String,
    pub recipe_id: String,
    pub recipe_name: String,
    /// Companion document written, for a document recipe.
    pub file: Option<String>,
    /// The text, for an answer recipe.
    pub answer: Option<String>,
    pub error: Option<String>,
    pub cancelled: bool,
}

/// The profile a recipe runs on when the caller names none: the cleanup
/// profile when it is local, else the first local one, else the cleanup
/// profile (which the privacy check then refuses with its explanation).
fn default_recipe_profile(settings: &Settings) -> crate::llm::LlmProfile {
    let cleanup = settings.cleanup_llm();
    if !cleanup.external {
        return cleanup;
    }
    settings
        .llm_profiles
        .iter()
        .find(|p| !p.external)
        .cloned()
        .unwrap_or(cleanup)
}

/// Run a recipe on an archive item with an LLM profile (default: see
/// [`default_recipe_profile`]). Refusals — unknown recipe or profile, an
/// external profile (#122 adds the per-run confirmation), a live item,
/// another run on the same item — are errors, and nothing is sent. Once
/// started, progress arrives as `recipe-progress` and the end as
/// `recipe-finished`, whose payload this also returns (with `error` /
/// `cancelled` set when it did not complete).
#[tauri::command]
pub async fn recipe_run(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
    recipe_id: String,
    profile_id: Option<String>,
) -> Result<RecipeFinished, String> {
    use tauri::{Emitter, Manager};
    let (archive, db) = archive_paths(&state)?;
    let settings = state.settings.lock().unwrap().clone();
    let recipe = recipes::find_recipe(&settings.recipes, &recipe_id)
        .ok_or_else(|| format!("no recipe '{recipe_id}'"))?;
    let profile = match profile_id.as_deref() {
        Some(pid) => settings
            .llm_profiles
            .iter()
            .find(|p| p.id == pid)
            .cloned()
            .ok_or_else(|| format!("no LLM profile '{pid}'"))?,
        None => default_recipe_profile(&settings),
    };
    recipes::run::check_profile(&profile).map_err(|e| format!("{e:#}"))?;
    {
        let journal = crate::engine::session::journal_path(&state);
        let (archive, id) = (archive.clone(), id.clone());
        blocking(move || crate::engine::session::ensure_not_live(&journal, &archive, &id)).await?;
    }
    let cancel = state
        .recipe_runs
        .begin(&id, &recipe)
        .map_err(|e| format!("{e:#}"))?;

    let handle = app.clone();
    let (item_id, r) = (id.clone(), recipe.clone());
    let joined = tauri::async_runtime::spawn_blocking(move || {
        let state = handle.state::<AppState>();
        /// Unregisters the run however the closure ends (panic included).
        struct End<'a>(&'a recipes::run::Runs, &'a str);
        impl Drop for End<'_> {
            fn drop(&mut self) {
                self.0.end(self.1);
            }
        }
        let _end = End(&state.recipe_runs, &item_id);
        let now = chrono::Local::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, false);
        let model = recipes::run::ProfileModel {
            profile: profile.clone(),
        };
        let result = recipes::run::run_on_item(
            &archive,
            &item_id,
            &r,
            &profile,
            &model,
            &now,
            &cancel,
            &mut |p| {
                state.recipe_runs.set_progress(&item_id, p);
                let _ = handle.emit(
                    "recipe-progress",
                    RecipeProgressEvent {
                        item_id: item_id.clone(),
                        recipe_id: r.id.clone(),
                        recipe_name: r.name.clone(),
                        progress: p,
                    },
                );
            },
        );
        if result.is_ok() {
            // Keep the index in step with the item folder.
            reindex(&archive, &db, |idx| idx.index_item(&item_id));
        }
        result
    })
    .await;
    let mut finished = RecipeFinished {
        item_id: id,
        recipe_id: recipe.id.clone(),
        recipe_name: recipe.name.clone(),
        file: None,
        answer: None,
        error: None,
        cancelled: false,
    };
    match joined {
        Ok(Ok(out)) => {
            finished.file = out.file;
            finished.answer = out.answer;
        }
        Ok(Err(e)) if e.downcast_ref::<recipes::engine::Cancelled>().is_some() => {
            finished.cancelled = true;
        }
        Ok(Err(e)) => finished.error = Some(format!("{e:#}")),
        Err(e) => finished.error = Some(e.to_string()),
    }
    let _ = app.emit("recipe-finished", finished.clone());
    Ok(finished)
}

/// Stop the recipe running on an item after its current step. False when
/// none runs there.
#[tauri::command]
pub fn recipe_cancel(state: State<'_, AppState>, id: String) -> bool {
    state.recipe_runs.cancel(&id)
}

/// Recipe runs in flight (a UI mounted mid-run adopts them).
#[tauri::command]
pub fn recipe_status(state: State<'_, AppState>) -> Vec<recipes::run::RunStatus> {
    state.recipe_runs.list()
}

/// Show a companion document in the OS file manager.
#[tauri::command]
pub fn recipe_reveal_document(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
    file: String,
) -> Result<(), String> {
    use tauri_plugin_opener::OpenerExt;
    let (dir, _) = archive_paths(&state)?;
    let path =
        archive::companion::companion_path(&dir, &id, &file).map_err(|e| format!("{e:#}"))?;
    app.opener()
        .reveal_item_in_dir(path)
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod recipe_tests {
    use super::*;
    use crate::llm::LlmProfile;
    use crate::settings::CleanupApi;

    #[test]
    fn default_recipe_profile_prefers_a_local_one() {
        let work = LlmProfile::new("work", "Work", CleanupApi::Openai, "https://api.example.com", "", "m");
        let lan = LlmProfile::new("lan", "LAN", CleanupApi::Ollama, "http://localhost:11434", "", "m");
        let mut s = Settings {
            llm_profiles: vec![work.clone(), lan],
            cleanup_profile: "work".into(),
            ..Default::default()
        };
        assert_eq!(default_recipe_profile(&s).id, "lan");
        s.cleanup_profile = "lan".into();
        assert_eq!(default_recipe_profile(&s).id, "lan");
        // Only external profiles: the run is then refused with the reason.
        s.llm_profiles = vec![work];
        s.cleanup_profile = "work".into();
        assert_eq!(default_recipe_profile(&s).id, "work");
    }
}
