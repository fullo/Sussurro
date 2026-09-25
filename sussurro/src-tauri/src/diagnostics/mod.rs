//! Live diagnostics (#101): where the time of a dictation goes, what the
//! speech engine runs on and what it costs in memory and CPU — shown in
//! Settings → Diagnostics while that section is open, and appended to the
//! "Copy diagnostics" report.
//!
//! Timings are recorded into two lock-free rings ([`ring`]): the last
//! dictations and the last long-form segments. Recording costs a few
//! atomic stores; nothing is persisted or sent anywhere, and no text is
//! ever kept — numbers only. Everything else (memory, CPU, backend,
//! sidecars) is read when the panel asks ([`snapshot`]), about once a
//! second while it is open, and never otherwise.

pub mod backend;
pub mod process;
pub mod report;
pub mod ring;

use crate::state::AppState;
use backend::ComputeBackend;
use ring::{Sample, Summary, TimingRing};
use serde::Serialize;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

static DICTATIONS: TimingRing = TimingRing::new();
static SEGMENTS: TimingRing = TimingRing::new();

/// The last hotkey dictations.
pub fn dictations() -> &'static TimingRing {
    &DICTATIONS
}

/// The last long-form engine segments (mic, file, link, meeting).
pub fn segments() -> &'static TimingRing {
    &SEGMENTS
}

fn unix_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

fn ms(d: Duration) -> u64 {
    d.as_millis() as u64
}

// ---- dictation timer -------------------------------------------------------

/// Times one hotkey dictation from Finish to Idle. The pipeline marks each
/// phase as it runs; [`DictationTimer::finish`] records the sample — only
/// when speech was actually transcribed (accidental taps and silence are
/// not dictations).
pub struct DictationTimer {
    finish: Instant,
    sample: Sample,
}

impl Default for DictationTimer {
    fn default() -> Self {
        Self::start()
    }
}

impl DictationTimer {
    /// The user pressed Finish.
    pub fn start() -> Self {
        Self {
            finish: Instant::now(),
            sample: Sample::default(),
        }
    }

    /// Length of the recording, from its 16 kHz sample count.
    pub fn audio_samples(&mut self, n: usize) {
        self.sample.audio_ms = Some(n as u64 / 16);
    }

    pub fn load(&mut self, d: Duration) {
        self.sample.load_ms = Some(ms(d));
    }

    pub fn stt(&mut self, d: Duration) {
        self.sample.stt_ms = Some(ms(d));
    }

    /// Adds up: streaming dictations clean in more than one call.
    pub fn cleanup(&mut self, d: Duration) {
        self.sample.cleanup_ms = Some(self.sample.cleanup_ms.unwrap_or(0) + ms(d));
    }

    pub fn paste(&mut self, d: Duration) {
        self.sample.paste_ms = Some(self.sample.paste_ms.unwrap_or(0) + ms(d));
    }

    /// Time `f` as a cleanup phase — counted only if it did call the
    /// cleanup server (cleanup off, or a privacy-gated external profile,
    /// makes no call and returns at once).
    pub fn time_cleanup<T>(&mut self, f: impl FnOnce() -> T) -> T {
        let calls = cleanup_calls();
        let t = Instant::now();
        let v = f();
        if cleanup_calls() != calls {
            self.cleanup(t.elapsed());
        }
        v
    }

    /// Time `f` as a paste phase.
    pub fn time_paste<T>(&mut self, f: impl FnOnce() -> T) -> T {
        let t = Instant::now();
        let v = f();
        self.paste(t.elapsed());
        v
    }

    /// The dictation is over (about to go Idle, or failed). Returns the
    /// sample recorded, if any.
    pub fn finish(mut self, ok: bool) -> Option<Sample> {
        self.sample.stt_ms?;
        self.sample.at_ms = unix_ms();
        self.sample.total_ms = Some(ms(self.finish.elapsed()));
        self.sample.failed = !ok;
        DICTATIONS.push(self.sample);
        Some(self.sample)
    }
}

/// One long-form segment went through STT (and cleanup).
pub fn note_segment(audio_samples: usize, stt: Duration, cleanup: Option<Duration>, failed: bool) {
    SEGMENTS.push(Sample {
        at_ms: unix_ms(),
        audio_ms: Some(audio_samples as u64 / 16),
        stt_ms: Some(ms(stt)),
        cleanup_ms: cleanup.map(ms),
        total_ms: Some(ms(stt + cleanup.unwrap_or_default())),
        failed,
        ..Default::default()
    });
}

// ---- cleanup endpoint ------------------------------------------------------

