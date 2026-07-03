pub mod audio;
pub mod cleanup;
pub mod commands;
pub mod config_io;
pub mod history;
pub mod hotkey;
pub mod inject;
pub mod pipeline;
pub mod settings;
pub mod snippets;
pub mod state;
pub mod stt;
pub mod tray;

use crate::audio::recorder::Recorder;
use crate::settings::Settings;
use crate::state::{AppPaths, AppState};
use std::sync::Mutex;
use tauri::Manager;
use tauri_plugin_global_shortcut::ShortcutState;

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
                .with_handler(|app, shortcut, event| {
                    let pressed = event.state() == ShortcutState::Pressed;
                    let is_command = {
                        let state = app.state::<state::AppState>();
                        let cmd = state.settings.lock().unwrap().command_hotkey.clone();
                        cmd.parse::<tauri_plugin_global_shortcut::Shortcut>()
                            .map(|c| c == *shortcut)
                            .unwrap_or(false)
                    };
                    pipeline::handle_trigger(app, pressed, is_command);
                })
                .build(),
        )
        .setup(|app| {
            let handle = app.handle();
            let paths = AppPaths::from_app(handle);
            let settings = Settings::load(&paths.settings_file);
            // Neither a failed shortcut registration (e.g. GNOME Wayland
            // policy) nor a missing tray host (headless CI, minimal WMs) is
            // fatal: the window and the in-app Dictate button still work.
            if let Err(e) = hotkey::apply(handle, &settings.hotkey, &settings.command_hotkey) {
                eprintln!("global shortcut unavailable: {e:#}");
            }
            if let Err(e) = tray::setup(handle) {
                eprintln!("tray unavailable: {e:#}");
            }
            app.manage(AppState {
                recorder: Mutex::new(Recorder::default()),
                transcriber: Mutex::new(None),
                settings: Mutex::new(settings),
                paths,
                command_mode: std::sync::atomic::AtomicBool::new(false),
                stream: Mutex::new(state::StreamState::default()),
            });
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
            commands::get_history,
            commands::clear_history,
            commands::model_is_downloaded,
            commands::download_model,
            commands::list_ollama_models,
            commands::trigger_dictation,
            commands::copy_text,
            commands::reclean,
            commands::learn_correction,
            commands::export_config,
            commands::import_config
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
