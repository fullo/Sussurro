pub mod api;
pub mod archive;
pub mod audio;
pub mod calendar;
pub mod cleanup;
pub mod commands;
pub mod config_io;
pub mod diagnostics;
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
pub mod tts;
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
        // Saved audio for the Audio tab's player (#142): range-served WAVs
        // (Opus decoded to WAV, #248) of archive items only (see
        // archive::playback for the confinement).
        .register_asynchronous_uri_scheme_protocol(
            archive::playback::SCHEME,
            |ctx, request, responder| {
                let app = ctx.app_handle().clone();
                let webview = ctx.webview_label().to_string();
                tauri::async_runtime::spawn_blocking(move || {
                    responder.respond(commands::serve_audio(&app, &webview, &request));
                });
            },
        )
        .setup(|app| {
            let handle = app.handle();
            let _ = APP_HANDLE.set(handle.clone());
            // Tauri drops its resource table on every way out (exit event,
            // restart, the updater's Windows install): no sidecar outlives
            // the app (#117).
            app.resources_table().add(stt::remote::ExitGuard);
            // whisper.cpp's log keeps going to stderr; its backend lines are
            // kept for Settings → Diagnostics (#101).
            diagnostics::backend::install_whisper_log_capture();
            let paths = AppPaths::from_app(handle);
            let (mut settings, migrated) = Settings::load_migrating(&paths.settings_file);
            // Profile API keys live in the OS credential store (#159): read
            // them, and move clear-text ones in (settings.json then keeps
            // only the reference). Only profiles with a key touch the store.
            let keys_moved = secrets::load_keys(&mut settings, &secrets::OsStore);
            // Builds with the llama-server sidecar list the built-in
            // "Local (bundled)" LLM profile (#118) — added, never selected.
            let bundled_added =
                stt::sidecar::sidecar_available(handle) && settings.ensure_bundled_profile();
            // Pre-0.8 cleanup settings became the "Local" LLM profile (#119):
            // write the new shape once. Best effort — the in-memory settings
            // are already migrated, and the next load would migrate again.
            if migrated || keys_moved || bundled_added {
                if let Err(e) = settings.save(&paths.settings_file) {
                    eprintln!("could not save the migrated settings: {e}");
                }
            }
            // Wayland portal injection persists its consent token next to
            // the settings; without this the dialog would reappear per launch.
            #[cfg(all(target_os = "linux", feature = "wayland-portal"))]
            wayland_portal::init(paths.settings_file.with_file_name("portal-restore-token"));
            let _ = history::prune_older_than(&paths.history_file, settings.history_retention_days);
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
            // while the app lives in the tray — check once a minute. The
            // bundled LLM's server (#118) stops on its own idle clock.
            {
                let handle = handle.clone();
                std::thread::spawn(move || loop {
                    std::thread::sleep(std::time::Duration::from_secs(60));
                    pipeline::unload_transcriber_if_idle(&handle.state::<state::AppState>());
                    if llm::bundled::global().stop_if_idle(llm::bundled::IDLE_STOP) {
                        eprintln!("bundled LLM stopped after 15 min idle");
                    }
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
            commands::archive_tokens_list,
            commands::archive_token_create,
            commands::archive_token_revoke,
            commands::get_history,
            commands::search_history,
            commands::clear_history,
            commands::usage_stats,
            commands::export_history,
            commands::model_is_downloaded,
            commands::list_whisper_models,
            commands::stt_sidecar_available,
            commands::bundled_llm_status,
            commands::bundled_llm_download,
            commands::bundled_llm_use,
            commands::download_model,
            commands::list_ollama_models,
            commands::llm_list_models,
            commands::list_input_devices,
            commands::start_mic_test,
            commands::stop_mic_test,
            commands::mic_level,
            commands::whisper_gpu,
            commands::trigger_dictation,
            commands::copy_text,
            commands::reclean,
            commands::learn_correction,
            commands::export_config,
            commands::import_config,
            commands::pick_import_file,
            commands::save_list_export,
            commands::transcribe_file,
            commands::engine_start_mic,
            commands::engine_start_link,
            commands::link_inspect,
            commands::yt_dlp_status,
            commands::engine_stop_mic,
            commands::engine_start_system,
            commands::engine_stop_system,
            commands::list_system_audio_devices,
            commands::engine_cancel,
            commands::engine_status,
            commands::get_default_prompts,
            commands::ollama_status,
            commands::diagnostics,
            commands::diagnostics_snapshot,
            commands::credential_store_status,
            commands::calendar_link_status,
            commands::calendar_link_save,
            commands::calendar_link_remove,
            commands::calendar_events_from_file,
            commands::calendar_events_from_link,
            commands::calendar_add_attendees,
            commands::pull_ollama_model,
            commands::translate_entry,
            commands::check_permissions,
            commands::open_settings,
            commands::archive_dir,
            commands::archive_prepare,
            commands::archive_list,
            commands::archive_search,
            commands::archive_facets,
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
            commands::archive_voice_map,
            commands::archive_identify_voices,
            commands::archive_delete,
            commands::archive_delete_audio,
            commands::archive_compress_audio,
            commands::archive_compress_cancel,
            commands::archive_uncompressed_audio,
            commands::archive_reveal,
            commands::archive_rebuild_index,
            commands::people_list,
            commands::people_usage,
            commands::people_add,
            commands::people_update,
            commands::people_delete,
            commands::people_merge,
            commands::voices_status,
            commands::voice_status,
            commands::voice_set_enabled,
            commands::voices_rebuild,
            commands::voice_forget,
            commands::voices_forget_all,
            commands::own_voice_status,
            commands::own_voice_enrol_start,
            commands::own_voice_enrol_progress,
            commands::own_voice_enrol_cancel,
            commands::own_voice_enrol_finish,
            commands::own_voice_set_label,
            commands::own_voice_forget,
            commands::own_voice_find,
            commands::voice_suggestions,
            commands::voice_suggestion_dismiss,
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
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|_app, event| {
            if let tauri::RunEvent::Exit = event {
                // Also covered by the ExitGuard; explicit here so the
                // sidecar is gone before anything else shuts down.
                stt::remote::kill_all();
            }
        });
}

#[cfg(test)]
mod tests {
    /// #115: the workspace is the only UI, so tauri.conf.json opens the main
    /// window at its size (the rail plus a two-pane Library need ~800 px).
    #[test]
    fn main_window_is_sized_for_the_workspace() {
        let conf: serde_json::Value =
            serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
        let main = &conf["app"]["windows"][0];
        assert!(main["label"].is_null() || main["label"] == "main");
        assert_eq!(main["width"].as_f64(), Some(1120.0));
        assert_eq!(main["height"].as_f64(), Some(740.0));
        assert_eq!(main["minWidth"].as_f64(), Some(800.0));
        assert_eq!(main["minHeight"].as_f64(), Some(560.0));
    }
}
