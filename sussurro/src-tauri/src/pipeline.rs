use crate::cleanup::ollama;
use crate::history::{self, HistoryEntry};
use crate::inject;
use crate::settings::SttEngine;
use crate::state::{AppState, Loaded, ModelKey};
use crate::stt::whisper::Transcriber;
use crate::stt::{dictionary_prompt, models, parakeet::ParakeetTranscriber, AnyTranscriber};
use std::sync::{Mutex, MutexGuard};
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
/// spawned.
pub fn handle_trigger(app: &AppHandle, pressed: bool) {
    let state = app.state::<AppState>();
    // A running mic test yields to the real thing: stop it and discard the audio.
    if state.mic_test.swap(false, std::sync::atomic::Ordering::Relaxed) {
        let _ = state.recorder.lock().unwrap().stop();
    }
    let push_to_talk = state.settings.lock().unwrap().push_to_talk;
    let recording = state.recorder.lock().unwrap().is_recording();

    match trigger_action(push_to_talk, pressed, recording) {
        TriggerAction::Ignore => {}
        TriggerAction::Start => {
            let device = state.settings.lock().unwrap().input_device.clone();
            // Long-form segments yield from now until the final pass is
            // done (`process_recording` ends the turn, #154).
            state.dictation.begin();
            if let Err(e) = state.recorder.lock().unwrap().start(&device) {
                state.dictation.end();
                set_status(app, &format!("error: {e}"));
                return;
            }
            state.stream.lock().unwrap().reset(focused_app_name());
            let settings = state.settings.lock().unwrap().clone();
            if settings.sound_feedback {
                crate::audio::beep::record_start();
            }
            set_status(app, "recording");
            if settings.live_preview {
                let app = app.clone();
                std::thread::spawn(move || preview_loop(&app));
            }
        }
        TriggerAction::Finish => {
            if state.settings.lock().unwrap().sound_feedback {
                crate::audio::beep::record_stop();
            }
            set_status(app, "processing");
            let app = app.clone();
            // whisper + ollama take seconds — never block the event thread.
            std::thread::spawn(move || {
                match process_recording(&app) {
                    Ok(()) => set_status(&app, "idle"),
                    Err(e) => set_status(&app, &format!("error: {e:#}")),
                }
            });
        }
    }
}

/// End byte index (exclusive) of the last COMPLETE sentence in `text`,
/// ignoring the trailing `hold_back` bytes (whisper may still revise them).
/// A boundary is `.` `!` `?` followed by whitespace — "3.5" is not one.
pub fn sentence_chunk_end(text: &str, hold_back: usize) -> Option<usize> {
    let safe_len = text.len().saturating_sub(hold_back);
    let mut end = None;
    for (i, c) in text.char_indices() {
        if i >= safe_len {
            break;
        }
        if matches!(c, '.' | '!' | '?') {
            let followed_by_space = text[i + c.len_utf8()..]
                .chars()
                .next()
                .map(|n| n.is_whitespace())
                .unwrap_or(false);
            if followed_by_space {
                end = Some(i + c.len_utf8());
            }
        }
    }
    end
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

/// Load the model a [`ModelKey`] names (takes seconds).
fn load_model(key: &ModelKey) -> anyhow::Result<AnyTranscriber> {
    let models_dir = &key.models_dir;
    Ok(match key.engine {
        SttEngine::Whisper => {
            if !models::model_exists(models_dir, &key.whisper_model) {
                anyhow::bail!("model not downloaded — open Settings and click 'Download model'");
            }
            // settings.json is user-editable — validate before loading.
            let path = models::resolve_model_path(models_dir, &key.whisper_model)?;
            AnyTranscriber::Whisper(Transcriber::load(&path)?)
        }
        SttEngine::Parakeet => {
            if !models::parakeet_exists(models_dir) {
                anyhow::bail!(
                    "Parakeet model not downloaded — open Settings and click 'Download model'"
                );
            }
            let dir = models_dir.join(crate::stt::parakeet::PARAKEET_DIR);
            AnyTranscriber::Parakeet(ParakeetTranscriber::load(&dir)?)
        }
    })
}

/// Lock `slot` and make sure it holds a model loaded for `wanted()` —
/// read *under* the lock, so a caller that waited for the lock (behind a
/// long-form segment or a dictation) uses the settings of now, not of when
/// it started waiting. A model loaded for other settings is the "reload
/// pending" state `set_settings` leaves when the lock is busy (#154): it is
/// dropped (freeing its RAM first) and the wanted one loaded. Generic so
/// tests can drive it without real models.
fn lock_loaded<'a, T>(
    slot: &'a Mutex<Option<Loaded<T>>>,
    wanted: impl FnOnce() -> ModelKey,
    load: impl FnOnce(&ModelKey) -> anyhow::Result<T>,
) -> anyhow::Result<MutexGuard<'a, Option<Loaded<T>>>> {
    let mut guard = slot.lock().unwrap();
    let key = wanted();
    if guard.as_ref().is_some_and(|l| l.key != key) {
        *guard = None;
    }
    if guard.is_none() {
        let model = load(&key)?;
        *guard = Some(Loaded { key, model });
    }
    Ok(guard)
}

