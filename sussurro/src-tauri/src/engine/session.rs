//! App glue for the long-form engine: session bookkeeping in `AppState`, the
//! shared transcriber, the cleanup client, Tauri events, and the two entry
//! points behind the Tauri commands (mic session, file path).

use super::queue::Policy;
use super::segmenter::{EnergyDetector, SegmenterParams, SpeechDetector};
use super::{Cleaner, EngineEvent, EngineSink, Job, RunResult, SegmentStt};
use crate::archive::{ItemMeta, ItemType};
use crate::settings::{Settings, SttEngine};
use crate::sources::Source;
use crate::state::AppState;
use crate::stt::TimedTranscript;
use anyhow::{Context, Result};
use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};

/// After a hotkey dictation stops recording, the engine waits this long
/// before taking the transcriber again, so the dictation's final pass
/// (which locks it a few ms after the recording stops) goes first.
const DICTATION_GRACE: Duration = Duration::from_millis(400);

struct MicSession {
    id: u64,
    stop: Arc<AtomicBool>,
}

/// Running engine sessions. Lives in `AppState`.
#[derive(Default)]
pub struct Sessions {
    active: AtomicUsize,
    next_id: AtomicU64,
    mic: Mutex<Option<MicSession>>,
    cancels: Mutex<HashMap<u64, Arc<AtomicBool>>>,
}

impl Sessions {
    /// A session is running: the idle unloader must keep the transcriber.
    pub fn is_active(&self) -> bool {
        self.active.load(Ordering::SeqCst) > 0
    }

    pub fn active_count(&self) -> usize {
        self.active.load(Ordering::SeqCst)
    }

    /// Register a new session; returns its id and cancel flag.
    pub fn begin(&self) -> (u64, Arc<AtomicBool>) {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst) + 1;
        let cancel = Arc::new(AtomicBool::new(false));
        self.cancels.lock().unwrap().insert(id, cancel.clone());
        self.active.fetch_add(1, Ordering::SeqCst);
        (id, cancel)
    }

    /// The session ended (done, failed or cancelled).
    pub fn end(&self, id: u64) {
        if self.cancels.lock().unwrap().remove(&id).is_some() {
            self.active.fetch_sub(1, Ordering::SeqCst);
        }
        let mut mic = self.mic.lock().unwrap();
        if mic.as_ref().is_some_and(|m| m.id == id) {
            *mic = None;
        }
    }

    /// Abort a running session; false if there is no such session.
    pub fn cancel(&self, id: u64) -> bool {
        match self.cancels.lock().unwrap().get(&id) {
            Some(c) => {
                c.store(true, Ordering::Relaxed);
                true
            }
            None => false,
        }
    }

    /// Id of the running mic session, if any.
    pub fn mic_session(&self) -> Option<u64> {
        self.mic.lock().unwrap().as_ref().map(|m| m.id)
    }

    /// Ask the mic session to stop recording; the engine then finishes the
    /// queued segments and writes the item. Returns the session id.
    pub fn stop_mic(&self) -> Option<u64> {
        let mic = self.mic.lock().unwrap();
        mic.as_ref().map(|m| {
            m.stop.store(true, Ordering::Relaxed);
            m.id
        })
    }
}

/// Ends the session when the run thread finishes, even on panic.
struct SessionGuard {
    app: AppHandle,
    id: u64,
}

impl Drop for SessionGuard {
    fn drop(&mut self) {
        self.app.state::<AppState>().engine.end(self.id);
    }
}

/// `whisper-<model>` or `parakeet-tdt-0.6b-v3` for the frontmatter. Pure.
pub fn engine_label(settings: &Settings) -> String {
    match settings.engine {
        SttEngine::Whisper => {
            let m = settings.whisper_model.trim();
            let m = m.strip_prefix("ggml-").unwrap_or(m);
            let m = m.strip_suffix(".bin").unwrap_or(m);
            format!("whisper-{m}")
        }
        SttEngine::Parakeet => "parakeet-tdt-0.6b-v3".to_string(),
    }
}

/// The dictation's transcriber, shared. Settings are re-read per segment so
/// a model change mid-session reloads the same engine the dictation uses.
struct AppStt {
    app: AppHandle,
}

impl SegmentStt for AppStt {
    fn transcribe(&mut self, samples: &[f32]) -> Result<TimedTranscript> {
        let state = self.app.state::<AppState>();
        wait_for_dictation(&state);
        for _ in 0..2 {
            let settings = state.settings.lock().unwrap().clone();
            crate::pipeline::ensure_transcriber(&state, &settings)?;
            let prompt = crate::stt::dictionary_prompt(&settings.dictionary);
            // Blocks while a dictation transcribes: the engine waits for it
            // — and a dictation arriving now waits for this one segment.
            let mut guard = state.transcriber.lock().unwrap();
            if let Some(t) = guard.as_mut() {
                return t.transcribe_timed(samples, prompt.as_deref(), &settings.language);
            }
            // Unloaded between ensure and lock (settings changed): reload.
        }
        anyhow::bail!("the speech model was unloaded while transcribing — retry")
    }
}

