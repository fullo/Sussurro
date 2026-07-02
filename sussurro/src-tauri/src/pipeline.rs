use crate::cleanup::ollama;
use crate::history::{self, HistoryEntry};
use crate::inject;
use crate::settings::SttEngine;
use crate::state::AppState;
use crate::stt::whisper::Transcriber;
use crate::stt::{dictionary_prompt, models, parakeet::ParakeetTranscriber, AnyTranscriber};
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
            .unwrap_or_else(|_| tauri::PhysicalSize::new(480, 130));
        let x = screen.width.saturating_sub(size.width) / 2;
        let y = screen.height.saturating_sub(size.height + 96);
        let _ = w.set_position(tauri::PhysicalPosition::new(x as i32, y as i32));
    }
}

/// Called from the global-shortcut handler. Must return fast — heavy work is
/// spawned. `command`: the trigger was the command-mode hotkey.
pub fn handle_trigger(app: &AppHandle, pressed: bool, command: bool) {
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
            state
                .command_mode
                .store(command, std::sync::atomic::Ordering::Relaxed);
            state.stream_injected.lock().unwrap().clear();
            let settings = state.settings.lock().unwrap().clone();
            if settings.sound_feedback {
                crate::audio::beep::record_start();
            }
            set_status(app, "recording");
            if settings.live_preview && !command {
                let app = app.clone();
                std::thread::spawn(move || preview_loop(&app));
            }
        }
        TriggerAction::Finish => {
            if state.settings.lock().unwrap().sound_feedback {
                crate::audio::beep::record_stop();
            }
            set_status(app, "processing");
            let was_command = state
                .command_mode
                .load(std::sync::atomic::Ordering::Relaxed);
            let app = app.clone();
            // whisper + ollama take seconds — never block the event thread.
            std::thread::spawn(move || {
                let result = if was_command {
                    process_command(&app)
                } else {
                    process_recording(&app)
                };
                match result {
                    Ok(()) => set_status(&app, "idle"),
                    Err(e) => set_status(&app, &format!("error: {e:#}")),
                }
            });
        }
    }
}

/// Streaming injection: the safe-to-type NEW portion of `partial`, holding
/// back the last `safety_words` words (whisper may still revise them).
/// None when `partial` no longer extends what was already injected.
pub fn stream_delta<'a>(injected: &str, partial: &'a str, safety_words: usize) -> Option<&'a str> {
    let remainder = partial.strip_prefix(injected)?;
    let word_starts: Vec<usize> = remainder
        .char_indices()
        .filter(|(i, c)| {
            !c.is_whitespace()
                && remainder[..*i]
                    .chars()
                    .next_back()
                    .map(|p| p.is_whitespace())
                    .unwrap_or(true)
        })
        .map(|(i, _)| i)
        .collect();
    if word_starts.len() <= safety_words {
        return None;
    }
    let cut = word_starts[word_starts.len() - safety_words];
    let delta = &remainder[..cut];
    if delta.trim().is_empty() {
        None
    } else {
        Some(delta)
    }
}

/// Gain-boost (whisper mode) and silence handling shared by all paths.
/// Returns (prepared samples, silence threshold).
fn prepare_samples(
    mut samples: Vec<f32>,
    settings: &crate::settings::Settings,
) -> (Vec<f32>, f32) {
    let threshold = if settings.whisper_mode { 0.003 } else { 0.01 };
    if settings.whisper_mode {
        crate::audio::resample::boost_gain(&mut samples, 3.0);
    }
    (samples, threshold)
}

