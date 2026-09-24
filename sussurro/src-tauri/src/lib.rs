pub mod api;
pub mod archive;
pub mod audio;
pub mod cleanup;
pub mod commands;
pub mod config_io;
pub mod engine;
pub mod history;
pub mod hotkey;
pub mod inject;
pub mod llm;
pub mod permissions;
pub mod pipeline;
pub mod recipes;
pub mod secrets;
pub mod settings;
pub mod snippets;
pub mod sources;
pub mod speakers;
pub mod state;
pub mod stats;
pub mod stt;
pub mod voice_commands;
pub mod tray;
#[cfg(all(target_os = "linux", feature = "wayland-portal"))]
pub mod wayland_portal;

use crate::audio::recorder::Recorder;
use crate::settings::Settings;
use crate::state::{AppPaths, AppState};
use std::sync::{Mutex, OnceLock};
use tauri::{AppHandle, Manager};
use tauri_plugin_global_shortcut::ShortcutState;

/// The running app handle, stashed at setup so free functions can hop onto the
/// main thread. On macOS, paste injection needs this: enigo's keycode lookup
/// calls Text Input Source APIs that abort if run off the main thread (#48).
/// `None` in unit tests, which never set it.
static APP_HANDLE: OnceLock<AppHandle> = OnceLock::new();

pub fn app_handle() -> Option<AppHandle> {
    APP_HANDLE.get().cloned()
}

/// Main window size for the classic single-column UI and for the workspace
/// preview (`Settings::ui_v2`): (width, height, min width, min height), in
/// logical pixels. The classic values match `tauri.conf.json`.
pub fn main_window_layout(workspace: bool) -> (f64, f64, f64, f64) {
    if workspace {
        (1120.0, 740.0, 800.0, 560.0)
    } else {
        (700.0, 860.0, 560.0, 640.0)
    }
}