static CLEANUP_MS: AtomicU64 = AtomicU64::new(u64::MAX);
static CLEANUP_AT: AtomicU64 = AtomicU64::new(0);
static CLEANUP_OK: AtomicBool = AtomicBool::new(false);
static CLEANUP_CALLS: AtomicU64 = AtomicU64::new(0);

/// The cleanup profile's server answered (or failed) after `elapsed`.
pub fn note_cleanup_call(elapsed: Duration, ok: bool) {
    CLEANUP_CALLS.fetch_add(1, Ordering::Relaxed);
    CLEANUP_OK.store(ok, Ordering::Relaxed);
    CLEANUP_AT.store(unix_ms(), Ordering::Relaxed);
    CLEANUP_MS.store(ms(elapsed), Ordering::Release);
}

/// Cleanup server calls so far (a phase that made none did not clean).
pub fn cleanup_calls() -> u64 {
    CLEANUP_CALLS.load(Ordering::Relaxed)
}

/// The last cleanup call.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct LastCall {
    pub ms: u64,
    pub ok: bool,
    /// Seconds since it ended.
    pub age_s: u64,
}

fn last_cleanup_call() -> Option<LastCall> {
    let ms = CLEANUP_MS.load(Ordering::Acquire);
    (ms != u64::MAX).then(|| LastCall {
        ms,
        ok: CLEANUP_OK.load(Ordering::Relaxed),
        age_s: unix_ms().saturating_sub(CLEANUP_AT.load(Ordering::Relaxed)) / 1000,
    })
}

// ---- long-form backlog -----------------------------------------------------

/// How far a running long-form session is behind its source.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct SessionBacklog {
    pub session_id: u64,
    pub backlog_s: f64,
    pub processed_s: f64,
    pub queue_len: usize,
    pub segments_done: usize,
}

static BACKLOG: Mutex<BTreeMap<u64, SessionBacklog>> = Mutex::new(BTreeMap::new());

/// The engine emitted a progress event.
pub fn note_progress(p: &crate::engine::ProgressPayload) {
    if let Ok(mut b) = BACKLOG.lock() {
        b.insert(
            p.session_id,
            SessionBacklog {
                session_id: p.session_id,
                backlog_s: p.backlog_s,
                processed_s: p.processed_s,
                queue_len: p.queue_len,
                segments_done: p.segments_done,
            },
        );
    }
}

/// The session ended.
pub fn forget_session(id: u64) {
    if let Ok(mut b) = BACKLOG.lock() {
        b.remove(&id);
    }
}

fn backlog() -> Vec<SessionBacklog> {
    BACKLOG
        .lock()
        .map(|b| b.values().copied().collect())
        .unwrap_or_default()
}

// ---- snapshot --------------------------------------------------------------

/// A `llama-server` sidecar (Qwen3-ASR or the bundled LLM).
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct SidecarStatus {
    /// This build ships the sidecar.
    pub available: bool,
    pub running: bool,
    /// The sidecar process's memory, bytes.
    pub memory_bytes: Option<u64>,
    pub backend: Option<ComputeBackend>,
    /// Crashes since it last answered.
    pub failures: u32,
    /// Size of the model file(s) on disk, bytes.
    pub model_file_bytes: Option<u64>,
}

/// The speech-to-text engine.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SttStatus {
    /// `whisper` | `parakeet` | `qwen3_asr`.
    pub engine: String,
    pub model: String,
    /// `not_loaded` | `loaded` | `busy` (transcribing or loading now).
    pub state: String,
    pub backend: Option<ComputeBackend>,
    /// Size of the model file(s) on disk, bytes.
    pub model_file_bytes: Option<u64>,
    /// Qwen3-ASR's sidecar.
    pub sidecar: Option<SidecarStatus>,
}

/// The cleanup profile.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CleanupStatus {
    /// Cleanup (or translation) runs on dictations.
    pub active: bool,
    pub profile: String,
    /// `ollama` | `openai` | `bundled`.
    pub api: String,
    /// Server address, credentials and query removed; empty for the
    /// bundled profile.
    pub endpoint: String,
    pub external: bool,
    pub last_call: Option<LastCall>,
}

/// This process.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ProcessStatus {
    pub memory_bytes: Option<u64>,
    /// Share of the whole machine since the previous snapshot.
    pub cpu_percent: Option<f64>,
    pub cpus: usize,
}

/// Everything the Diagnostics panel shows.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Snapshot {
    pub version: String,
    pub os: String,
    /// Oldest first.
    pub dictations: Vec<Sample>,
    pub dictation_summary: Summary,
    pub segments: Vec<Sample>,
    pub segment_summary: Summary,
    pub stt: SttStatus,
    pub cleanup: CleanupStatus,
    pub bundled_llm: SidecarStatus,
    pub process: ProcessStatus,
    pub backlog: Vec<SessionBacklog>,
    pub recording: bool,
}