/// Yield the transcriber to a hotkey dictation that is recording right now.
fn wait_for_dictation(state: &AppState) {
    let dictating =
        || state.recorder.lock().unwrap().is_recording() && !state.mic_test.load(Ordering::Relaxed);
    if !dictating() {
        return;
    }
    while dictating() {
        std::thread::sleep(Duration::from_millis(50));
    }
    std::thread::sleep(DICTATION_GRACE);
}

struct AppCleaner {
    settings: Settings,
}

impl Cleaner for AppCleaner {
    fn clean(&self, previous: Option<&str>, raw: &str) -> String {
        crate::cleanup::ollama::cleanup_with_context(&self.settings, previous, raw)
    }
}

struct TauriSink {
    app: AppHandle,
}

impl EngineSink for TauriSink {
    fn emit(&self, event: &EngineEvent) {
        let _ = self.app.emit(event.name(), event.payload());
    }
}

/// Silero via whisper.cpp when its model is (or can be) downloaded and
/// verified; the energy segmenter otherwise, so a failed download never
/// blocks a transcription.
fn load_detector(models_dir: &Path) -> Box<dyn SpeechDetector> {
    match crate::stt::models::ensure_vad_model(models_dir)
        .and_then(|p| super::vad::SileroDetector::load(&p))
    {
        Ok(d) => Box::new(d),
        Err(e) => {
            eprintln!("Silero VAD unavailable, using the energy segmenter: {e:#}");
            Box::new(EnergyDetector::default())
        }
    }
}

/// A file of the engine's in the app data dir (next to the dictation
/// history): mic spools and the session journal.
fn app_data_file(state: &AppState, name: &str) -> std::path::PathBuf {
    state.paths.history_file.with_file_name(name)
}

/// At app start, before the user can begin a session: items of sessions
/// the previous run never finished become `interrupted` (and are indexed),
/// stale mic spools are removed (#153). Touches the archive folder only
/// when the journal names an item, so a normal start never does.
pub fn recover_after_crash(app: &AppHandle) {
    let state = app.state::<AppState>();
    let settings = state.settings.lock().unwrap().clone();
    let archive = crate::state::resolve_archive_dir(&state.paths, &settings).ok();
    let journal = app_data_file(&state, super::checkpoint::JOURNAL_FILE);
    let spool_dir = journal.parent().map(Path::to_path_buf).unwrap_or_default();
    let r = super::checkpoint::recover(
        &journal,
        archive.as_deref(),
        Some(&state.paths.archive_index),
        &spool_dir,
    );
    if !r.interrupted.is_empty() || r.spools_removed > 0 {
        eprintln!(
            "engine: recovered {} interrupted session item(s), removed {} stale spool(s)",
            r.interrupted.len(),
            r.spools_removed
        );
    }
}

/// Fail a start before any audio is captured when the archive folder
/// can't be written (the run would otherwise lose the recording).
fn ensure_archive_writable(state: &AppState) -> Result<()> {
    let settings = state.settings.lock().unwrap().clone();
    let archive = crate::state::resolve_archive_dir(&state.paths, &settings)?;
    crate::archive::live::ensure_writable(&archive)
}

struct Request {
    id: u64,
    cancel: Arc<AtomicBool>,
    source: Box<dyn Source>,
    policy: Policy,
    defer: bool,
    item_type: ItemType,
    title: String,
    source_label: String,
}

fn run_request(app: &AppHandle, req: Request) -> Result<RunResult> {
    let sink: Arc<dyn EngineSink> = Arc::new(TauriSink { app: app.clone() });
    let state = app.state::<AppState>();
    let settings = state.settings.lock().unwrap().clone();
    let prepared = (|| -> Result<Job> {
        let archive_dir = crate::state::resolve_archive_dir(&state.paths, &settings)?;
        let models_dir = crate::state::resolve_models_dir(&state.paths, &settings);
        Ok(Job {
            session_id: req.id,
            source: req.source,
            detector: load_detector(&models_dir),
            params: SegmenterParams::default(),
            policy: req.policy,
            // App data dir (next to the dictation history), not the shared
            // temp dir: it holds the user's audio while the backlog lasts.
            // The `<prefix><pid>-` part lets the next start tell a dead
            // process's spool from a live one (see `checkpoint`).
            spool_path: app_data_file(
                &state,
                &format!(
                    "{}{}-{}.f32",
                    super::checkpoint::SPOOL_PREFIX,
                    std::process::id(),
                    req.id
                ),
            ),
            defer: req.defer,
            cancel: req.cancel,
            // Spoken "new line" etc. only make sense in the user's own notes.
            voice_commands: settings.voice_commands && req.item_type == ItemType::Note,
            archive_dir,
            index_db: Some(state.paths.archive_index.clone()),
            journal: Some(app_data_file(&state, super::checkpoint::JOURNAL_FILE)),
            meta: ItemMeta {
                item_type: req.item_type,
                title: req.title,
                date: chrono::Local::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, false),
                source: req.source_label,
                language: settings.language.clone(),
                engine: engine_label(&settings),
                ..Default::default()
            },
        })
    })();
    let job = match prepared {
        Ok(job) => job,
        Err(e) => {
            sink.emit(&EngineEvent::Error(super::ErrorPayload {
                session_id: req.id,
                error: format!("{e:#}"),
                item_id: None,
            }));
            return Err(e);
        }
    };
    let mut stt = AppStt { app: app.clone() };
    super::run(job, &mut stt, &AppCleaner { settings }, sink)
}

