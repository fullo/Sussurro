use crate::cleanup::ollama;
use crate::history::{self, HistoryEntry};
use crate::inject;
use crate::state::AppState;
use crate::stt::whisper::Transcriber;
use crate::stt::{dictionary_prompt, models};
use tauri::{AppHandle, Emitter, Manager};

#[derive(Debug, PartialEq)]
pub enum TriggerAction {
    Start,
    Finish,
    Ignore,
}

pub fn trigger_action(push_to_talk: bool, pressed: bool, recording: bool) -> TriggerAction {
    match (push_to_talk, pressed, recording) {
        (true, true, false) => TriggerAction::Start,
        (true, false, true) => TriggerAction::Finish,
        (false, true, false) => TriggerAction::Start,
        (false, true, true) => TriggerAction::Finish,
        _ => TriggerAction::Ignore,
    }
}

/// Emit pipeline status to the frontend: "idle" | "recording" | "processing" | "error: ...".
fn set_status(app: &AppHandle, status: &str) {
    update_overlay(app, status.split(':').next().unwrap_or("idle"));
    let _ = app.emit("pipeline-status", status.to_string());
}

/// The floating pill near the bottom of the screen: visible while recording
/// or processing, hidden otherwise. Never takes focus (focusable: false).
fn update_overlay(app: &AppHandle, state: &str) {
    let Some(w) = app.get_webview_window("overlay") else {
        return;
    };
    match state {
        "recording" | "processing" => {
            position_overlay(&w);
            let _ = w.show();
        }
        _ => {
            let _ = w.hide();
        }
    }
}

fn position_overlay(w: &tauri::WebviewWindow) {
    if let Ok(Some(monitor)) = w.current_monitor() {
        let screen = monitor.size();
        let size = w
            .outer_size()
            .unwrap_or_else(|_| tauri::PhysicalSize::new(210, 52));
        let x = screen.width.saturating_sub(size.width) / 2;
        let y = screen.height.saturating_sub(size.height + 96);
        let _ = w.set_position(tauri::PhysicalPosition::new(x as i32, y as i32));
    }
}

/// Called from the global-shortcut handler. Must return fast — heavy work is spawned.
pub fn handle_trigger(app: &AppHandle, pressed: bool) {
    let state = app.state::<AppState>();
    let push_to_talk = state.settings.lock().unwrap().push_to_talk;
    let recording = state.recorder.lock().unwrap().is_recording();

    match trigger_action(push_to_talk, pressed, recording) {
        TriggerAction::Ignore => {}
        TriggerAction::Start => {
            if let Err(e) = state.recorder.lock().unwrap().start() {
                set_status(app, &format!("error: {e}"));
                return;
            }
            if state.settings.lock().unwrap().sound_feedback {
                crate::audio::beep::record_start();
            }
            set_status(app, "recording");
        }
        TriggerAction::Finish => {
            if state.settings.lock().unwrap().sound_feedback {
                crate::audio::beep::record_stop();
            }
            set_status(app, "processing");
            let app = app.clone();
            // whisper + ollama take seconds — never block the event thread.
            std::thread::spawn(move || {
                let result = process_recording(&app);
                match result {
                    Ok(()) => set_status(&app, "idle"),
                    Err(e) => set_status(&app, &format!("error: {e:#}")),
                }
            });
        }
    }
}

fn process_recording(app: &AppHandle) -> anyhow::Result<()> {
    let state = app.state::<AppState>();

    let samples = state.recorder.lock().unwrap().stop()?;
    if samples.len() < 4_800 {
        // <0.3 s: accidental tap, nothing to transcribe.
        return Ok(());
    }
    if crate::audio::resample::is_mostly_silence(&samples, 0.01) {
        // No speech energy — skip inference, Whisper would hallucinate.
        return Ok(());
    }

    let settings = state.settings.lock().unwrap().clone();

    // Lazy-load the transcriber (model load takes seconds; do it once).
    {
        let mut guard = state.transcriber.lock().unwrap();
        if guard.is_none() {
            if !models::model_exists(&state.paths.models_dir, &settings.whisper_model) {
                anyhow::bail!(
                    "model not downloaded — open Settings and click 'Download model'"
                );
            }
            let path = state.paths.models_dir.join(&settings.whisper_model);
            *guard = Some(Transcriber::load(&path)?);
        }
    }

    let prompt = dictionary_prompt(&settings.dictionary);
    let raw = {
        let guard = state.transcriber.lock().unwrap();
        guard
            .as_ref()
            .expect("transcriber loaded above")
            .transcribe(&samples, prompt.as_deref(), &settings.language)?
    };
    if raw.is_empty() {
        return Ok(());
    }

    let cleaned = ollama::cleanup(
        &settings.ollama_url,
        &settings.ollama_model,
        &settings.cleanup_level,
        &settings.dictionary,
        &raw,
    );

    inject::inject_text(&cleaned)?;

    let _ = history::append(
        &state.paths.history_file,
        &HistoryEntry {
            timestamp: chrono::Utc::now().to_rfc3339(),
            raw,
            cleaned,
        },
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_to_talk_records_while_held() {
        assert_eq!(trigger_action(true, true, false), TriggerAction::Start);
        assert_eq!(trigger_action(true, false, true), TriggerAction::Finish);
        // Key repeat while already recording, or release when idle: no-ops.
        assert_eq!(trigger_action(true, true, true), TriggerAction::Ignore);
        assert_eq!(trigger_action(true, false, false), TriggerAction::Ignore);
    }

    #[test]
    fn toggle_mode_flips_on_press_and_ignores_release() {
        assert_eq!(trigger_action(false, true, false), TriggerAction::Start);
        assert_eq!(trigger_action(false, true, true), TriggerAction::Finish);
        assert_eq!(trigger_action(false, false, true), TriggerAction::Ignore);
        assert_eq!(trigger_action(false, false, false), TriggerAction::Ignore);
    }
}