static CPU: process::CpuMeter = process::CpuMeter::new();

/// The Qwen3-ASR sidecar's pid when last seen (0 = none), for snapshots
/// taken while a transcription holds the model.
static QWEN_PID: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

fn file_size(p: &std::path::Path) -> Option<u64> {
    std::fs::metadata(p)
        .ok()
        .filter(|m| m.is_file())
        .map(|m| m.len())
}

/// Total size of the files directly in `dir`.
fn dir_size(dir: &std::path::Path) -> Option<u64> {
    let total: u64 = std::fs::read_dir(dir)
        .ok()?
        .filter_map(|e| e.ok()?.metadata().ok())
        .filter(|m| m.is_file())
        .map(|m| m.len())
        .sum();
    (total > 0).then_some(total)
}

fn sum_sizes(paths: &[std::path::PathBuf]) -> Option<u64> {
    let sizes: Vec<u64> = paths.iter().filter_map(|p| file_size(p)).collect();
    (!sizes.is_empty()).then(|| sizes.iter().sum())
}

/// The tail of a sidecar log, parsed for its backend. Reads at most the
/// last 256 KiB.
fn sidecar_backend(log: Option<&std::path::Path>) -> Option<ComputeBackend> {
    use std::io::{Read, Seek, SeekFrom};
    let mut f = std::fs::File::open(log?).ok()?;
    let len = f.metadata().ok()?.len();
    let from = len.saturating_sub(256 * 1024);
    f.seek(SeekFrom::Start(from)).ok()?;
    let mut buf = Vec::new();
    f.read_to_end(&mut buf).ok()?;
    backend::parse_llama_log(&String::from_utf8_lossy(&buf))
}

fn app_log(app: &tauri::AppHandle, name: &str) -> Option<std::path::PathBuf> {
    use tauri::Manager;
    app.path().app_log_dir().ok().map(|d| d.join(name))
}

