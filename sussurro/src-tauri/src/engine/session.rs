//! App glue for the long-form engine: session bookkeeping in `AppState`, the
//! shared transcriber, the cleanup client, Tauri events, and the entry
//! points behind the Tauri commands (mic session, file path, link).

use super::queue::Policy;
use super::segmenter::{EnergyDetector, SegmenterParams, SpeechDetector};
use super::{Cleaner, EngineEvent, EngineSink, Job, RunResult, SegmentStt};
use crate::archive::{ItemMeta, ItemType};
use crate::settings::{CleanupLevel, Settings, SttEngine};
use crate::sources::Source;
use crate::state::{AppPaths, AppState};
use crate::stt::TimedTranscript;
use anyhow::{Context, Result};
use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Emitter, Manager};

/// The live capture session: a microphone session, or a system-audio one
/// (#139) — both hold the microphone, so one at a time.
struct MicSession {
    id: u64,
    stop: Arc<AtomicBool>,
    /// A *System audio + mic* session (#139).
    system: bool,
}

/// What a session captures, with the label `engine_status` reports.
#[derive(Debug, Clone, PartialEq)]
pub enum SessionKind {
    Mic,
    /// A file transcription: the file's name.
    File(String),
    /// A link transcription (#123): the link, shortened for display.
    Link(String),
    /// A browser meeting (#126): the meeting page's host.
    Meeting(String),
    /// System audio + mic (#139): the system audio device's name.
    System(String),
}

/// One running session's bookkeeping.
struct Running {
    cancel: Arc<AtomicBool>,
    kind: SessionKind,
}