/// Start a long microphone session (separate from the hotkey). Returns the
/// session id at once; the result arrives as `engine-done`/`engine-error`
/// after [`Sessions::stop_mic`].
pub fn start_mic(app: &AppHandle, item_type: ItemType, title: String, defer: bool) -> Result<u64> {
    let state = app.state::<AppState>();
    let mut mic = state.engine.mic.lock().unwrap();
    if mic.is_some() {
        anyhow::bail!("a microphone session is already running");
    }
    ensure_archive_writable(&state)?;
    let device = state.settings.lock().unwrap().input_device.clone();
    let stop = Arc::new(AtomicBool::new(false));
    let source = crate::sources::mic::MicSource::start(&device, stop.clone())
        .context("could not start the microphone")?;
    let (id, cancel) = state.engine.begin();
    *mic = Some(MicSession { id, stop });
    drop(mic);

    let app = app.clone();
    std::thread::spawn(move || {
        let _guard = SessionGuard {
            app: app.clone(),
            id,
        };
        let req = Request {
            id,
            cancel,
            source: Box::new(source),
            policy: Policy::Spill {
                max_in_ram: super::MIC_MAX_IN_RAM,
            },
            defer,
            item_type,
            title,
            source_label: "mic".to_string(),
        };
        if let Err(e) = run_request(&app, req) {
            eprintln!("mic session {id} failed: {e:#}");
        }
    });
    Ok(id)
}

/// Transcribe a file from its path, blocking until the item is written.
/// Progress arrives as events; the session can be cancelled by id.
pub fn transcribe_file(
    app: &AppHandle,
    path: &Path,
    item_type: ItemType,
    title: String,
) -> Result<RunResult> {
    let state = app.state::<AppState>();
    ensure_archive_writable(&state)?;
    let source = crate::sources::file::FileSource::open(path)?;
    let (id, cancel) = state.engine.begin();
    let _guard = SessionGuard {
        app: app.clone(),
        id,
    };
    let title = if title.trim().is_empty() {
        path.file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default()
    } else {
        title
    };
    run_request(
        app,
        Request {
            id,
            cancel,
            source: Box::new(source),
            policy: Policy::Block {
                max_queued: super::FILE_MAX_QUEUED,
            },
            defer: false,
            item_type,
            title,
            source_label: crate::sources::file::source_label(path),
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sessions_count_active_runs_and_forget_ended_ones() {
        let s = Sessions::default();
        assert!(!s.is_active());
        let (a, cancel_a) = s.begin();
        let (b, _) = s.begin();
        assert_ne!(a, b);
        assert_eq!(s.active_count(), 2);
        assert!(s.cancel(a));
        assert!(cancel_a.load(Ordering::Relaxed));
        s.end(a);
        s.end(a); // idempotent
        assert_eq!(s.active_count(), 1);
        assert!(!s.cancel(a), "an ended session can't be cancelled");
        s.end(b);
        assert!(!s.is_active());
    }

    #[test]
    fn mic_session_stop_and_end() {
        let s = Sessions::default();
        assert_eq!(s.stop_mic(), None);
        let (id, _) = s.begin();
        let stop = Arc::new(AtomicBool::new(false));
        *s.mic.lock().unwrap() = Some(MicSession {
            id,
            stop: stop.clone(),
        });
        assert_eq!(s.mic_session(), Some(id));
        assert_eq!(s.stop_mic(), Some(id));
        assert!(stop.load(Ordering::Relaxed));
        s.end(id);
        assert_eq!(s.mic_session(), None);
        assert!(!s.is_active());
    }

    #[test]
    fn engine_label_names_the_model() {
        let mut s = Settings {
            engine: SttEngine::Whisper,
            whisper_model: "ggml-large-v3-turbo-q5_0.bin".into(),
            ..Default::default()
        };
        assert_eq!(engine_label(&s), "whisper-large-v3-turbo-q5_0");
        s.engine = SttEngine::Parakeet;
        assert_eq!(engine_label(&s), "parakeet-tdt-0.6b-v3");
    }
}