/// Resize the main window for the chosen UI. Best effort: a failure only
/// leaves the window at its current size.
pub fn apply_main_window_layout(app: &AppHandle, workspace: bool) {
    let Some(w) = app.get_webview_window("main") else {
        return;
    };
    let (width, height, min_w, min_h) = main_window_layout(workspace);
    let _ = w.set_min_size(Some(tauri::LogicalSize::new(min_w, min_h)));
    let _ = w.set_size(tauri::LogicalSize::new(width, height));
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            Some(vec!["--autostart"]),
        ))
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|app, _shortcut, event| {
                    let pressed = event.state() == ShortcutState::Pressed;
                    pipeline::handle_trigger(app, pressed);
                })
                .build(),
        )
        .setup(|app| {
            let handle = app.handle();
            let _ = APP_HANDLE.set(handle.clone());
            let paths = AppPaths::from_app(handle);
            let (mut settings, migrated) = Settings::load_migrating(&paths.settings_file);
            // Profile API keys live in the OS credential store (#159): read
            // them, and move clear-text ones in (settings.json then keeps
            // only the reference). Only profiles with a key touch the store.
            let keys_moved = secrets::load_keys(&mut settings, &secrets::OsStore);
            // Pre-0.8 cleanup settings became the "Local" LLM profile (#119):
            // write the new shape once. Best effort — the in-memory settings
            // are already migrated, and the next load would migrate again.
            if migrated || keys_moved {
                if let Err(e) = settings.save(&paths.settings_file) {
                    eprintln!("could not save the migrated settings: {e}");
                }
            }
            // Wayland portal injection persists its consent token next to
            // the settings; without this the dialog would reappear per launch.
            #[cfg(all(target_os = "linux", feature = "wayland-portal"))]
            wayland_portal::init(paths.settings_file.with_file_name("portal-restore-token"));
            let _ = history::prune_older_than(&paths.history_file, settings.history_retention_days);
            // tauri.conf.json sizes the window for the classic UI.
            if settings.ui_v2 {
                apply_main_window_layout(handle, true);
            }
            // Neither a failed shortcut registration (e.g. GNOME Wayland
            // policy) nor a missing tray host (headless CI, minimal WMs) is
            // fatal: the window and the in-app Dictate button still work.
            if let Err(e) = hotkey::apply(handle, &settings.hotkey) {
                eprintln!("global shortcut unavailable: {e:#}");
            }
            if let Err(e) = tray::setup(handle) {
                eprintln!("tray unavailable: {e:#}");
            }
            app.manage(AppState {
                recorder: Mutex::new(Recorder::default()),
                transcriber: Mutex::new(None),
                transcriber_last_used: Mutex::new(None),
                settings: Mutex::new(settings),
                paths,
                mic_test: std::sync::atomic::AtomicBool::new(false),
                stream: Mutex::new(state::StreamState::default()),
                engine: Default::default(),
                dictation: Default::default(),
                recipe_runs: Default::default(),
                recipe_answers: Default::default(),
                consents: Default::default(),
            });
            // Long-form sessions the last run never finished (crash, forced
            // quit): keep their items as "interrupted" (#153). Off the main
            // thread; a session started meanwhile waits on the journal lock.
            {
                let handle = handle.clone();
                std::thread::spawn(move || engine::session::recover_after_crash(&handle));
            }
            {
                let s = app.state::<state::AppState>().settings.lock().unwrap().clone();
                if s.api_enabled {
                    api::spawn(handle.clone(), s.api_port);
                }
            }
            // Idle model unload: a loaded transcriber holds up to GBs of RAM
            // while the app lives in the tray — check once a minute.
            {
                let handle = handle.clone();
                std::thread::spawn(move || loop {
                    std::thread::sleep(std::time::Duration::from_secs(60));
                    pipeline::unload_transcriber_if_idle(&handle.state::<state::AppState>());
                });
            }
            // Launched at login: live in the tray, don't pop the window.
            if std::env::args().any(|a| a == "--autostart") {
                if let Some(w) = app.get_webview_window("main") {
                    let _ = w.hide();
                }
            }
            Ok(())
        })
        // Closing the window hides to tray; Quit lives in the tray menu.
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_settings,
            commands::set_settings,
            commands::extension_token_get,
            commands::extension_token_regenerate,
            commands::local_api_status,
            commands::get_history,
            commands::search_history,
            commands::clear_history,
            commands::usage_stats,
            commands::export_history,
            commands::model_is_downloaded,
            commands::list_whisper_models,
            commands::download_model,
            commands::list_ollama_models,
            commands::llm_list_models,
            commands::list_input_devices,
            commands::start_mic_test,
            commands::stop_mic_test,
            commands::mic_level,
            commands::trigger_dictation,
            commands::copy_text,
            commands::reclean,
            commands::learn_correction,
            commands::export_config,
            commands::import_config,
            commands::pick_import_file,
            commands::transcribe_file,
            commands::engine_start_mic,
            commands::engine_start_link,
            commands::link_inspect,
            commands::yt_dlp_status,
            commands::engine_stop_mic,
            commands::engine_cancel,
            commands::engine_status,
            commands::get_default_prompts,
            commands::ollama_status,
            commands::diagnostics,
            commands::credential_store_status,
            commands::pull_ollama_model,
            commands::translate_entry,
            commands::check_permissions,
            commands::open_settings,
            commands::archive_dir,
            commands::archive_list,
            commands::archive_search,
            commands::archive_get,
            commands::archive_update_meta,
            commands::archive_update_segment,
            commands::archive_delete_segment,
            commands::archive_move_segment_speaker,
            commands::archive_rename_speaker,
            commands::archive_redetect_speakers,
            commands::archive_link_speaker,
            commands::archive_unlink_speaker,
            commands::archive_voice_source,
            commands::archive_identify_voices,
            commands::archive_delete,
            commands::archive_reveal,
            commands::archive_rebuild_index,
            commands::people_list,
            commands::people_usage,
            commands::people_add,
            commands::people_update,
            commands::people_delete,
            commands::people_merge,
            commands::archive_export,
            commands::archive_subtitles_status,
            commands::archive_create_subtitles,
            commands::recipes_list,
            commands::recipe_documents,
            commands::recipe_run,
            commands::recipe_ask,
            commands::recipe_save_answer,
            commands::recipe_dismiss_answer,
            commands::recipe_cancel,
            commands::recipe_status,
            commands::recipe_reveal_document,
            commands::external_run_preview,
            commands::prepare_external_run
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(test)]
mod tests {
    use super::main_window_layout;

    /// The classic layout restored when the workspace preview is switched
    /// off must match the window declared in tauri.conf.json.
    #[test]
    fn classic_window_layout_matches_tauri_conf() {
        let conf: serde_json::Value =
            serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
        let main = &conf["app"]["windows"][0];
        let (w, h, min_w, min_h) = main_window_layout(false);
        assert_eq!(main["width"].as_f64(), Some(w));
        assert_eq!(main["height"].as_f64(), Some(h));
        assert_eq!(main["minWidth"].as_f64(), Some(min_w));
        assert_eq!(main["minHeight"].as_f64(), Some(min_h));
        let (ww, _, wmin, _) = main_window_layout(true);
        assert!(ww > w && wmin >= 800.0);
    }
}