/// Lazy-load the configured STT engine into AppState (load takes seconds; do it once).
fn ensure_transcriber(state: &AppState, settings: &crate::settings::Settings) -> anyhow::Result<()> {
    let models_dir = crate::state::resolve_models_dir(&state.paths, settings);
    let mut guard = state.transcriber.lock().unwrap();
    if guard.is_none() {
        *guard = Some(match settings.engine {
            SttEngine::Whisper => {
                if !models::model_exists(&models_dir, &settings.whisper_model) {
                    anyhow::bail!(
                        "model not downloaded — open Settings and click 'Download model'"
                    );
                }
                let path = models_dir.join(&settings.whisper_model);
                AnyTranscriber::Whisper(Transcriber::load(&path)?)
            }
            SttEngine::Parakeet => {
                if !models::parakeet_exists(&models_dir) {
                    anyhow::bail!(
                        "Parakeet model not downloaded — open Settings and click 'Download model'"
                    );
                }
                let dir = models_dir.join(crate::stt::parakeet::PARAKEET_DIR);
                AnyTranscriber::Parakeet(ParakeetTranscriber::load(&dir)?)
            }
        });
    }
    Ok(())
}

/// Live preview: while the recording lasts, periodically re-transcribe the
/// accumulated buffer and emit the partial text to the overlay. Best-effort —
/// any failure just means no preview.
fn preview_loop(app: &AppHandle) {
    let state = app.state::<AppState>();
    let settings = state.settings.lock().unwrap().clone();
    if ensure_transcriber(&state, &settings).is_err() {
        return; // no model yet — the final pass will surface the error
    }
    let prompt = dictionary_prompt(&settings.dictionary);
    let streaming = settings.stream_injection
        && settings.cleanup_level == crate::settings::CleanupLevel::None;
    let mut last_len = 0usize;

    loop {
        std::thread::sleep(std::time::Duration::from_millis(1200));
        if !state.recorder.lock().unwrap().is_recording() {
            return;
        }
        let Some(raw_samples) = state.recorder.lock().unwrap().snapshot_16k() else {
            continue;
        };
        // Wait for at least 1 s of audio and 0.5 s of NEW audio per pass.
        if raw_samples.len() < 16_000 || raw_samples.len() < last_len + 8_000 {
            continue;
        }
        last_len = raw_samples.len();
        let (samples, threshold) = prepare_samples(raw_samples, &settings);
        if crate::audio::resample::is_mostly_silence(&samples, threshold) {
            continue;
        }

        // Never queue behind the final transcription: skip a beat if busy.
        let Ok(mut guard) = state.transcriber.try_lock() else {
            continue;
        };
        let Some(transcriber) = guard.as_mut() else {
            return;
        };
        if let Ok(text) = transcriber.transcribe(&samples, prompt.as_deref(), &settings.language)
        {
            if !text.is_empty() {
                let _ = app.emit("partial-transcript", text.clone());
                if streaming {
                    // Type the stable new words into the target app as we go.
                    let mut injected = state.stream_injected.lock().unwrap();
                    if let Some(delta) = stream_delta(&injected, &text, 2) {
                        if inject::inject_text(delta).is_ok() {
                            let delta = delta.to_string();
                            injected.push_str(&delta);
                        }
                    }
                }
            }
        }
    }
}

/// Command mode: the spoken words are an INSTRUCTION applied to the currently
/// selected text via the LLM; the result replaces the selection.
fn process_command(app: &AppHandle) -> anyhow::Result<()> {
    let state = app.state::<AppState>();
    let samples = state.recorder.lock().unwrap().stop()?;
    if samples.len() < 4_800 {
        return Ok(());
    }
    let settings = state.settings.lock().unwrap().clone();
    let (samples, threshold) = prepare_samples(samples, &settings);
    if crate::audio::resample::is_mostly_silence(&samples, threshold) {
        return Ok(());
    }
    let samples = crate::audio::resample::trim_silence(&samples, threshold, 1_600, 3_200);

    ensure_transcriber(&state, &settings)?;
    let instruction = {
        let mut guard = state.transcriber.lock().unwrap();
        guard
            .as_mut()
            .expect("transcriber loaded above")
            .transcribe(&samples, None, &settings.language)?
    };
    if instruction.is_empty() {
        return Ok(());
    }

    let Some(selection) = inject::copy_selection()? else {
        anyhow::bail!("command mode: select some text first — the instruction is applied to the selection");
    };
    let edited = ollama::command_edit(
        &settings.ollama_url,
        &settings.ollama_model,
        &instruction,
        &selection,
    )?;
    inject::inject_text(&edited)?;

    let _ = history::append(
        &state.paths.history_file,
        &HistoryEntry {
            timestamp: chrono::Utc::now().to_rfc3339(),
            raw: format!("[command] {instruction}"),
            cleaned: edited,
        },
    );
    Ok(())
}