/// Running engine sessions. Lives in `AppState`.
#[derive(Default)]
pub struct Sessions {
    active: AtomicUsize,
    next_id: AtomicU64,
    mic: Mutex<Option<MicSession>>,
    /// Id of the browser meeting being recorded (#126): one at a time.
    meeting: Mutex<Option<u64>>,
    running: Mutex<HashMap<u64, Running>>,
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
    pub fn begin(&self, kind: SessionKind) -> (u64, Arc<AtomicBool>) {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst) + 1;
        let cancel = Arc::new(AtomicBool::new(false));
        self.running.lock().unwrap().insert(
            id,
            Running {
                cancel: cancel.clone(),
                kind,
            },
        );
        self.active.fetch_add(1, Ordering::SeqCst);
        (id, cancel)
    }

    /// The session ended (done, failed or cancelled).
    pub fn end(&self, id: u64) {
        if self.running.lock().unwrap().remove(&id).is_some() {
            self.active.fetch_sub(1, Ordering::SeqCst);
        }
        let mut mic = self.mic.lock().unwrap();
        if mic.as_ref().is_some_and(|m| m.id == id) {
            *mic = None;
        }
        drop(mic);
        let mut meeting = self.meeting.lock().unwrap();
        if *meeting == Some(id) {
            *meeting = None;
        }
    }

    /// Id of the browser meeting being recorded, if any (#126).
    pub fn meeting_session(&self) -> Option<u64> {
        *self.meeting.lock().unwrap()
    }

    /// Register a browser meeting; fails while another one runs.
    pub fn begin_meeting(&self, host: &str) -> Result<(u64, Arc<AtomicBool>)> {
        let mut meeting = self.meeting.lock().unwrap();
        if meeting.is_some() {
            anyhow::bail!("a meeting is already being recorded");
        }
        let (id, cancel) = self.begin(SessionKind::Meeting(host.to_string()));
        *meeting = Some(id);
        Ok((id, cancel))
    }

    /// Abort a running session; false if there is no such session.
    pub fn cancel(&self, id: u64) -> bool {
        match self.running.lock().unwrap().get(&id) {
            Some(r) => {
                r.cancel.store(true, Ordering::Relaxed);
                true
            }
            None => false,
        }
    }

    /// Running file transcriptions: `(session id, file name)`, oldest
    /// first. Reported by `engine_status` so a UI mounted mid-run (a window
    /// reload, `ui_v2` switched) can show and cancel them (#158).
    pub fn file_sessions(&self) -> Vec<(u64, String)> {
        self.sessions_where(|k| match k {
            SessionKind::File(l) => Some(l.clone()),
            _ => None,
        })
    }

    /// Running link transcriptions (#123): `(session id, link label)`,
    /// oldest first — adopted by a UI mounted mid-run like the files.
    pub fn link_sessions(&self) -> Vec<(u64, String)> {
        self.sessions_where(|k| match k {
            SessionKind::Link(l) => Some(l.clone()),
            _ => None,
        })
    }

    fn sessions_where(&self, label: impl Fn(&SessionKind) -> Option<String>) -> Vec<(u64, String)> {
        let mut out: Vec<(u64, String)> = self
            .running
            .lock()
            .unwrap()
            .iter()
            .filter_map(|(id, r)| label(&r.kind).map(|l| (*id, l)))
            .collect();
        out.sort();
        out
    }

    /// Id of the running mic session, if any (not a system-audio one).
    pub fn mic_session(&self) -> Option<u64> {
        self.capture_session(false)
    }

    /// Id of the running *System audio + mic* session, if any (#139).
    pub fn system_session(&self) -> Option<u64> {
        self.capture_session(true)
    }

    fn capture_session(&self, system: bool) -> Option<u64> {
        self.mic
            .lock()
            .unwrap()
            .as_ref()
            .filter(|m| m.system == system)
            .map(|m| m.id)
    }

    /// Ask the mic session to stop recording; the engine then finishes the
    /// queued segments and writes the item. Returns the session id.
    pub fn stop_mic(&self) -> Option<u64> {
        self.stop_capture(false)
    }

    /// Ask the system-audio session to stop recording (#139), like
    /// [`Self::stop_mic`].
    pub fn stop_system(&self) -> Option<u64> {
        self.stop_capture(true)
    }

    fn stop_capture(&self, system: bool) -> Option<u64> {
        let mic = self.mic.lock().unwrap();
        mic.as_ref().filter(|m| m.system == system).map(|m| {
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
        // A meeting that ended takes its overlay pill with it.
        crate::pipeline::refresh_overlay(&self.app);
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

/// Per-run choices made in *New* (#157): the language hint and the cleanup
/// level for this session only. `None` (or an empty language) falls back to
/// the dictation settings; the global settings are never modified.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RunOptions {
    pub language: Option<String>,
    pub cleanup_level: Option<CleanupLevel>,
    /// "Identify voices" (P11, #134): label a transcription's voices as
    /// "Voice N". Off by default; ignored for notes (never) and meetings
    /// (the 0.9 preview decides, see [`speaker_options`]).
    pub identify_voices: bool,
}

impl RunOptions {
    /// The settings a run uses: a copy of `global` with this run's
    /// overrides. Pure — `global` is only read.
    pub fn apply(&self, global: &Settings) -> Settings {
        let mut s = global.clone();
        if let Some(lang) = self
            .language
            .as_deref()
            .map(str::trim)
            .filter(|l| !l.is_empty())
        {
            s.language = lang.to_string();
        }
        if let Some(level) = &self.cleanup_level {
            s.cleanup_level = level.clone();
        }
        s
    }
}

/// The run's STT: the language is fixed when the run starts (the value the
/// frontmatter records); `inner` transcribes with that hint. In the app,
/// `inner` shares the dictation's transcriber ([`app_transcriber`]); tests
/// pass a fake.
pub(crate) struct RunStt<T> {
    pub language: String,
    pub inner: T,
}

impl<T> SegmentStt for RunStt<T>
where
    T: FnMut(&[f32], &str) -> Result<TimedTranscript> + Send,
{
    fn transcribe(&mut self, samples: &[f32]) -> Result<TimedTranscript> {
        (self.inner)(samples, &self.language)
    }
}

/// The run's chunked cleanup, with the run's settings (overrides applied)
/// fixed when the run starts. In the app `clean` is
/// [`crate::cleanup::ollama::cleanup_with_context`]; tests pass a fake.
pub(crate) struct RunCleaner<C> {
    pub settings: Settings,
    pub clean: C,
}

impl<C> Cleaner for RunCleaner<C>
where
    C: Fn(&Settings, Option<&str>, &str) -> String + Send + Sync,
{
    fn clean(&self, previous: Option<&str>, raw: &str) -> String {
        (self.clean)(&self.settings, previous, raw)
    }
}

/// Resolve what a run takes from the settings, once at its start: the run
/// settings (`global` + `options`, see [`RunOptions::apply`]), the STT with
/// their language and the cleaner with their level. `global` is only read.
pub(crate) fn run_parts<T, C>(
    global: &Settings,
    options: &RunOptions,
    transcribe: T,
    clean: C,
) -> (Settings, RunStt<T>, RunCleaner<C>) {
    let settings = options.apply(global);
    let stt = RunStt {
        language: settings.language.clone(),
        inner: transcribe,
    };
    let cleaner = RunCleaner {
        settings: settings.clone(),
        clean,
    };
    (settings, stt, cleaner)
}

/// The frontmatter a run starts with, from its run settings — so the
/// recorded language is the one the run transcribes with. Pure.
pub(crate) fn start_meta(
    settings: &Settings,
    item_type: ItemType,
    title: String,
    source_label: String,
    date: String,
) -> ItemMeta {
    ItemMeta {
        item_type,
        title,
        date,
        source: source_label,
        language: settings.language.clone(),
        engine: engine_label(settings),
        ..Default::default()
    }
}

/// The dictation's transcriber, shared, as the `inner` of a [`RunStt`]. The
/// model is (re)loaded for the settings current when the lock is taken, so
/// a model change mid-session reloads the same engine the dictation uses;
/// the dictionary is re-read per segment too. The language is the run's.
fn app_transcriber(
    app: AppHandle,
    cancel: Arc<AtomicBool>,
) -> impl FnMut(&[f32], &str) -> Result<TimedTranscript> + Send {
    move |samples, language| {
        let state = app.state::<AppState>();
        // A hotkey dictation recording or waiting for its final pass goes
        // first; one pressed while this segment runs waits for it only
        // (see `priority` for the bound). A cancel ends the wait (#158).
        let mut model = super::priority::acquire_yielding(&state.dictation, &cancel, || {
            crate::pipeline::lock_transcriber(&state)
        })?;
        let dictionary = state.settings.lock().unwrap().dictionary.clone();
        let prompt = crate::stt::dictionary_prompt(&dictionary);
        model.transcribe_timed(samples, prompt.as_deref(), language)
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
fn app_data_file(paths: &AppPaths, name: &str) -> std::path::PathBuf {
    paths.history_file.with_file_name(name)
}

/// The session journal ([`super::checkpoint`]) in the app data dir.
pub fn journal_path(state: &AppState) -> std::path::PathBuf {
    app_data_file(&state.paths, super::checkpoint::JOURNAL_FILE)
}

/// Refuse a delete or a line edit of an item a running capture session is
/// writing — this process's or another instance's (#158). The store also
/// refuses items marked `recording`; this catches a live item whose marker
/// was edited away by hand.
pub fn ensure_not_live(journal: &Path, archive: &Path, id: &str) -> Result<()> {
    if super::checkpoint::owned_by_live_session(journal, archive, id) {
        anyhow::bail!("'{id}' is being written by a running session — stop it first");
    }
    Ok(())
}

/// Folder of the link runs' temporary downloads (#123), in the app data dir.
pub(crate) fn link_temp_dir(paths: &AppPaths) -> std::path::PathBuf {
    app_data_file(paths, crate::sources::url::TEMP_DIR)
}

/// At app start, before the user can begin a session: items of sessions
/// the previous run never finished become `interrupted` (and are indexed),
/// stale mic spools are removed (#153), and so are the downloads of link
/// runs a crash left behind (#123). Touches the archive folder only
/// when the journal names an item, so a normal start never does.
pub fn recover_after_crash(app: &AppHandle) {
    let state = app.state::<AppState>();
    let settings = state.settings.lock().unwrap().clone();
    let archive = crate::state::resolve_archive_dir(&state.paths, &settings).ok();
    let journal = journal_path(&state);
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
    let downloads =
        crate::sources::url::sweep_stale(&link_temp_dir(&state.paths), std::process::id());
    if downloads > 0 {
        eprintln!("engine: removed {downloads} leftover link download(s)");
    }
}

/// Fail a start before any audio is captured when the archive folder
/// can't be written (the run would otherwise lose the recording).
fn ensure_archive_writable(state: &AppState) -> Result<()> {
    let settings = state.settings.lock().unwrap().clone();
    let archive = crate::state::resolve_archive_dir(&state.paths, &settings)?;
    crate::archive::live::ensure_writable(&archive)
}

/// One run to start: the source and what the caller chose for it.
pub(crate) struct Request {
    pub id: u64,
    pub cancel: Arc<AtomicBool>,
    pub source: Box<dyn Source>,
    pub policy: Policy,
    pub defer: bool,
    pub item_type: ItemType,
    pub title: String,
    pub source_label: String,
    pub options: RunOptions,
}

fn run_request(app: &AppHandle, req: Request) -> Result<RunResult> {
    let state = app.state::<AppState>();
    let transcribe = app_transcriber(app.clone(), req.cancel.clone());
    run_request_with(
        &state.settings,
        &state.paths,
        req,
        transcribe,
        crate::cleanup::ollama::cleanup_with_context,
        load_detector,
        Arc::new(TauriSink { app: app.clone() }),
    )
}

/// The body of [`run_request`], with the app's pieces passed in so tests
/// drive the real path (#158): `shared` is the app's settings, read
/// **once** when the run starts and never written — this run's language
/// and cleanup level (#157) apply to a copy, and later changes to the
/// settings don't reach a run in flight.
pub(crate) fn run_request_with<T, C>(
    shared: &Mutex<Settings>,
    paths: &AppPaths,
    req: Request,
    transcribe: T,
    clean: C,
    detector: impl FnOnce(&Path) -> Box<dyn SpeechDetector>,
    sink: Arc<dyn EngineSink>,
) -> Result<RunResult>
where
    T: FnMut(&[f32], &str) -> Result<TimedTranscript> + Send,
    C: Fn(&Settings, Option<&str>, &str) -> String + Send + Sync,
{
    run_request_with_speakers(
        shared,
        paths,
        req,
        transcribe,
        clean,
        detector,
        embedder_loader,
        sink,
    )
}

/// [`run_request_with`] with the speaker model injected too (#134): tests
/// pass a fake embedder instead of the downloaded WeSpeaker. `speaker_model`
/// is called only when the run labels voices ([`speaker_options`]).
#[allow(clippy::too_many_arguments)]
pub(crate) fn run_request_with_speakers<T, C>(
    shared: &Mutex<Settings>,
    paths: &AppPaths,
    req: Request,
    transcribe: T,
    clean: C,
    detector: impl FnOnce(&Path) -> Box<dyn SpeechDetector>,
    speaker_model: impl FnOnce(std::path::PathBuf) -> crate::speakers::tracker::EmbedderLoader,
    sink: Arc<dyn EngineSink>,
) -> Result<RunResult>
where
    T: FnMut(&[f32], &str) -> Result<TimedTranscript> + Send,
    C: Fn(&Settings, Option<&str>, &str) -> String + Send + Sync,
{
    let global = shared.lock().unwrap().clone();
    // This run's settings: the dictation's plus the choices made in New
    // (#157). The global settings are not modified.
    let (settings, mut stt, cleaner) = run_parts(&global, &req.options, transcribe, clean);
    let prepared = (|| -> Result<Job> {
        let archive_dir = crate::state::resolve_archive_dir(paths, &settings)?;
        let models_dir = crate::state::resolve_models_dir(paths, &settings);
        let speakers = speaker_options(
            &settings,
            req.item_type,
            req.source.channel(),
            &req.source_label,
            req.options.identify_voices,
        )
        .map(|o| crate::speakers::Tracker::new(o, speaker_model(models_dir.clone())));
        Ok(Job {
            session_id: req.id,
            source: req.source,
            detector: detector(&models_dir),
            params: SegmenterParams::default(),
            policy: req.policy,
            // App data dir (next to the dictation history), not the shared
            // temp dir: it holds the user's audio while the backlog lasts.
            // The `<prefix><pid>-` part lets the next start tell a dead
            // process's spool from a live one (see `checkpoint`).
            spool_path: app_data_file(
                paths,
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
            index_db: Some(paths.archive_index.clone()),
            journal: Some(app_data_file(paths, super::checkpoint::JOURNAL_FILE)),
            external_cleanup: external_cleanup_entry(&settings),
            speakers,
            write_subtitles: settings.subtitles == crate::settings::SubtitlesMode::Always,
            meta: start_meta(
                &settings,
                req.item_type,
                req.title,
                req.source_label,
                chrono::Local::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, false),
            ),
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
    super::run(job, &mut stt, &cleaner, sink)
}

/// Which channels a run labels with "Voice N" (#130):
/// - notes never (P10: the user's own voice);
/// - transcriptions only with this run's "Identify voices" toggle (P11,
///   #134), whatever the 0.9 preview flag says — the flag gates meeting
///   pieces only;
/// - meetings behind the 0.9 flag (`meetings_enabled`, E12).
///
/// What is clustered:
/// - One channel (the in-room case, a file or a link): it is clustered.
/// - A browser meeting (`source: browser:<host>`, #126) records the mic
///   and the remote side apart: the mic is always "You" and only the
///   remote channel is clustered (names from the meeting page are #131).
/// - System audio + mic (`source: system`, #139) likewise: the mic is
///   "You" and the system channel is clustered.
///
/// Pure.
pub(crate) fn speaker_options(
    settings: &Settings,
    item_type: ItemType,
    channel: crate::archive::Channel,
    source_label: &str,
    identify_voices: bool,
) -> Option<crate::speakers::SpeakerOptions> {
    use crate::archive::Channel;
    let on = match item_type {
        ItemType::Note => false,
        ItemType::Transcription => identify_voices,
        ItemType::Meeting => settings.meetings_enabled,
    };
    if !on {
        return None;
    }
    Some(if source_label.starts_with("browser:") {
        crate::speakers::SpeakerOptions {
            cluster: vec![Channel::Remote],
            two_channel: true,
        }
    } else if source_label == crate::sources::system::SOURCE_LABEL {
        crate::speakers::SpeakerOptions {
            cluster: vec![Channel::System],
            two_channel: true,
        }
    } else {
        crate::speakers::SpeakerOptions::clustering(&[channel])
    })
}

/// Loads the speaker model when a run first needs it: downloaded into the
/// models folder on first use and verified against its pinned SHA-256.
pub(crate) fn embedder_loader(models_dir: std::path::PathBuf) -> crate::speakers::tracker::EmbedderLoader {
    Box::new(move || {
        let path = crate::speakers::model::ensure_model(&models_dir)?;
        let model = crate::speakers::model::WeSpeaker::load(&path)?;
        Ok(Box::new(model) as Box<dyn crate::speakers::model::SpeakerEmbedder>)
    })
}

/// The external-send entry of a run whose cleanup goes to an external
/// profile the user opted in for (#122), recorded by the engine before the
/// first segment is cleaned; `None` when cleanup stays on this machine or
/// sends nothing (level None, no translation, no opt-in).
fn external_cleanup_entry(settings: &Settings) -> Option<crate::archive::external::ExternalSend> {
    if !settings.cleanup_sends_externally() {
        return None;
    }
    let p = settings.cleanup_llm();
    Some(crate::archive::external::ExternalSend {
        date: chrono::Local::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, false),
        host: p.host(),
        profile: p.name.trim().to_string(),
        model: p.model.trim().to_string(),
        kind: crate::archive::external::SendKind::Cleanup,
        recipe: String::new(),
    })
}

/// Start a long microphone session (separate from the hotkey). Returns the
/// session id at once; the result arrives as `engine-done`/`engine-error`
/// after [`Sessions::stop_mic`]. `options`: this run's language and cleanup
/// level (#157).
pub fn start_mic(
    app: &AppHandle,
    item_type: ItemType,
    title: String,
    defer: bool,
    options: RunOptions,
) -> Result<u64> {
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
    let (id, cancel) = state.engine.begin(SessionKind::Mic);
    *mic = Some(MicSession {
        id,
        stop,
        system: false,
    });
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
            options,
        };
        if let Err(e) = run_request(&app, req) {
            eprintln!("mic session {id} failed: {e:#}");
        }
    });
    Ok(id)
}

/// A *System audio + mic* session records other people (#139): like the
/// browser meetings it is a 0.9 meeting piece, behind `meetings_enabled`
/// (E12) until #138 — checked here, not only in the UI. Pure.
pub(crate) fn ensure_system_audio_allowed(settings: &Settings) -> Result<()> {
    if !settings.meetings_enabled {
        anyhow::bail!(
            "recording system audio is part of the meetings preview — turn it on in Settings → Browser extension"
        );
    }
    Ok(())
}

/// The run of a *System audio + mic* session (#139): a `meeting` item
/// whose source is `system`, fed live (the queue spills like a mic
/// session's). The mic channel is "You", the system channel is clustered
/// ([`speaker_options`]).
pub(crate) fn system_request(
    id: u64,
    cancel: Arc<AtomicBool>,
    source: crate::sources::system::SystemSource,
    title: String,
    defer: bool,
    options: RunOptions,
) -> Request {
    Request {
        id,
        cancel: cancel.clone(),
        source: Box::new(source.with_cancel(cancel)),
        policy: Policy::Spill {
            max_in_ram: super::MIC_MAX_IN_RAM,
        },
        defer,
        item_type: ItemType::Meeting,
        title,
        source_label: crate::sources::system::SOURCE_LABEL.to_string(),
        options,
    }
}

/// Start a *System audio + mic* session (#139): the microphone
/// (`mic_device`, empty = the dictation's input device) and a second input
/// device carrying the computer's output (`system_device`, e.g. BlackHole)
/// as two channels of one meeting. Returns the session id at once; the item
/// arrives as `engine-done` after [`Sessions::stop_system`]. Refused without
/// the meetings preview, with the same device twice, or while a mic session
/// runs (both need the microphone).
pub fn start_system(
    app: &AppHandle,
    mic_device: Option<String>,
    system_device: &str,
    title: String,
    defer: bool,
    options: RunOptions,
) -> Result<u64> {
    let state = app.state::<AppState>();
    let settings = state.settings.lock().unwrap().clone();
    ensure_system_audio_allowed(&settings)?;
    let mic_device = mic_device.unwrap_or(settings.input_device);
    crate::sources::system::validate_devices(
        &mic_device,
        system_device,
        crate::audio::recorder::default_input_device_name().as_deref(),
    )?;
    if !crate::audio::recorder::list_input_devices()
        .iter()
        .any(|d| d == system_device)
    {
        anyhow::bail!("the input device '{system_device}' is not available — is it connected?");
    }
    let mut mic = state.engine.mic.lock().unwrap();
    if let Some(m) = mic.as_ref() {
        anyhow::bail!(if m.system {
            "a system audio session is already running"
        } else {
            "a microphone session is running — stop it first"
        });
    }
    ensure_archive_writable(&state)?;
    let stop = Arc::new(AtomicBool::new(false));
    let source = crate::sources::system::SystemSource::start(&mic_device, system_device, stop.clone())
        .context("could not start the audio devices")?;
    let (id, cancel) = state
        .engine
        .begin(SessionKind::System(system_device.to_string()));
    *mic = Some(MicSession {
        id,
        stop,
        system: true,
    });
    drop(mic);

    let app = app.clone();
    std::thread::spawn(move || {
        let _guard = SessionGuard {
            app: app.clone(),
            id,
        };
        let req = system_request(id, cancel, source, title, defer, options);
        if let Err(e) = run_request(&app, req) {
            eprintln!("system audio session {id} failed: {e:#}");
        }
    });
    Ok(id)
}

/// The run of a browser meeting (#126): a `meeting` item whose source is
/// `browser:<host>`, fed live (the queue spills like a mic session's).
pub(crate) fn meeting_request(
    id: u64,
    cancel: Arc<AtomicBool>,
    start: &crate::api::protocol::Start,
    source: crate::sources::browser::BrowserSource,
) -> Request {
    Request {
        id,
        cancel: cancel.clone(),
        source: Box::new(source.with_cancel(cancel)),
        policy: Policy::Spill {
            max_in_ram: super::MIC_MAX_IN_RAM,
        },
        defer: false,
        item_type: ItemType::Meeting,
        title: start.title.clone(),
        source_label: start.source_label(),
        options: RunOptions::default(),
    }
}

/// Start recording a browser meeting from the `/live` WebSocket (#126).
/// Returns the session id at once; the run ends when the extension stops
/// (or goes away) and its events go to the app and to `meeting.sink`. The
/// overlay pill shows "Recording (meeting)" meanwhile.
pub fn start_meeting(app: &AppHandle, meeting: crate::api::live::MeetingStart) -> Result<u64> {
    let state = app.state::<AppState>();
    ensure_archive_writable(&state)?;
    let crate::api::live::MeetingStart {
        start,
        source,
        sink,
    } = meeting;
    let (id, cancel) = state.engine.begin_meeting(&start.host)?;
    crate::pipeline::refresh_overlay(app);
    let app = app.clone();
    std::thread::spawn(move || {
        let _guard = SessionGuard {
            app: app.clone(),
            id,
        };
        let state = app.state::<AppState>();
        let req = meeting_request(id, cancel.clone(), &start, source);
        let sinks: Vec<Arc<dyn EngineSink>> = vec![Arc::new(TauriSink { app: app.clone() }), sink];
        if let Err(e) = run_request_with(
            &state.settings,
            &state.paths,
            req,
            app_transcriber(app.clone(), cancel),
            crate::cleanup::ollama::cleanup_with_context,
            load_detector,
            Arc::new(super::TeeSink(sinks)),
        ) {
            eprintln!("meeting session {id} failed: {e:#}");
        }
    });
    Ok(id)
}

/// Transcribe a file from its path, blocking until the item is written.
/// Progress arrives as events; the session can be cancelled by id.
/// `options`: this run's language and cleanup level (#157).
pub fn transcribe_file(
    app: &AppHandle,
    path: &Path,
    item_type: ItemType,
    title: String,
    options: RunOptions,
) -> Result<RunResult> {
    let state = app.state::<AppState>();
    ensure_archive_writable(&state)?;
    let source = crate::sources::file::FileSource::open(path)?;
    let file_name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let (id, cancel) = state.engine.begin(SessionKind::File(file_name));
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
    let result = run_request(
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
            options,
        },
    )?;
    remember_source_file(&state, &result, path);
    Ok(result)
}

/// The app data file remembering where transcriptions' files came from
/// (#134, [`super::source_files`]).
pub fn source_files_path(state: &AppState) -> std::path::PathBuf {
    app_data_file(&state.paths, super::source_files::FILE)
}

/// After a file transcription: remember the file on this machine so
/// "Identify voices" can run later (#134). Transcriptions only — notes
/// never have speakers (P10). A failure is only logged.
fn remember_source_file(state: &AppState, result: &RunResult, path: &Path) {
    if result.meta.item_type != ItemType::Transcription {
        return;
    }
    let settings = state.settings.lock().unwrap().clone();
    let recorded = crate::state::resolve_archive_dir(&state.paths, &settings).and_then(|archive| {
        super::source_files::record(&source_files_path(state), &archive, &result.item_id, path)
    });
    if let Err(e) = recorded {
        eprintln!("engine: original file of {} not remembered ({e:#})", result.item_id);
    }
}

/// One link run to start (#123).
pub(crate) struct LinkRequest {
    pub id: u64,
    pub cancel: Arc<AtomicBool>,
    pub link: crate::sources::url::Link,
    /// The user ticked "Allow local network addresses" for this run.
    pub allow_local: bool,
    /// As typed; empty = the platform's title, else one from the link.
    pub title: String,
    pub options: RunOptions,
}

/// Minimum wall time between two `engine-download` events.
const DOWNLOAD_EVENTS_EVERY: std::time::Duration = std::time::Duration::from_millis(250);

/// A link run: fetch the audio to a temporary file (emitting
/// `engine-download`), then transcribe it like a file into a
/// `transcription` item whose source is `url:<link>` (P10). The temporary
/// file is removed when this returns, whatever the outcome; a failure
/// before the transcription starts is reported as `engine-error`. The
/// app's pieces are passed in so tests drive the real path.
#[allow(clippy::too_many_arguments)]
pub(crate) fn run_link_with<T, C>(
    shared: &Mutex<Settings>,
    paths: &AppPaths,
    req: LinkRequest,
    max_bytes: u64,
    find_yt_dlp: &dyn Fn() -> Option<std::path::PathBuf>,
    transcribe: T,
    clean: C,
    detector: impl FnOnce(&Path) -> Box<dyn SpeechDetector>,
    sink: Arc<dyn EngineSink>,
) -> Result<RunResult>
where
    T: FnMut(&[f32], &str) -> Result<TimedTranscript> + Send,
    C: Fn(&Settings, Option<&str>, &str) -> String + Send + Sync,
{
    use crate::sources::url;
    let LinkRequest {
        id,
        cancel,
        link,
        allow_local,
        title,
        options,
    } = req;
    let fail = |e: anyhow::Error| {
        sink.emit(&EngineEvent::Error(super::ErrorPayload {
            session_id: id,
            error: format!("{e:#}"),
            item_id: None,
        }));
        Err(e)
    };
    let temp = match url::TempDownload::create(&link_temp_dir(paths), id) {
        Ok(t) => t,
        Err(e) => return fail(e),
    };
    let mut last: Option<(std::time::Instant, url::Via, Option<String>)> = None;
    let mut on_progress = |via: url::Via, p: &url::Progress| {
        // Throttled, except when the way or the title changes.
        let due = match &last {
            None => true,
            Some((at, v, t)) => *v != via || *t != p.title || at.elapsed() >= DOWNLOAD_EVENTS_EVERY,
        };
        if due {
            last = Some((std::time::Instant::now(), via, p.title.clone()));
            sink.emit(&EngineEvent::Download(super::DownloadPayload {
                session_id: id,
                via,
                downloaded_bytes: p.downloaded,
                total_bytes: p.total,
                title: p.title.clone(),
            }));
        }
    };
    let fetched = url::fetch(
        &link,
        allow_local,
        max_bytes,
        &temp,
        &cancel,
        find_yt_dlp,
        &mut on_progress,
    );
    let fetched = match fetched {
        Ok(f) => f,
        Err(e) => return fail(e),
    };
    let source = match crate::sources::file::FileSource::open(&fetched.path) {
        Ok(s) => s,
        Err(e) => {
            return fail(url::undecodable(
                &fetched.path,
                fetched.via == url::Via::YtDlp,
                &e,
            ))
        }
    };
    let title = if !title.trim().is_empty() {
        title
    } else {
        fetched
            .title
            .filter(|t| !t.trim().is_empty())
            .unwrap_or_else(|| url::title_from_url(&link.url))
    };
    let result = run_request_with(
        shared,
        paths,
        Request {
            id,
            cancel,
            source: Box::new(source),
            policy: Policy::Block {
                max_queued: super::FILE_MAX_QUEUED,
            },
            defer: false,
            // Audio recorded by others (P10).
            item_type: ItemType::Transcription,
            title,
            source_label: url::source_label(&link.url),
            options,
        },
        transcribe,
        clean,
        detector,
        sink,
    );
    drop(temp);
    result
}

/// Start transcribing a link (#123). Validation, the yt-dlp check and the
/// archive check fail at once; otherwise returns the session id and runs
/// in the background (`engine-download`, then the usual `engine-*`).
pub fn start_link(
    app: &AppHandle,
    input: &str,
    title: String,
    allow_local: bool,
    options: RunOptions,
) -> Result<u64> {
    use crate::sources::url;
    let link = url::parse_link(input)?;
    if link.kind == url::LinkKind::Platform && url::ytdlp::find().is_none() {
        return Err(url::ytdlp::missing_error());
    }
    let state = app.state::<AppState>();
    ensure_archive_writable(&state)?;
    let (id, cancel) = state
        .engine
        .begin(SessionKind::Link(url::display_label(&link.url)));
    let app = app.clone();
    std::thread::spawn(move || {
        let _guard = SessionGuard {
            app: app.clone(),
            id,
        };
        let state = app.state::<AppState>();
        let req = LinkRequest {
            id,
            cancel: cancel.clone(),
            link,
            allow_local,
            title,
            options,
        };
        if let Err(e) = run_link_with(
            &state.settings,
            &state.paths,
            req,
            url::MAX_DOWNLOAD_BYTES,
            &url::ytdlp::find,
            app_transcriber(app.clone(), cancel),
            crate::cleanup::ollama::cleanup_with_context,
            load_detector,
            Arc::new(TauriSink { app: app.clone() }),
        ) {
            eprintln!("link session {id} failed: {e:#}");
        }
    });
    Ok(id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sessions_count_active_runs_and_forget_ended_ones() {
        let s = Sessions::default();
        assert!(!s.is_active());
        let (a, cancel_a) = s.begin(SessionKind::Mic);
        let (b, _) = s.begin(SessionKind::File("call.wav".into()));
        let (c, _) = s.begin(SessionKind::Link("youtube.com/watch?v=x".into()));
        assert_ne!(a, b);
        assert_eq!(s.active_count(), 3);
        // #158: a UI mounted mid-run learns about the file transcription,
        // and (#123) about the link.
        assert_eq!(s.file_sessions(), [(b, "call.wav".to_string())]);
        assert_eq!(
            s.link_sessions(),
            [(c, "youtube.com/watch?v=x".to_string())]
        );
        s.end(c);
        assert_eq!(s.active_count(), 2);
        assert!(s.link_sessions().is_empty());
        assert!(s.cancel(a));
        assert!(cancel_a.load(Ordering::Relaxed));
        s.end(a);
        s.end(a); // idempotent
        assert_eq!(s.active_count(), 1);
        assert!(!s.cancel(a), "an ended session can't be cancelled");
        s.end(b);
        assert!(!s.is_active());
        assert!(s.file_sessions().is_empty());
    }

    #[test]
    fn one_meeting_at_a_time() {
        let s = Sessions::default();
        assert_eq!(s.meeting_session(), None);
        let (id, _) = s.begin_meeting("meet.google.com").unwrap();
        assert_eq!(s.meeting_session(), Some(id));
        assert!(s.is_active(), "the idle unloader keeps the model");
        assert!(s.begin_meeting("teams.microsoft.com").is_err());
        assert!(s.file_sessions().is_empty() && s.link_sessions().is_empty());
        s.end(id);
        assert_eq!(s.meeting_session(), None);
        assert!(!s.is_active());
        assert!(s.begin_meeting("meet.google.com").is_ok());
    }

    #[test]
    fn mic_session_stop_and_end() {
        let s = Sessions::default();
        assert_eq!(s.stop_mic(), None);
        let (id, _) = s.begin(SessionKind::Mic);
        let stop = Arc::new(AtomicBool::new(false));
        *s.mic.lock().unwrap() = Some(MicSession {
            id,
            stop: stop.clone(),
            system: false,
        });
        assert_eq!(s.mic_session(), Some(id));
        assert_eq!(s.system_session(), None);
        assert!(s.file_sessions().is_empty(), "the mic is not a file");
        assert_eq!(s.stop_system(), None, "not a system-audio session");
        assert!(!stop.load(Ordering::Relaxed));
        assert_eq!(s.stop_mic(), Some(id));
        assert!(stop.load(Ordering::Relaxed));
        s.end(id);
        assert_eq!(s.mic_session(), None);
        assert!(!s.is_active());
    }

    #[test]
    fn system_session_is_reported_and_stopped_apart_from_the_mic() {
        let s = Sessions::default();
        let (id, _) = s.begin(SessionKind::System("BlackHole 2ch".into()));
        let stop = Arc::new(AtomicBool::new(false));
        *s.mic.lock().unwrap() = Some(MicSession {
            id,
            stop: stop.clone(),
            system: true,
        });
        // A UI mounted mid-session adopts it as system audio, not as a mic
        // session, and the Microphone tab's Stop can't end it.
        assert_eq!((s.system_session(), s.mic_session()), (Some(id), None));
        assert_eq!(s.stop_mic(), None);
        assert!(!stop.load(Ordering::Relaxed));
        assert_eq!(s.stop_system(), Some(id));
        assert!(stop.load(Ordering::Relaxed));
        assert!(s.file_sessions().is_empty() && s.link_sessions().is_empty());
        s.end(id);
        assert_eq!(s.system_session(), None);
        assert!(!s.is_active());
    }

    #[test]
    fn system_audio_is_gated_by_the_meetings_preview() {
        let off = Settings::default();
        assert!(!off.meetings_enabled, "off by default (E12)");
        assert!(ensure_system_audio_allowed(&off).is_err());
        let on = Settings {
            meetings_enabled: true,
            ..Default::default()
        };
        assert!(ensure_system_audio_allowed(&on).is_ok());
    }

    #[test]
    fn a_system_audio_run_is_a_meeting_from_source_system() {
        use crate::sources::system::{Capture, Pacer, SystemSource};
        struct Silent;
        impl Capture for Silent {
            fn take(&mut self) -> Vec<f32> {
                Vec::new()
            }
            fn failed(&self) -> bool {
                false
            }
            fn stop(&mut self) -> Vec<f32> {
                Vec::new()
            }
        }
        struct NoWait;
        impl Pacer for NoWait {
            fn wait(&mut self) {}
            fn now(&self) -> u64 {
                0
            }
        }
        let cancel = Arc::new(AtomicBool::new(false));
        let source = SystemSource::from_parts(
            Box::new(Silent),
            Box::new(Silent),
            Box::new(NoWait),
            Arc::new(AtomicBool::new(false)),
        );
        let options = RunOptions {
            language: Some("it".into()),
            ..Default::default()
        };
        let req = system_request(3, cancel.clone(), source, "Standup".into(), false, options.clone());
        assert_eq!(req.item_type, ItemType::Meeting);
        assert_eq!(req.source_label, "system");
        assert_eq!(req.source.channel(), crate::archive::Channel::System);
        assert_eq!((req.title.as_str(), req.options), ("Standup", options));
        assert!(matches!(req.policy, Policy::Spill { .. }), "a live source spills");
        // The run's cancel reaches the source: it stops waiting for audio.
        cancel.store(true, Ordering::Relaxed);
        let mut source = req.source;
        assert_eq!(source.next_frame().unwrap(), None);
    }

    #[test]
    fn run_options_override_a_copy_and_fall_back_to_settings() {
        let global = Settings {
            language: "it".into(),
            cleanup_level: CleanupLevel::Light,
            ..Default::default()
        };
        let before = global.clone();
        let o = RunOptions {
            language: Some(" en ".into()),
            cleanup_level: Some(CleanupLevel::None),
            identify_voices: true,
        };
        let s = o.apply(&global);
        assert_eq!(
            (s.language.as_str(), s.cleanup_level.clone()),
            ("en", CleanupLevel::None)
        );
        assert_eq!(global, before);
        // Unset or blank: the dictation settings.
        assert_eq!(RunOptions::default().apply(&global), global);
        let blank = RunOptions {
            language: Some(String::new()),
            ..Default::default()
        };
        assert_eq!(blank.apply(&global), global);
        // The frontmatter records the run's language.
        let meta = start_meta(&s, ItemType::Note, "t".into(), "mic".into(), "d".into());
        assert_eq!(meta.language, "en");
    }

    /// #158: the delete / line-edit commands refuse an item a running
    /// session still owns, even with its `recording` marker edited away.
    #[test]
    fn a_live_sessions_item_is_refused_by_the_archive_commands() {
        let tmp = tempfile::tempdir().unwrap();
        let archive = tmp.path().join("Sussurro");
        let journal = tmp.path().join(super::super::checkpoint::JOURNAL_FILE);
        let meta = start_meta(
            &Settings::default(),
            ItemType::Note,
            "Live".into(),
            "mic".into(),
            "2026-09-24T10:00:00+02:00".into(),
        );
        let live =
            super::super::checkpoint::LiveItem::begin(&archive, &meta, Some(&journal)).unwrap();
        let id = live.id().to_string();
        let err = ensure_not_live(&journal, &archive, &id).unwrap_err();
        assert!(err.to_string().contains("running session"), "{err}");
        assert!(ensure_not_live(&journal, &archive, "2026/09/other").is_ok());
        assert!(live.discard().is_none());
        assert!(ensure_not_live(&journal, &archive, &id).is_ok());
    }

    /// #130 / #134: who gets "Voice N" — the gating matrix of item type ×
    /// 0.9 preview flag × the run's "Identify voices" toggle. Notes never;
    /// transcriptions exactly when the toggle is on, whatever the flag;
    /// meetings exactly when the flag is on, whatever the toggle.
    #[test]
    fn speaker_labels_gating_matrix() {
        use crate::archive::Channel;
        for flag in [false, true] {
            let settings = Settings {
                meetings_enabled: flag,
                ..Default::default()
            };
            for identify in [false, true] {
                let note = speaker_options(&settings, ItemType::Note, Channel::Mic, "mic", identify);
                assert_eq!(note, None, "notes never (flag {flag}, toggle {identify})");
                let note_file =
                    speaker_options(&settings, ItemType::Note, Channel::File, "file:a.wav", identify);
                assert_eq!(note_file, None, "a voice memo from a file is still a note");

                for source in ["file:a.wav", "url:https://x.org/a.mp3"] {
                    let t = speaker_options(
                        &settings,
                        ItemType::Transcription,
                        Channel::File,
                        source,
                        identify,
                    );
                    assert_eq!(
                        t.is_some(),
                        identify,
                        "transcription {source}: flag {flag}, toggle {identify}"
                    );
                    if let Some(o) = t {
                        assert!(o.clusters(Channel::File) && !o.two_channel);
                    }
                }

                let m = speaker_options(&settings, ItemType::Meeting, Channel::Mic, "mic", identify);
                assert_eq!(m.is_some(), flag, "meeting: flag {flag}, toggle {identify}");
                if let Some(o) = m {
                    assert!(o.clusters(Channel::Mic) && !o.two_channel);
                }
            }
        }
        // A browser meeting: the mic is You, the remote side is clustered.
        let on = Settings {
            meetings_enabled: true,
            ..Default::default()
        };
        let b = speaker_options(
            &on,
            ItemType::Meeting,
            Channel::Remote,
            "browser:meet.google.com",
            false,
        )
        .unwrap();
        assert!(b.two_channel && b.clusters(Channel::Remote) && !b.clusters(Channel::Mic));
        // System audio + mic (#139): the mic is You, the system side is
        // clustered — and only with the 0.9 flag, whatever the toggle.
        let sys = speaker_options(&on, ItemType::Meeting, Channel::System, "system", false).unwrap();
        assert!(sys.two_channel && sys.clusters(Channel::System) && !sys.clusters(Channel::Mic));
        assert_eq!(
            speaker_options(&Settings::default(), ItemType::Meeting, Channel::System, "system", true),
            None
        );
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