/// The shared transcriber, locked and loaded for the current settings.
pub(crate) struct TranscriberGuard<'a>(MutexGuard<'a, Option<Loaded<AnyTranscriber>>>);

impl std::ops::Deref for TranscriberGuard<'_> {
    type Target = AnyTranscriber;
    fn deref(&self) -> &AnyTranscriber {
        &self.0.as_ref().expect("loaded by lock_transcriber").model
    }
}

impl std::ops::DerefMut for TranscriberGuard<'_> {
    fn deref_mut(&mut self) -> &mut AnyTranscriber {
        &mut self.0.as_mut().expect("loaded by lock_transcriber").model
    }
}

/// Lock the shared transcriber, lazily (re)loading the configured engine
/// (load takes seconds; done once per model). Used by the dictation, the
/// HTTP API and the long-form engine — one model in RAM for all.
pub(crate) fn lock_transcriber(state: &AppState) -> anyhow::Result<TranscriberGuard<'_>> {
    // Lock order: transcriber → settings (nothing takes them the other way).
    let guard = lock_loaded(
        &state.transcriber,
        || ModelKey::of(&state.paths, &state.settings.lock().unwrap()),
        load_model,
    )?;
    // Refresh the idle-unload clock while still holding the transcriber lock,
    // so a concurrent idle check can't unload what was just (re)loaded.
    *state.transcriber_last_used.lock().unwrap() = Some(std::time::Instant::now());
    Ok(TranscriberGuard(guard))
}

/// Load the transcriber now (at recording start) so the final pass doesn't pay for it.
pub(crate) fn ensure_transcriber(state: &AppState) -> anyhow::Result<()> {
    lock_transcriber(state).map(drop)
}

/// After a model change: free the old model's RAM now if nobody is using
/// it, without ever waiting for the lock — `set_settings` runs on the main
/// thread and a long-form segment can hold the transcriber for seconds.
/// When busy, [`lock_transcriber`] reloads on the next use (the loaded
/// model's key no longer matches the settings). Returns whether it dropped.
pub(crate) fn release_transcriber_if_free<T>(slot: &Mutex<Option<T>>) -> bool {
    match slot.try_lock() {
        Ok(mut guard) => guard.take().is_some(),
        Err(_) => false,
    }
}

/// Make `settings` the live settings (`set_settings`, main thread; the file
/// is already saved). Never waits for the transcriber — a long-form
/// segment or a dictation can hold it for seconds (#154): a model change
/// frees the old model only if it's idle, and otherwise its next user
/// reloads it ([`lock_transcriber`]). Returns whether the model changed.
pub(crate) fn swap_settings(state: &AppState, settings: crate::settings::Settings) -> bool {
    let model_changed = {
        let mut current = state.settings.lock().unwrap();
        let changed = current.whisper_model != settings.whisper_model
            || current.engine != settings.engine
            || current.models_dir != settings.models_dir;
        *current = settings;
        changed
    };
    if model_changed {
        release_transcriber_if_free(&state.transcriber);
    }
    model_changed
}

