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
) -> Result<Settings, String> {
    // Valid LLM profiles and a cleanup selection that names one (#119).
    settings.normalize();
    // The extension token changes only through its own commands (#126): a
    // UI holding an older copy of the settings must not undo a regenerate.
    settings.extension_token = state.settings.lock().unwrap().extension_token.clone();
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
    // Profile API keys go to the OS credential store (#159) — written when
    // new or changed, deleted when cleared or when their profile is. Where
    // no store works, a key stays in settings.json (the editor warns).
    let prev = state.settings.lock().unwrap().clone();
    crate::secrets::sync_keys(&mut settings, &prev, &crate::secrets::OsStore);
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
    // The UI learns where each key ended up (keychain, or the file fallback).
    let saved = settings.clone();
    crate::pipeline::swap_settings(&state, settings);
    if let Some(on) = workspace {
        crate::apply_main_window_layout(&app, on);
    }
    Ok(saved)
}

/// Whether a profile API key saved now goes to the OS credential store
/// (#159); the profile editor warns when it would land in settings.json.
#[tauri::command]
pub async fn credential_store_status() -> Result<crate::secrets::StoreStatus, String> {
    // May block on D-Bus (Linux): off the main thread.
    tauri::async_runtime::spawn_blocking(|| crate::secrets::status(&crate::secrets::OsStore))
        .await
        .map_err(|e| e.to_string())
}

/// The browser extension's pairing token (#126, E6), created on first use.
/// Settings → Browser extension (#127) copies it into the pairing code.
#[tauri::command]
pub fn extension_token_get(state: State<'_, AppState>) -> Result<String, String> {
    current_or_new_token(&state.settings, &state.paths.settings_file)
}

/// Replace the extension token: a paired extension must be paired again.
/// The local API reads the token on every request, so the old one stops
/// working at once.
#[tauri::command]
pub fn extension_token_regenerate(state: State<'_, AppState>) -> Result<String, String> {
    replace_extension_token(&state.settings, &state.paths.settings_file)
}

/// The current token, or a fresh (saved) one when none exists yet.
fn current_or_new_token(
    settings: &std::sync::Mutex<Settings>,
    file: &std::path::Path,
) -> Result<String, String> {
    let current = settings.lock().unwrap().extension_token.clone();
    if !current.trim().is_empty() {
        return Ok(current);
    }
    replace_extension_token(settings, file)
}

/// A fresh token, saved; on a failed save the old one stays in effect.
fn replace_extension_token(
    settings: &std::sync::Mutex<Settings>,
    file: &std::path::Path,
) -> Result<String, String> {
    let mut settings = settings.lock().unwrap();
    let mut next = settings.clone();
    let token = next
        .regenerate_extension_token()
        .map_err(|e| e.to_string())?;
    next.save(file).map_err(|e| e.to_string())?;
    settings.extension_token = token.clone();
    Ok(token)
}