/// Name of the app that will receive the injected text. Read at Finish time,
/// i.e. exactly when the user releases the trigger with the target focused.
fn focused_app_name() -> String {
    active_win_pos_rs::get_active_window()
        .map(|w| w.app_name)
        .unwrap_or_default()
}

fn process_recording(app: &AppHandle) -> anyhow::Result<()> {
    let state = app.state::<AppState>();
    let target_app = focused_app_name();

    let samples = state.recorder.lock().unwrap().stop()?;
    if samples.len() < 4_800 {
        // <0.3 s: accidental tap, nothing to transcribe.
        return Ok(());
    }
    let settings = state.settings.lock().unwrap().clone();
    let (samples, threshold) = prepare_samples(samples, &settings);
    if crate::audio::resample::is_mostly_silence(&samples, threshold) {
        // No speech energy — skip inference, Whisper would hallucinate.
        return Ok(());
    }
    // VAD-lite: don't waste inference on leading/trailing silence.
    let samples = crate::audio::resample::trim_silence(&samples, threshold, 1_600, 3_200);

    ensure_transcriber(&state, &settings)?;

    let prompt = dictionary_prompt(&settings.dictionary);
    let raw = {
        let mut guard = state.transcriber.lock().unwrap();
        guard
            .as_mut()
            .expect("transcriber loaded above")
            .transcribe(&samples, prompt.as_deref(), &settings.language)?
    };
    if raw.is_empty() {
        return Ok(());
    }

    // Streaming injection already typed most of the text: only the tail is
    // missing. Applies only with cleanup None (streamed text is raw).
    if settings.stream_injection
        && settings.cleanup_level == crate::settings::CleanupLevel::None
    {
        let injected = state.stream_injected.lock().unwrap().clone();
        if !injected.is_empty() {
            if let Some(rest) = raw.strip_prefix(injected.as_str()) {
                if !rest.trim().is_empty() {
                    inject::inject_text(rest)?;
                }
            }
            // Prefix mismatch: whisper revised already-typed words — nothing
            // safe to add; the typed text stands.
            let _ = history::append(
                &state.paths.history_file,
                &HistoryEntry {
                    timestamp: chrono::Utc::now().to_rfc3339(),
                    raw: raw.clone(),
                    cleaned: raw,
                },
            );
            return Ok(());
        }
    }

    // Voice shortcut: the transcript IS a snippet cue → paste its text, no LLM.
    if let Some(snippet) = crate::snippets::find(&settings.snippets, &raw) {
        inject::inject_text(&snippet.text)?;
        let _ = history::append(
            &state.paths.history_file,
            &HistoryEntry {
                timestamp: chrono::Utc::now().to_rfc3339(),
                raw,
                cleaned: snippet.text.clone(),
            },
        );
        return Ok(());
    }

    let style = crate::cleanup::prompt::find_style(&settings.app_styles, &target_app);
    let cleaned = ollama::cleanup(
        &settings.ollama_url,
        &settings.ollama_model,
        &settings.cleanup_level,
        &settings.dictionary,
        style,
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

    #[test]
    fn stream_delta_types_only_stable_new_words() {
        // Nothing injected yet: hold back the last 2 words.
        assert_eq!(stream_delta("", "hello brave new world", 2), Some("hello brave "));
        // Continues from what was injected.
        assert_eq!(
            stream_delta("hello brave ", "hello brave new world again now", 2),
            Some("new world ")
        );
        // Too short: nothing safe to type yet.
        assert_eq!(stream_delta("", "hello world", 2), None);
        // Whisper revised the beginning: no longer a prefix, skip.
        assert_eq!(stream_delta("hello brave ", "help brave new world", 2), None);
    }
}