/// A loaded model holds hundreds of MB to a few GB of RAM while the app sits
/// idle in the tray. After this much inactivity the transcriber is dropped;
/// the next dictation pays one reload.
const TRANSCRIBER_IDLE_UNLOAD: std::time::Duration = std::time::Duration::from_secs(15 * 60);

/// Periodic idle check, called from the background thread spawned at setup.
/// Never blocks dictation: an active recording or a running long-form
/// session (mic or file, #113) counts as in use, and try_lock skips the
/// round when a transcription holds the transcriber.
pub fn unload_transcriber_if_idle(state: &AppState) -> bool {
    let in_use = state.recorder.lock().unwrap().is_recording() || state.engine.is_active();
    let unloaded = unload_if_idle(
        &state.transcriber,
        &state.transcriber_last_used,
        TRANSCRIBER_IDLE_UNLOAD,
        in_use,
    );
    if unloaded {
        eprintln!(
            "transcriber unloaded after {} min idle",
            TRANSCRIBER_IDLE_UNLOAD.as_secs() / 60
        );
    }
    unloaded
}

/// Generic over the slot content so tests can exercise the locking/expiry
/// logic without loading a real model. Lock order (slot → last_used) matches
/// `ensure_transcriber`, and `last_used` is only read under the slot lock —
/// no unload can race a load that just refreshed the clock.
fn unload_if_idle<T>(
    slot: &std::sync::Mutex<Option<T>>,
    last_used: &std::sync::Mutex<Option<std::time::Instant>>,
    threshold: std::time::Duration,
    in_use: bool,
) -> bool {
    // A recording or an engine session between segments holds no lock but
    // will need the model again in a moment — never idle.
    if in_use {
        return false;
    }
    let Ok(mut guard) = slot.try_lock() else {
        return false; // busy transcribing — obviously not idle
    };
    if guard.is_none() {
        return false;
    }
    let expired = last_used
        .lock()
        .unwrap()
        .is_some_and(|t| t.elapsed() >= threshold);
    if !expired {
        return false;
    }
    *guard = None;
    *last_used.lock().unwrap() = None;
    true
}

/// Cleanup honouring the rule for the focused app: tone instruction plus the
/// per-app output language override (the rule's language beats the global
/// "Translate to" setting).
fn cleanup_for_app(
    settings: &crate::settings::Settings,
    target_app: &str,
    text: &str,
) -> String {
    let rule = crate::cleanup::prompt::find_style_rule(&settings.app_styles, target_app);
    let style = crate::cleanup::prompt::find_style(&settings.app_styles, target_app);
    let lang = crate::cleanup::prompt::effective_output_language(settings, rule);
    if lang != settings.output_language {
        let mut s = settings.clone();
        s.output_language = lang.to_string();
        ollama::cleanup(&s, style, text)
    } else {
        ollama::cleanup(settings, style, text)
    }
}

/// Dictate-to-file mode: append the completed dictation (plus a blank line)
/// to the user's notes file instead of pasting into the focused app.
fn append_to_output_file(path: &str, text: &str) -> anyhow::Result<()> {
    use std::io::Write as _;
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path.trim())?;
    writeln!(f, "{text}\n")?;
    Ok(())
}

/// Count a completed mic dictation in the persistent usage stats. Best-effort:
/// stats must never break the pipeline. File imports are not dictations and
/// don't go through here.
fn record_stats(state: &AppState, cleaned: &str) {
    let day = chrono::Local::now().format("%Y-%m-%d").to_string();
    let _ = crate::stats::record(
        &state.paths.stats_file,
        &day,
        crate::stats::word_count(cleaned),
    );
}