/// Whether the local API is listening, and on which port (#127). Its
/// settings apply at startup, so Settings → Browser extension compares
/// this with them to tell when a restart is needed.
#[tauri::command]
pub fn local_api_status() -> crate::api::ListenState {
    crate::api::listen_state()
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
///
/// The People registry (#132) holds other people's emails, so it is left
/// out unless `include_people` is explicitly true.
#[tauri::command]
pub fn export_config(
    state: State<'_, AppState>,
    path: String,
    include_people: Option<bool>,
) -> Result<(), String> {
    let settings = state.settings.lock().unwrap().clone();
    let persons = if include_people == Some(true) {
        let (dir, _) = archive_paths(&state)?;
        crate::archive::people::list_people(&dir).map_err(|e| format!("{e:#}"))?
    } else {
        Vec::new()
    };
    crate::config_io::export_to(std::path::Path::new(&path), &settings, &persons)
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
    let mut msg = format!("Imported {w} words, {sn} snippets, {st} app styles");
    // People travel only in a bundle exported with them (#132).
    if !bundle.people.is_empty() {
        let (dir, _) = archive_paths(&state)?;
        let added = crate::archive::people::modify(&dir, |ps| {
            Ok(crate::archive::people::import_people(ps, &bundle.people))
        })
        .map_err(|e| format!("{msg}, but People could not be imported: {e:#}"))?;
        msg.push_str(&format!(", {added} people"));
    }
    Ok(msg)
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

/// Transcribe a link (#123): a direct audio/video file, or a video
/// platform through yt-dlp found on PATH. Returns the session id at once;
/// `engine-download` events report the download, then the usual `engine-*`
/// events the transcription into a `transcription` item (`source:
/// url:<link>`). `allow_local`: this run may reach hosts on this computer
/// or the local network (refused by default). An invalid link, a missing
/// yt-dlp or an unwritable archive fail here, before any download.
#[tauri::command]
pub fn engine_start_link(
    app: AppHandle,
    url: String,
    title: Option<String>,
    allow_local: Option<bool>,
    language: Option<String>,
    cleanup_level: Option<crate::settings::CleanupLevel>,
) -> Result<u64, String> {
    crate::engine::session::start_link(
        &app,
        &url,
        title.unwrap_or_default(),
        allow_local.unwrap_or(false),
        crate::engine::session::RunOptions {
            language,
            cleanup_level,
        },
    )
    .map_err(|e| format!("{e:#}"))
}

/// What the Link tab shows while the user types (#123). Pure: no network,
/// no process.
#[derive(serde::Serialize)]
pub struct LinkInfo {
    /// `direct` | `platform`; absent when the link is invalid.
    pub kind: Option<crate::sources::url::LinkKind>,
    /// Why the link can't be used, if so.
    pub error: Option<String>,
    /// The host is visibly this computer or the local network (an IP
    /// literal or localhost): the run needs "Allow local network addresses".
    pub local: bool,
    /// Short form for display.
    pub label: String,
}

#[tauri::command]
pub fn link_inspect(url: String) -> LinkInfo {
    match crate::sources::url::parse_link(&url) {
        Ok(link) => LinkInfo {
            kind: Some(link.kind),
            error: None,
            local: crate::sources::url::is_visibly_local(&link.url),
            label: crate::sources::url::display_label(&link.url),
        },
        Err(e) => LinkInfo {
            kind: None,
            error: Some(format!("{e:#}")),
            local: false,
            label: String::new(),
        },
    }
}

/// Whether yt-dlp is installed (#123), for the Link tab.
#[derive(serde::Serialize)]
pub struct YtDlpStatus {
    pub found: bool,
    pub path: Option<String>,
    /// `yt-dlp --version`, when it answered.
    pub version: Option<String>,
    /// How to install it on this OS.
    pub install_help: String,
}

#[tauri::command]
pub async fn yt_dlp_status() -> Result<YtDlpStatus, String> {
    tauri::async_runtime::spawn_blocking(|| {
        use crate::sources::url::ytdlp;
        let path = ytdlp::find();
        let version = path
            .as_deref()
            .and_then(|p| ytdlp::version(p, std::time::Duration::from_secs(10)).ok());
        YtDlpStatus {
            found: path.is_some(),
            path: path.map(|p| p.to_string_lossy().into_owned()),
            version,
            install_help: ytdlp::install_instructions(),
        }
    })
    .await
    .map_err(|e| e.to_string())
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
    /// The browser meeting being recorded, if any (#126).
    pub meeting_session: Option<u64>,
    /// Running file transcriptions, oldest first (#158): a UI mounted
    /// mid-run (window reload, `ui_v2` switched) adopts them, so a file
    /// started from the other UI can still be followed and cancelled.
    pub file_sessions: Vec<FileSessionStatus>,
    /// Running link transcriptions, oldest first (#123), adopted likewise.
    pub link_sessions: Vec<FileSessionStatus>,
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
        meeting_session: state.engine.meeting_session(),
        file_sessions: state
            .engine
            .file_sessions()
            .into_iter()
            .map(|(session_id, label)| FileSessionStatus { session_id, label })
            .collect(),
        link_sessions: state
            .engine
            .link_sessions()
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
        // Never the key itself (#159): only where it is kept.
        let _ = writeln!(
            r,
            "Cleanup: {:?} · profile \"{}\" ({:?}, {}) · {} @ {} (running: {llm_running}, model present: {llm_has_model}) · API key: {} · {} profile(s)",
            settings.cleanup_level,
            profile.name,
            profile.api,
            if profile.external { "external" } else { "local" },
            profile.model,
            crate::secrets::redact_url(&profile.base_url),
            crate::secrets::key_summary(&profile),
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

/// Whether the subtitles setting is *Always* (P7, #133).
fn subtitles_always(state: &AppState) -> bool {
    state.settings.lock().unwrap().subtitles == crate::settings::SubtitlesMode::Always
}

/// With the *Always* subtitles setting, bring the item's `transcript.srt`
/// up to date after its transcript was saved. The save already succeeded:
/// a failure here is only logged.
fn refresh_subtitles_if(always: bool, root: &Path, id: &str) {
    if !always {
        return;
    }
    if let Err(e) = archive::export::refresh_subtitles(root, id) {
        eprintln!("archive: transcript.srt of {id} not updated ({e:#})");
    }
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

/// Replace an item's frontmatter; returns the updated item. Participants
/// are refused on notes (P10, see `archive::update_meta`); new participants
/// are linked to the People registry (`archive::people::link_on_save`).
#[tauri::command]
pub async fn archive_update_meta(
    state: State<'_, AppState>,
    id: String,
    meta: ItemMeta,
) -> Result<Item, String> {
    let (dir, db) = archive_paths(&state)?;
    let always = subtitles_always(&state);
    let mut meta = meta;
    blocking(move || {
        // New participants whose name matches the People registry get the
        // person's email (#132).
        archive::people::link_on_save(&dir, &id, &mut meta);
        let item = archive::update_meta(&dir, &id, &meta)?;
        reindex(&dir, &db, |idx| idx.index_item(&id));
        // Speaker labels and the item type shape the subtitles too.
        refresh_subtitles_if(always, &dir, &id);
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
    let always = subtitles_always(state);
    blocking(move || {
        // A live item can't be line-edited (#158): the store refuses the
        // `recording` marker, this a session that still owns the item.
        crate::engine::session::ensure_not_live(&journal, &dir, &id)?;
        let item = archive::edit_segment(&dir, &id, segment_id, edit)?;
        reindex(&dir, &db, |idx| idx.index_item(&id));
        refresh_subtitles_if(always, &dir, &id);
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

/// Export an item to a file the user picked in the save dialog: `.md`,
/// `.txt`, `.srt` or `.vtt` (#133; subtitles refused for notes, P10). The
/// format's extension is added when the path lacks it. Returns the path
/// written.
#[tauri::command]
pub async fn archive_export(
    state: State<'_, AppState>,
    id: String,
    format: archive::export::ExportFormat,
    path: String,
) -> Result<String, String> {
    let (dir, _) = archive_paths(&state)?;
    if path.trim().is_empty() {
        return Err("no file chosen".to_string());
    }
    blocking(move || {
        let written = archive::export::export_to_file(&dir, &id, format, Path::new(&path))?;
        Ok(written.display().to_string())
    })
    .await
}

/// Whether the item can have subtitles and where its `transcript.srt`
/// stands (missing, the app's, edited outside).
#[tauri::command]
pub async fn archive_subtitles_status(
    state: State<'_, AppState>,
    id: String,
) -> Result<archive::export::SubtitlesStatus, String> {
    let (dir, _) = archive_paths(&state)?;
    blocking(move || archive::export::subtitles_status(&dir, &id)).await
}

/// "Create .srt": write or update the item's `transcript.srt` (#133).
/// Refused for notes (P10), live items and a `transcript.srt` edited
/// outside the app.
#[tauri::command]
pub async fn archive_create_subtitles(
    state: State<'_, AppState>,
    id: String,
) -> Result<archive::export::SubtitlesStatus, String> {
    let (dir, _) = archive_paths(&state)?;
    let journal = crate::engine::session::journal_path(&state);
    blocking(move || {
        crate::engine::session::ensure_not_live(&journal, &dir, &id)?;
        archive::export::create_subtitles(&dir, &id)
    })
    .await
}

// ---- People registry (0.9, #132): names, emails and aliases ----
//
// The registry holds other people's emails: nothing here logs it, and it
// never goes into diagnostics.

use crate::archive::people::{self, Person};

/// Every person, sorted by name. An error when `people.json` can't be read
/// (the screen says so instead of showing an empty list).
#[tauri::command]
pub async fn people_list(state: State<'_, AppState>) -> Result<Vec<Person>, String> {
    let (dir, _) = archive_paths(&state)?;
    blocking(move || people::list_people(&dir)).await
}

/// "Appears in N items" per person id, from the search index (synced with
/// the folder first).
#[tauri::command]
pub async fn people_usage(
    state: State<'_, AppState>,
) -> Result<std::collections::HashMap<String, usize>, String> {
    let (dir, db) = archive_paths(&state)?;
    blocking(move || {
        let persons = people::read_people(&dir);
        let rows = archive::with_index(&dir, &db, |idx| idx.participant_rows())?;
        Ok(people::usage(&persons, &rows))
    })
    .await
}

/// Add a person (the id is assigned); returns it as stored. Refused when
/// the name or the email is already in the registry.
#[tauri::command]
pub async fn people_add(state: State<'_, AppState>, person: Person) -> Result<Person, String> {
    let (dir, _) = archive_paths(&state)?;
    blocking(move || people::modify(&dir, |ps| people::add_person(ps, &person))).await
}

/// Replace a person (matched by id); returns it as stored. Existing items
/// are not changed.
#[tauri::command]
pub async fn people_update(state: State<'_, AppState>, person: Person) -> Result<Person, String> {
    let (dir, _) = archive_paths(&state)?;
    blocking(move || people::modify(&dir, |ps| people::update_person(ps, &person))).await
}

/// Remove a person. Existing items keep their participants and emails.
#[tauri::command]
pub async fn people_delete(state: State<'_, AppState>, id: String) -> Result<(), String> {
    let (dir, _) = archive_paths(&state)?;
    blocking(move || people::modify(&dir, |ps| people::delete_person(ps, &id))).await
}

/// Merge duplicates `from` into `into`; returns the merged person.
#[tauri::command]
pub async fn people_merge(
    state: State<'_, AppState>,
    into: String,
    from: Vec<String>,
) -> Result<Person, String> {
    let (dir, _) = archive_paths(&state)?;
    blocking(move || people::modify(&dir, |ps| people::merge_people(ps, &into, &from))).await
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
    /// The question, for a free question (#121).
    pub question: Option<String>,
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
    /// The text, for an answer recipe or a free question.
    pub answer: Option<String>,
    /// Handle of the answer for `recipe_save_answer` (kept in memory only).
    pub answer_id: Option<u64>,
    /// The question, for a free question (#121).
    pub question: Option<String>,
    /// Profile the run used (name as shown), its model and external flag.
    pub profile: String,
    pub model: String,
    pub external: bool,
    /// Server the transcript went to, for an external profile (#122).
    pub host: String,
    pub error: Option<String>,
    pub cancelled: bool,
}

/// The profile a recipe runs on when the caller names none: the cleanup
/// profile when it is local, else the first local one. Never an external
/// profile (#122): there is no silent fallback from local to external —
/// an external run is always one the user picked and confirmed.
fn default_recipe_profile(settings: &Settings) -> Option<crate::llm::LlmProfile> {
    let cleanup = settings.cleanup_llm();
    if !cleanup.external {
        return Some(cleanup);
    }
    settings.llm_profiles.iter().find(|p| !p.external).cloned()
}

/// The profile `profile_id` names, or the default (local) one.
fn recipe_profile(settings: &Settings, profile_id: Option<&str>) -> Result<crate::llm::LlmProfile, String> {
    match profile_id {
        Some(pid) => settings
            .llm_profiles
            .iter()
            .find(|p| p.id == pid)
            .cloned()
            .ok_or_else(|| format!("no LLM profile '{pid}'")),
        None => default_recipe_profile(settings).ok_or_else(|| {
            "no local LLM profile — pick a profile for this run (an external one asks for a confirmation first)"
                .to_string()
        }),
    }
}

/// The recipe a run asks for: a free question (normalized) when `question`
/// is given, else the recipe `recipe_id`.
fn run_recipe_of(
    settings: &Settings,
    recipe_id: Option<&str>,
    question: Option<&str>,
) -> Result<(Recipe, Option<String>), String> {
    match question {
        Some(q) => {
            let q = recipes::answer::normalize_question(q).map_err(|e| format!("{e:#}"))?;
            let r = recipes::answer::question_recipe(&q).map_err(|e| format!("{e:#}"))?;
            Ok((r, Some(q)))
        }
        None => {
            let rid = recipe_id.ok_or("name a recipe or a question")?;
            let r = recipes::find_recipe(&settings.recipes, rid).ok_or_else(|| format!("no recipe '{rid}'"))?;
            Ok((r, None))
        }
    }
}

/// What a run on an external profile would send, and where (#122): the
/// confirmation dialog shows it. Reads the item, sends nothing, issues no
/// confirmation.
#[tauri::command]
pub async fn external_run_preview(
    state: State<'_, AppState>,
    id: String,
    recipe_id: Option<String>,
    question: Option<String>,
    profile_id: String,
) -> Result<recipes::run::ExternalRunPreview, String> {
    let settings = state.settings.lock().unwrap().clone();
    let (recipe, question) = run_recipe_of(&settings, recipe_id.as_deref(), question.as_deref())?;
    let profile = recipe_profile(&settings, Some(&profile_id))?;
    let (archive, _) = archive_paths(&state)?;
    blocking(move || recipes::run::preview(&archive, &id, &recipe, question.as_deref(), &profile)).await
}

/// `prepare_external_run` result.
#[derive(serde::Serialize, Clone, Debug)]
pub struct ExternalRunConsent {
    /// One-time token for `recipe_run` / `recipe_ask` (`consent`).
    pub token: String,
    pub expires_in_secs: u64,
}

/// The user confirmed a run on an external profile in the dialog (#122):
/// issue the one-time token that run needs. It is bound to this item, this
/// recipe or question, and the profile's current server and model; it
/// works once, within [`crate::llm::consent::CONSENT_TTL`].
#[tauri::command]
pub fn prepare_external_run(
    state: State<'_, AppState>,
    id: String,
    recipe_id: Option<String>,
    question: Option<String>,
    profile_id: String,
) -> Result<ExternalRunConsent, String> {
    let settings = state.settings.lock().unwrap().clone();
    let (recipe, _) = run_recipe_of(&settings, recipe_id.as_deref(), question.as_deref())?;
    let profile = recipe_profile(&settings, Some(&profile_id))?;
    if !profile.external {
        return Err(format!("“{}” is a local profile: its runs need no confirmation", profile.name));
    }
    archive::paths::validate_item_id(&id).map_err(|e| format!("{e:#}"))?;
    let target = crate::llm::consent::RunTarget::new(&id, &recipe, &profile);
    Ok(ExternalRunConsent {
        token: state.consents.issue(target),
        expires_in_secs: crate::llm::consent::CONSENT_TTL.as_secs(),
    })
}

/// Run a recipe on an archive item with an LLM profile (default: see
/// [`default_recipe_profile`]). An external profile needs `consent`, the
/// token `prepare_external_run` issued after the user confirmed (#122).
/// Refusals — unknown recipe or profile, an external profile without a
/// valid confirmation, a live item, another run on the same item — are
/// errors, and nothing is sent. Once started, progress arrives as
/// `recipe-progress` and the end as `recipe-finished`, whose payload this
/// also returns (with `error` / `cancelled` set when it did not complete).
/// An answer recipe's result is kept in memory for `recipe_save_answer`
/// (`answer_id`).
#[tauri::command]
pub async fn recipe_run(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
    recipe_id: String,
    profile_id: Option<String>,
    consent: Option<String>,
) -> Result<RecipeFinished, String> {
    let settings = state.settings.lock().unwrap().clone();
    let (recipe, _) = run_recipe_of(&settings, Some(&recipe_id), None)?;
    let profile = recipe_profile(&settings, profile_id.as_deref())?;
    run_recipe(app, &state, id, recipe, None, profile, consent).await
}

/// Ask a free question about an archive item (the Ask panel, #121): a
/// transient answer recipe whose task is the question, run like any other
/// recipe (map-reduce on long transcripts, same refusals and confirmation,
/// same events). The answer is not written anywhere unless saved with
/// `recipe_save_answer`.
#[tauri::command]
pub async fn recipe_ask(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
    question: String,
    profile_id: Option<String>,
    consent: Option<String>,
) -> Result<RecipeFinished, String> {
    let settings = state.settings.lock().unwrap().clone();
    let (recipe, question) = run_recipe_of(&settings, None, Some(&question))?;
    let profile = recipe_profile(&settings, profile_id.as_deref())?;
    run_recipe(app, &state, id, recipe, question, profile, consent).await
}

/// The confirmation a run needs (#122): none on a local profile; on an
/// external one the token is consumed (single use, whatever happens next)
/// and must have been issued for exactly this run.
fn consent_for(
    consents: &crate::llm::consent::ConsentStore,
    id: &str,
    recipe: &Recipe,
    profile: &crate::llm::LlmProfile,
    token: Option<&str>,
) -> Result<Option<crate::llm::consent::ConsentGrant>, String> {
    use crate::llm::consent::{authorize, RunTarget};
    let target = RunTarget::new(id, recipe, profile);
    let grant = match (profile.external, token) {
        (true, Some(t)) => Some(consents.consume(t, &target).map_err(|e| format!("{e:#}"))?),
        _ => None,
    };
    authorize(profile, &target, grant.as_ref()).map_err(|e| format!("{e:#}"))?;
    Ok(grant)
}

async fn run_recipe(
    app: AppHandle,
    state: &State<'_, AppState>,
    id: String,
    recipe: Recipe,
    question: Option<String>,
    profile: crate::llm::LlmProfile,
    consent: Option<String>,
) -> Result<RecipeFinished, String> {
    use tauri::{Emitter, Manager};
    let (archive, db) = archive_paths(state)?;
    recipes::run::check_profile(&profile).map_err(|e| format!("{e:#}"))?;
    let grant = consent_for(&state.consents, &id, &recipe, &profile, consent.as_deref())?;
    {
        let journal = crate::engine::session::journal_path(state);
        let (archive, id) = (archive.clone(), id.clone());
        blocking(move || crate::engine::session::ensure_not_live(&journal, &archive, &id)).await?;
    }
    let cancel = state
        .recipe_runs
        .begin_with(&id, &recipe, question.as_deref())
        .map_err(|e| format!("{e:#}"))?;

    let handle = app.clone();
    let (item_id, r, q, p) = (id.clone(), recipe.clone(), question.clone(), profile.clone());
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
        let model = recipes::run::ProfileModel { profile: p.clone() };
        let result = recipes::run::run_on_item(
            &archive,
            &item_id,
            &r,
            &p,
            grant.as_ref(),
            &model,
            &now,
            &cancel,
            &mut |step| {
                state.recipe_runs.set_progress(&item_id, step);
                let _ = handle.emit(
                    "recipe-progress",
                    RecipeProgressEvent {
                        item_id: item_id.clone(),
                        recipe_id: r.id.clone(),
                        recipe_name: r.name.clone(),
                        question: q.clone(),
                        progress: step,
                    },
                );
            },
        );
        if result.as_ref().is_ok_and(|o| o.file.is_some()) {
            // Keep the index in step with the item folder.
            reindex(&archive, &db, |idx| idx.index_item(&item_id));
        }
        result.map(|out| (out, now))
    })
    .await;
    let mut finished = RecipeFinished {
        item_id: id.clone(),
        recipe_id: recipe.id.clone(),
        recipe_name: recipe.name.clone(),
        file: None,
        answer: None,
        answer_id: None,
        question: question.clone(),
        profile: profile.name.trim().to_string(),
        model: profile.model.trim().to_string(),
        external: profile.external,
        host: if profile.external { profile.host() } else { String::new() },
        error: None,
        cancelled: false,
    };
    match joined {
        Ok(Ok((out, now))) => {
            finished.file = out.file;
            if let Some(text) = out.answer {
                finished.answer_id = Some(state.recipe_answers.put(recipes::answer::PendingAnswer {
                    item_id: id,
                    recipe_id: recipe.id.clone(),
                    recipe_name: recipe.name.clone(),
                    question,
                    profile: finished.profile.clone(),
                    model: finished.model.clone(),
                    external: profile.external,
                    host: finished.host.clone(),
                    date: now,
                    text: text.clone(),
                }));
                finished.answer = Some(text);
            }
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

/// Save an answer shown in the Ask panel as a companion document next to
/// the transcript (`<slug of the question>.md`, or of the recipe name),
/// with provenance frontmatter. Never replaces an existing file. The
/// answer is forgotten once saved. Returns the file name written.
#[tauri::command]
pub async fn recipe_save_answer(
    state: State<'_, AppState>,
    id: String,
    answer_id: u64,
) -> Result<String, String> {
    let (archive, db) = archive_paths(&state)?;
    let answer = state
        .recipe_answers
        .get(&id, answer_id)
        .ok_or("this answer is no longer available — ask again to save it")?;
    let item_id = id.clone();
    let file = blocking(move || {
        let file = recipes::answer::save_answer(&archive, &answer)?;
        reindex(&archive, &db, |idx| idx.index_item(&item_id));
        Ok(file)
    })
    .await?;
    state.recipe_answers.remove(&id, answer_id);
    Ok(file)
}

/// Forget an answer the Ask panel no longer shows (dismissed, replaced).
#[tauri::command]
pub fn recipe_dismiss_answer(state: State<'_, AppState>, id: String, answer_id: u64) -> bool {
    state.recipe_answers.remove(&id, answer_id)
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
        assert_eq!(default_recipe_profile(&s).unwrap().id, "lan");
        s.cleanup_profile = "lan".into();
        assert_eq!(default_recipe_profile(&s).unwrap().id, "lan");
        assert_eq!(recipe_profile(&s, None).unwrap().id, "lan");
        assert_eq!(recipe_profile(&s, Some("work")).unwrap().id, "work", "an explicit choice is kept");
        assert!(recipe_profile(&s, Some("nope")).is_err());
    }

    /// #122: with no profile named, a run never falls back from local to
    /// external — not even when every profile is external.
    #[test]
    fn local_to_external_fallback_never_happens() {
        let work = LlmProfile::new("work", "Work", CleanupApi::Openai, "https://api.example.com", "", "m");
        let mut lan_by_hand = LlmProfile::new("lan", "LAN", CleanupApi::Ollama, "http://localhost:11434", "", "m");
        lan_by_hand.external = true;
        let s = Settings {
            llm_profiles: vec![work, lan_by_hand],
            cleanup_profile: "work".into(),
            ..Default::default()
        };
        assert!(default_recipe_profile(&s).is_none());
        let err = recipe_profile(&s, None).unwrap_err();
        assert!(err.contains("no local LLM profile"), "{err}");
    }

    #[test]
    fn runs_resolve_a_recipe_or_a_question() {
        let s = Settings::default();
        let (r, q) = run_recipe_of(&s, Some("summary"), None).unwrap();
        assert_eq!((r.id.as_str(), q), ("summary", None));
        let (r, q) = run_recipe_of(&s, Some("summary"), Some("  Who\nsends it? ")).unwrap();
        assert_eq!((r.id.as_str(), q.as_deref()), ("question", Some("Who sends it?")));
        assert!(run_recipe_of(&s, Some("nope"), None).is_err());
        assert!(run_recipe_of(&s, None, None).is_err());
        assert!(run_recipe_of(&s, None, Some(" ")).is_err());
    }

    /// The command-side gate: no token → refused; a token is consumed once;
    /// a local profile needs none.
    #[test]
    fn consent_for_refuses_without_a_token_and_consumes_it_once() {
        use crate::llm::consent::{ConsentStore, RunTarget};
        let store = ConsentStore::default();
        let work = LlmProfile::new("work", "Work", CleanupApi::Openai, "https://api.example.com", "", "m");
        let recipe = recipes::builtin_recipes().remove(1);
        let err = consent_for(&store, "2026/09/a", &recipe, &work, None).unwrap_err();
        assert!(err.contains("Confirm the run first"), "{err}");
        let token = store.issue(RunTarget::new("2026/09/a", &recipe, &work));
        assert!(consent_for(&store, "2026/09/a", &recipe, &work, Some(&token)).unwrap().is_some());
        assert!(consent_for(&store, "2026/09/a", &recipe, &work, Some(&token)).is_err(), "single use");
        let token = store.issue(RunTarget::new("2026/09/a", &recipe, &work));
        assert!(consent_for(&store, "2026/09/b", &recipe, &work, Some(&token)).is_err(), "bound to the item");
        let local = LlmProfile::default();
        assert!(consent_for(&store, "2026/09/a", &recipe, &local, None).unwrap().is_none());
    }
}

#[cfg(test)]
mod extension_token_tests {
    use super::*;
    use std::sync::Mutex;

    fn saved_token(file: &std::path::Path) -> String {
        Settings::load(file).extension_token
    }

    #[test]
    fn the_token_is_created_once_and_saved() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("settings.json");
        let settings = Mutex::new(Settings::default());
        let first = current_or_new_token(&settings, &file).unwrap();
        assert_eq!(first.len(), 64);
        assert_eq!(saved_token(&file), first);
        assert_eq!(current_or_new_token(&settings, &file).unwrap(), first, "stable once created");
    }

    #[test]
    fn regenerating_replaces_the_old_token_in_memory_and_on_disk() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("settings.json");
        let settings = Mutex::new(Settings::default());
        let old = current_or_new_token(&settings, &file).unwrap();
        let new = replace_extension_token(&settings, &file).unwrap();
        assert_ne!(old, new);
        assert_eq!(settings.lock().unwrap().extension_token, new);
        assert_eq!(saved_token(&file), new);
        assert_eq!(current_or_new_token(&settings, &file).unwrap(), new);
    }

    #[test]
    fn a_failed_save_keeps_the_old_token() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("settings.json");
        let settings = Mutex::new(Settings::default());
        let old = current_or_new_token(&settings, &file).unwrap();
        // A directory where the file should be: the save fails.
        let blocked = dir.path().join("blocked");
        std::fs::create_dir_all(blocked.join("settings.json")).unwrap();
        assert!(replace_extension_token(&settings, &blocked.join("settings.json")).is_err());
        assert_eq!(settings.lock().unwrap().extension_token, old);
    }
}