/// Read everything the panel shows. Never waits for a busy model: a
/// transcriber held by a transcription reads as `busy`.
pub fn snapshot(app: &tauri::AppHandle, state: &AppState) -> Snapshot {
    use crate::settings::SttEngine;
    let settings = state.settings.lock().unwrap().clone();
    let models_dir = crate::state::resolve_models_dir(&state.paths, &settings);
    let sidecar_available = crate::stt::sidecar::sidecar_available(app);

    let (engine, model, model_file_bytes) = match settings.engine {
        SttEngine::Whisper => (
            "whisper",
            settings.whisper_model.clone(),
            crate::stt::models::resolve_model_path(&models_dir, &settings.whisper_model)
                .ok()
                .and_then(|p| file_size(&p)),
        ),
        SttEngine::Parakeet => (
            "parakeet",
            crate::stt::parakeet::PARAKEET_DIR.to_string(),
            dir_size(&models_dir.join(crate::stt::parakeet::PARAKEET_DIR)),
        ),
        SttEngine::Qwen3Asr => (
            "qwen3_asr",
            crate::stt::remote::QWEN3_ASR_LABEL.to_string(),
            sum_sizes(
                &crate::stt::remote::QWEN3_ASR_FILES
                    .iter()
                    .map(|f| models_dir.join(f))
                    .collect::<Vec<_>>(),
            ),
        ),
    };

    // Loaded, and the Qwen3-ASR sidecar's process — without waiting. While
    // a transcription holds the model, the sidecar is the one seen last.
    let (stt_state, qwen_pid, qwen_running, qwen_failures) = match state.transcriber.try_lock() {
        Ok(guard) => {
            let seen = match guard.as_ref() {
                Some(loaded) => match &loaded.model {
                    crate::stt::AnyTranscriber::Remote(r) => {
                        let s = r.sidecar();
                        ("loaded", s.pid(), s.is_running(), s.failures())
                    }
                    _ => ("loaded", None, false, 0),
                },
                None => ("not_loaded", None, false, 0),
            };
            QWEN_PID.store(seen.1.unwrap_or(0), Ordering::Relaxed);
            seen
        }
        Err(_) => {
            let pid = QWEN_PID.load(Ordering::Relaxed);
            ("busy", (pid != 0).then_some(pid), false, 0)
        }
    };
    let stt_backend = match settings.engine {
        SttEngine::Whisper => {
            Some(backend::whisper_backend().unwrap_or_else(backend::whisper_build_guess))
        }
        SttEngine::Parakeet => Some(ComputeBackend {
            kind: "CPU".into(),
            device: None,
            source: "engine".into(),
            note: Some("ONNX Runtime".into()),
        }),
        SttEngine::Qwen3Asr => sidecar_backend(app_log(app, "llama-server.log").as_deref()),
    };
    let sidecar = (settings.engine == SttEngine::Qwen3Asr).then(|| SidecarStatus {
        available: sidecar_available,
        // Busy = transcribing on the sidecar, so it runs.
        running: qwen_running || stt_state == "busy",
        memory_bytes: qwen_pid.and_then(process::memory_of),
        backend: stt_backend.clone(),
        failures: qwen_failures,
        model_file_bytes,
    });

    let profile = settings.cleanup_llm();
    let cleanup = CleanupStatus {
        active: settings.cleanup_active(),
        profile: profile.name.clone(),
        api: if profile.bundled {
            "bundled".into()
        } else {
            match profile.api {
                crate::settings::CleanupApi::Ollama => "ollama".into(),
                crate::settings::CleanupApi::Openai => "openai".into(),
            }
        },
        endpoint: if profile.bundled {
            String::new()
        } else {
            crate::secrets::redact_url(&profile.base_url)
        },
        external: profile.external,
        last_call: last_cleanup_call(),
    };

    let llm = crate::llm::bundled::global();
    let llm_pid = llm.try_pid();
    let bundled_llm = SidecarStatus {
        available: sidecar_available,
        running: llm_pid.is_some(),
        memory_bytes: llm_pid.and_then(process::memory_of),
        backend: if llm_pid.is_some() {
            sidecar_backend(app_log(app, "llama-server-llm.log").as_deref())
        } else {
            None
        },
        failures: 0,
        model_file_bytes: file_size(&models_dir.join(crate::llm::bundled::MODEL_FILE)),
    };

    let cpus = process::cpu_count();
    let cpu_percent = process::self_cpu_time().and_then(|t| CPU.sample(Instant::now(), t, cpus));
    let dictations = DICTATIONS.samples();
    let segments = SEGMENTS.samples();
    Snapshot {
        version: env!("CARGO_PKG_VERSION").to_string(),
        os: format!("{} {}", std::env::consts::OS, std::env::consts::ARCH),
        dictation_summary: ring::summarize(&dictations),
        dictations,
        segment_summary: ring::summarize(&segments),
        segments,
        stt: SttStatus {
            engine: engine.to_string(),
            model,
            state: stt_state.to_string(),
            backend: stt_backend,
            model_file_bytes,
            sidecar,
        },
        cleanup,
        bundled_llm,
        process: ProcessStatus {
            memory_bytes: process::self_memory(),
            cpu_percent,
            cpus,
        },
        backlog: backlog(),
        recording: state
            .recorder
            .try_lock()
            .map(|r| r.is_recording())
            .unwrap_or(false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_dictation_without_transcription_is_not_recorded() {
        let mut t = DictationTimer::start();
        t.audio_samples(1_000);
        assert_eq!(t.finish(true), None);
    }

    #[test]
    fn dictation_timer_records_its_phases() {
        let mut t = DictationTimer::start();
        t.audio_samples(48_000);
        t.load(Duration::from_millis(5));
        t.stt(Duration::from_millis(800));
        t.cleanup(Duration::from_millis(300));
        t.cleanup(Duration::from_millis(100));
        // No server call inside: not a cleanup.
        assert_eq!(t.time_cleanup(|| 1), 1);
        let v = t.time_paste(|| 7);
        assert_eq!(v, 7);
        let s = t.finish(true).unwrap();
        assert_eq!(s.audio_ms, Some(3_000));
        assert_eq!(s.load_ms, Some(5));
        assert_eq!(s.stt_ms, Some(800));
        assert_eq!(s.cleanup_ms, Some(400));
        assert!(s.paste_ms.is_some());
        assert!(s.total_ms.is_some());
        assert!(!s.failed);
    }

    #[test]
    fn time_cleanup_counts_only_calls_to_the_server() {
        let mut t = DictationTimer::start();
        t.stt(Duration::from_millis(1));
        t.time_cleanup(|| note_cleanup_call(Duration::from_millis(250), true));
        let s = t.finish(true).unwrap();
        assert!(s.cleanup_ms.is_some());
        // Other tests may have called since; there is a last call.
        assert!(last_cleanup_call().is_some());
    }

    #[test]
    fn backlog_is_kept_per_session_until_it_ends() {
        let p = crate::engine::ProgressPayload {
            session_id: 987_654,
            processed_s: 10.0,
            ingested_s: 14.0,
            total_s: 14.0,
            backlog_s: 4.0,
            queue_len: 1,
            segments_done: 3,
        };
        note_progress(&p);
        assert!(backlog()
            .iter()
            .any(|b| b.session_id == 987_654 && b.backlog_s == 4.0));
        forget_session(987_654);
        assert!(!backlog().iter().any(|b| b.session_id == 987_654));
    }
}