/// Transcribe a batch of 16 kHz mono samples and clean the result, using the
/// current settings. Appends a history entry. Used by the local HTTP API's
/// `POST /transcribe` (no injection, no per-app style); files picked in the
/// app go through the long-form engine instead (#113). Returns (raw, cleaned).
pub fn transcribe_batch(state: &AppState, samples: &[f32]) -> anyhow::Result<(String, String)> {
    let settings = state.settings.lock().unwrap().clone();
    let prompt = dictionary_prompt(&settings.dictionary);
    let raw = lock_transcriber(state)?.transcribe(samples, prompt.as_deref(), &settings.language)?;
    if raw.trim().is_empty() {
        anyhow::bail!("no speech found in the audio");
    }
    let processed = if settings.voice_commands {
        crate::voice_commands::apply_basic_commands(&raw)
    } else {
        raw.clone()
    };
    let cleaned = ollama::cleanup(&settings, None, &processed);
    let _ = history::append(
        &state.paths.history_file,
        &HistoryEntry {
            timestamp: chrono::Utc::now().to_rfc3339(),
            raw: raw.clone(),
            cleaned: cleaned.clone(),
        },
    );
    Ok((raw, cleaned))
}

/// Live preview: while the recording lasts, periodically re-transcribe the
/// accumulated buffer and emit the partial text to the overlay. Best-effort —
/// any failure just means no preview.
fn preview_loop(app: &AppHandle) {
    let state = app.state::<AppState>();
    let settings = state.settings.lock().unwrap().clone();
    if ensure_transcriber(&state).is_err() {
        return; // no model yet — the final pass will surface the error
    }
    let prompt = dictionary_prompt(&settings.dictionary);
    // Dictate-to-file writes once at the end — never type into the focused app.
    let streaming = settings.stream_injection && settings.output_file.trim().is_empty();
    // Word-by-word raw streaming only when there's truly no LLM step: cleanup
    // None AND no translation. Otherwise stream sentence by sentence.
    let raw_streaming = settings.cleanup_level == crate::settings::CleanupLevel::None
        && crate::cleanup::prompt::output_language_name(&settings.output_language).is_none();
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
        let Some(Loaded {
            model: transcriber, ..
        }) = guard.as_mut()
        else {
            return;
        };
        if let Ok(text) = transcriber.transcribe(&samples, prompt.as_deref(), &settings.language)
        {
            if !text.is_empty() {
                let _ = app.emit("partial-transcript", text.clone());
                if streaming && raw_streaming {
                    // Cleanup None: type the stable new words as-is.
                    let mut stream = state.stream.lock().unwrap();
                    if let Some(delta) = stream_delta(&stream.raw_consumed, &text, 2) {
                        if inject::inject_text(delta).is_ok() {
                            let delta = delta.to_string();
                            stream.raw_consumed.push_str(&delta);
                            stream.injected.push_str(&delta);
                        }
                    }
                } else if streaming {
                    // Cleanup on: clean and type sentence by sentence.
                    let chunk = {
                        let stream = state.stream.lock().unwrap();
                        text.strip_prefix(stream.raw_consumed.as_str())
                            .and_then(|rem| {
                                sentence_chunk_end(rem, 12).map(|end| rem[..end].to_string())
                            })
                    };
                    if let Some(chunk) = chunk {
                        // Ollama call happens WITHOUT holding the stream lock.
                        let target_app = state.stream.lock().unwrap().target_app.clone();
                        let cleaned = cleanup_for_app(&settings, &target_app, chunk.trim());
                        let cleaned = cleaned.trim().to_string();
                        if !cleaned.is_empty()
                            && inject::inject_text(&format!("{cleaned} ")).is_ok()
                        {
                            let mut stream = state.stream.lock().unwrap();
                            stream.raw_consumed.push_str(&chunk);
                            stream.injected.push_str(&cleaned);
                            stream.injected.push(' ');
                        }
                    }
                }
            }
        }
    }
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
    // Begun by `handle_trigger` at Start. Ends right after the final
    // transcription — or on any early return — so long-form segments,
    // which yield to it, resume as soon as the model is free (#154).
    let turn = state.dictation.adopt();
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

    let prompt = dictionary_prompt(&settings.dictionary);
    let raw = lock_transcriber(&state)?.transcribe(&samples, prompt.as_deref(), &settings.language)?;
    turn.finish(); // cleanup and injection don't need the model
    if raw.is_empty() {
        return Ok(());
    }

    // Streaming injection already typed most of the text: finish the tail.
    if settings.stream_injection {
        let (raw_consumed, injected_so_far) = {
            let stream = state.stream.lock().unwrap();
            (stream.raw_consumed.clone(), stream.injected.clone())
        };
        if !raw_consumed.is_empty() {
            let rule =
                crate::cleanup::prompt::find_style_rule(&settings.app_styles, &target_app);
            let raw_streaming = settings.cleanup_level == crate::settings::CleanupLevel::None
                && crate::cleanup::prompt::output_language_name(
                    crate::cleanup::prompt::effective_output_language(&settings, rule),
                )
                .is_none();
            let mut final_text = injected_so_far;
            if let Some(tail) = raw.strip_prefix(raw_consumed.as_str()) {
                if !tail.trim().is_empty() {
                    let typed_tail = if raw_streaming {
                        tail.to_string()
                    } else {
                        cleanup_for_app(&settings, &target_app, tail.trim())
                    };
                    inject::inject_text(&typed_tail)?;
                    final_text.push_str(&typed_tail);
                }
            }
            // Prefix mismatch: whisper revised already-typed words — nothing
            // safe to add; what was typed stands.
            record_stats(&state, &final_text);
            let _ = history::append(
                &state.paths.history_file,
                &HistoryEntry {
                    timestamp: chrono::Utc::now().to_rfc3339(),
                    raw,
                    cleaned: final_text,
                },
            );
            return Ok(());
        }
    }

    let to_file = !settings.output_file.trim().is_empty();

    // Voice shortcut: the transcript IS a snippet cue → paste its text, no LLM.
    if let Some(snippet) = crate::snippets::find(&settings.snippets, &raw) {
        if to_file {
            append_to_output_file(&settings.output_file, &snippet.text)?;
        } else {
            inject::inject_text(&snippet.text)?;
        }
        record_stats(&state, &snippet.text);
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

    // Deterministic spoken commands (a capo / new line) work even without
    // the LLM; contextual ones (scratch that) ride the cleanup prompt.
    let processed = if settings.voice_commands {
        crate::voice_commands::apply_basic_commands(&raw)
    } else {
        raw.clone()
    };
    let cleaned = cleanup_for_app(&settings, &target_app, &processed);

    if to_file {
        append_to_output_file(&settings.output_file, &cleaned)?;
    } else {
        inject::inject_text(&cleaned)?;
    }

    record_stats(&state, &cleaned);
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
    fn sentence_chunk_end_finds_last_safe_boundary() {
        // Boundary must be followed by whitespace and outside the hold-back tail.
        let text = "First sentence. Second one! Still being spoken";
        let end = sentence_chunk_end(text, 12).unwrap();
        assert_eq!(&text[..end], "First sentence. Second one!");
        // No boundary at all.
        assert_eq!(sentence_chunk_end("no punctuation here", 12), None);
        // Decimal points are not sentence boundaries.
        assert_eq!(sentence_chunk_end("pi is 3.14159 roughly speaking", 5), None);
        // A boundary inside the hold-back window is not safe yet.
        assert_eq!(sentence_chunk_end("Short. tail", 12), None);
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

    #[test]
    fn idle_unload_drops_an_expired_slot() {
        let slot = std::sync::Mutex::new(Some(1u8));
        let last = std::sync::Mutex::new(
            std::time::Instant::now().checked_sub(std::time::Duration::from_secs(10)),
        );
        assert!(unload_if_idle(&slot, &last, std::time::Duration::from_secs(1), false));
        assert!(slot.lock().unwrap().is_none());
        assert!(last.lock().unwrap().is_none());
    }

    #[test]
    fn idle_unload_keeps_a_recently_used_slot() {
        let slot = std::sync::Mutex::new(Some(1u8));
        let last = std::sync::Mutex::new(Some(std::time::Instant::now()));
        assert!(!unload_if_idle(&slot, &last, std::time::Duration::from_secs(60), false));
        assert!(slot.lock().unwrap().is_some());
    }

    #[test]
    fn idle_unload_ignores_an_empty_or_never_used_slot() {
        let empty: std::sync::Mutex<Option<u8>> = std::sync::Mutex::new(None);
        let stale = std::sync::Mutex::new(
            std::time::Instant::now().checked_sub(std::time::Duration::from_secs(10)),
        );
        assert!(!unload_if_idle(&empty, &stale, std::time::Duration::from_secs(1), false));

        // Loaded but the clock was never set: leave it alone.
        let slot = std::sync::Mutex::new(Some(1u8));
        let never = std::sync::Mutex::new(None);
        assert!(!unload_if_idle(&slot, &never, std::time::Duration::from_secs(1), false));
        assert!(slot.lock().unwrap().is_some());
    }

    #[test]
    fn idle_unload_skips_when_the_slot_is_busy() {
        let slot = std::sync::Mutex::new(Some(1u8));
        let last = std::sync::Mutex::new(
            std::time::Instant::now().checked_sub(std::time::Duration::from_secs(10)),
        );
        let held = slot.lock().unwrap(); // a transcription in flight
        assert!(!unload_if_idle(&slot, &last, std::time::Duration::from_secs(1), false));
        assert!(held.is_some());
    }

    #[test]
    fn idle_unload_keeps_an_expired_slot_while_a_session_is_active() {
        // A long-form session between two segments holds no lock and may not
        // have touched the clock for a while (a long pause on the mic), yet
        // needs the model for the next segment.
        let slot = std::sync::Mutex::new(Some(1u8));
        let last = std::sync::Mutex::new(
            std::time::Instant::now().checked_sub(std::time::Duration::from_secs(10)),
        );
        assert!(!unload_if_idle(&slot, &last, std::time::Duration::from_secs(1), true));
        assert!(slot.lock().unwrap().is_some());
        assert!(last.lock().unwrap().is_some(), "the clock is left alone too");
        // The session ends: the next idle round unloads.
        assert!(unload_if_idle(&slot, &last, std::time::Duration::from_secs(1), false));
        assert!(slot.lock().unwrap().is_none());
    }

    fn key(model: &str) -> ModelKey {
        ModelKey {
            engine: SttEngine::Whisper,
            whisper_model: model.to_string(),
            models_dir: "/models".into(),
        }
    }

    fn test_state(dir: &std::path::Path) -> AppState {
        AppState {
            recorder: Default::default(),
            transcriber: Mutex::new(None),
            transcriber_last_used: Mutex::new(None),
            settings: Mutex::new(crate::settings::Settings::default()),
            paths: crate::state::AppPaths {
                settings_file: dir.join("settings.json"),
                models_dir: dir.join("models"),
                history_file: dir.join("history.jsonl"),
                stats_file: dir.join("stats.json"),
                archive_index: dir.join("index.sqlite"),
                documents_dir: None,
                home_dir: None,
            },
            mic_test: Default::default(),
            stream: Default::default(),
            engine: Default::default(),
            dictation: Default::default(),
            recipe_runs: Default::default(),
            recipe_answers: Default::default(),
            consents: Default::default(),
        }
    }

    /// Hold `slot`'s lock on another thread (a long-form segment
    /// transcribing) until the returned sender is used or dropped.
    fn hold_lock<T: Send + 'static>(
        slot: std::sync::Arc<Mutex<T>>,
    ) -> (std::sync::mpsc::Sender<()>, std::thread::JoinHandle<()>) {
        let (held_tx, held_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
        let t = std::thread::spawn(move || {
            let _g = slot.lock().unwrap();
            held_tx.send(()).unwrap();
            let _ = release_rx.recv();
        });
        held_rx.recv().unwrap();
        (release_tx, t)
    }

    #[test]
    fn a_model_change_never_waits_for_a_busy_transcriber() {
        let dir = tempfile::tempdir().unwrap();
        let state = std::sync::Arc::new(test_state(dir.path()));
        // The lock is held by a "segment" until we say so: if swap_settings
        // waited for it, this test would never get past the call.
        let (release, segment) = {
            let (held_tx, held_rx) = std::sync::mpsc::channel();
            let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
            let state = state.clone();
            let t = std::thread::spawn(move || {
                let _g = state.transcriber.lock().unwrap();
                held_tx.send(()).unwrap();
                let _ = release_rx.recv();
            });
            held_rx.recv().unwrap();
            (release_tx, t)
        };
        let mut s = crate::settings::Settings::default();
        s.whisper_model = format!("{}-other", s.whisper_model);
        assert!(swap_settings(&state, s.clone()), "model change detected");
        assert_eq!(state.settings.lock().unwrap().whisper_model, s.whisper_model);
        // Not a model change: nothing to reload either way.
        s.push_to_talk = !s.push_to_talk;
        assert!(!swap_settings(&state, s));
        release.send(()).unwrap();
        segment.join().unwrap();
    }

    #[test]
    fn a_busy_model_is_reloaded_by_its_next_user_after_a_change() {
        let slot = std::sync::Arc::new(Mutex::new(Some(Loaded {
            key: key("a"),
            model: "model a",
        })));
        let (release, segment) = hold_lock(slot.clone());
        // set_settings while the segment runs: the old model stays for now.
        assert!(!release_transcriber_if_free(&slot));
        release.send(()).unwrap();
        segment.join().unwrap();
        assert_eq!(slot.lock().unwrap().as_ref().unwrap().model, "model a");

        // The next user wants "b": the stale model goes, "b" is loaded.
        let mut loads = Vec::new();
        {
            let g = lock_loaded(
                &slot,
                || key("b"),
                |k| {
                    loads.push(k.clone());
                    Ok("model b")
                },
            )
            .unwrap();
            let loaded = g.as_ref().unwrap();
            assert_eq!((loaded.key.clone(), loaded.model), (key("b"), "model b"));
        }
        assert_eq!(loads, [key("b")]);
        // Same settings: no reload.
        let g = lock_loaded(
            &slot,
            || key("b"),
            |_| -> anyhow::Result<&str> { panic!("reloaded an up-to-date model") },
        )
        .unwrap();
        assert_eq!(g.as_ref().unwrap().model, "model b");
        drop(g);
        // A failed load leaves no stale model behind.
        let err = lock_loaded(
            &slot,
            || key("c"),
            |_| -> anyhow::Result<&str> { anyhow::bail!("model not downloaded") },
        );
        assert!(err.is_err());
        assert!(slot.lock().unwrap().is_none());
        // Idle: a change frees the model at once.
        *slot.lock().unwrap() = Some(Loaded {
            key: key("c"),
            model: "model c",
        });
        assert!(release_transcriber_if_free(&slot));
        assert!(slot.lock().unwrap().is_none());
    }

    #[test]
    fn the_wanted_model_is_read_after_waiting_for_the_lock() {
        // A caller queued behind a segment must load what the settings say
        // when it gets the lock, not what they said when it started waiting.
        let slot = std::sync::Arc::new(Mutex::new(Some(Loaded {
            key: key("a"),
            model: "model a",
        })));
        let wanted = std::sync::Arc::new(Mutex::new(key("a")));
        let (release, segment) = hold_lock(slot.clone());
        let waiter = {
            let (slot, wanted) = (slot.clone(), wanted.clone());
            std::thread::spawn(move || {
                let g = lock_loaded(&slot, || wanted.lock().unwrap().clone(), |_| Ok("model b"))
                    .unwrap();
                g.as_ref().unwrap().key.clone()
            })
        };
        *wanted.lock().unwrap() = key("b"); // settings change while it waits
        release.send(()).unwrap();
        segment.join().unwrap();
        assert_eq!(waiter.join().unwrap(), key("b"));
    }

    #[test]
    fn idle_unload_follows_the_engine_session_registry() {
        // The flag the real unloader passes: `Sessions::is_active`.
        let sessions = crate::engine::session::Sessions::default();
        let slot = std::sync::Mutex::new(Some(1u8));
        let last = std::sync::Mutex::new(
            std::time::Instant::now().checked_sub(std::time::Duration::from_secs(10)),
        );
        let (id, _) = sessions.begin(crate::engine::session::SessionKind::Mic);
        let t = std::time::Duration::from_secs(1);
        assert!(!unload_if_idle(&slot, &last, t, sessions.is_active()));
        sessions.end(id);
        assert!(unload_if_idle(&slot, &last, t, sessions.is_active()));
    }
}
