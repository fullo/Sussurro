//! The "Local (bundled)" LLM profile (Track E, #118): cleanup and recipes
//! with no Ollama or other server installed. Sussurro's own `llama-server`
//! sidecar (#116, the one Qwen3-ASR runs in) serves a small instruct model
//! on a random loopback port, OpenAI-compatible; the profile is local by
//! construction and never external.
//!
//! **Model** — Qwen3 1.7B Q8_0 from `ggml-org/Qwen3-1.7B-GGUF`
//! (Apache-2.0, ggml-org's own conversion of `Qwen/Qwen3-1.7B`), 2.17 GB,
//! downloaded into the models folder when the user picks the profile, via
//! the HuggingFace-tree SHA-256 downloader plus the digest pinned here
//! ([`MODEL_SHA256`], fail-closed). Served with reasoning off (Qwen3 is a
//! hybrid thinking model) and an 8192-token window, the profile's
//! `context_tokens`, which recipes size their chunks for. On our Italian
//! and English dictation samples with Sussurro's cleanup prompt, Q4_K_M
//! (1.28 GB) kept more Italian fillers and repeated words than Q8_0; both
//! answered ~0.2–0.7 s per dictation on an M1 Pro and never replied to a
//! question instead of cleaning it.
//!
//! **One process or two** — two: the LLM runs in its own `llama-server`,
//! next to the Qwen3-ASR one when both are in use. Measured on an M1 Pro
//! (16 GB, Metal, b11146), warm disk cache (RSS also counts the mapped
//! model file and swings with page residency; the physical footprint is
//! the stable figure):
//!
//! | | start → healthy | phys footprint | RSS |
//! |---|---|---|---|
//! | Qwen3-ASR 1.7B Q8 alone | 0.9–2.6 s | 0.9 GB | 2.9 GB |
//! | Qwen3 1.7B Q8_0, 8k ctx, alone | 0.8–2.3 s | 1.0 GB | 3.1 GB |
//! | both, two processes | 0.8–2.0 s each | 1.9 GB | 5.9 GB |
//! | both, one router (`--models-preset`, `--models-max 2`) | router 0.3 s, then 3.1 s and 5.3 s per model | 1.9 GB | 3.8–5.0 GB |
//!
//! The pinned build's router mode shares nothing: it spawns one child
//! `llama-server` per model, so memory and start-up match two instances
//! (plus the router itself). It would add a preset file, a `model` field on
//! every request, grandchild processes (which Linux's `PR_SET_PDEATHSIG` on
//! our direct child does not cover) and one lifecycle for two engines with
//! different idle rules. Two instances keep each owner simple: the
//! transcriber owns the Qwen3-ASR process ([`crate::stt::remote`]), this
//! module owns the LLM one, and each stops on its own idle timer. A cold
//! disk cache made the second model's first load take ~25 s either way.
//!
//! **Lifecycle** — [`BundledLlm`], the same [`Sidecar`] as Qwen3-ASR
//! (loopback only, no shell, lib folder as cwd and on the library path,
//! health check, crash restart with backoff, [`crate::stt::remote::kill_all`]
//! and the exit guard on every way out): started on the first chat request
//! (or pre-warmed when a dictation starts with this profile), a crash
//! during a request is restarted and the request retried once, a live but
//! failing server is left alone, and it stops after [`IDLE_STOP`] without
//! requests. Nothing here downloads: a missing model file fails the
//! request (cleanup then keeps the raw text) until the user downloads it.

use super::LlmProfile;
use crate::settings::CleanupApi;
use crate::stt::remote::{Sidecar, SidecarConfig};
use crate::stt::sidecar::SidecarPaths;
use anyhow::{anyhow, Result};
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

/// Id of the built-in profile. Reserved: `Settings::normalize` moves a
/// user profile that happens to have it.
pub const PROFILE_ID: &str = "bundled";
/// Its display name.
pub const PROFILE_NAME: &str = "Local (bundled)";
/// Where the model comes from (ggml-org's official GGUF conversion).
pub const REPO: &str = "ggml-org/Qwen3-1.7B-GGUF";
/// The model file, in the models folder.
pub const MODEL_FILE: &str = "Qwen3-1.7B-Q8_0.gguf";
/// Its SHA-256 as the HuggingFace tree API published it (2025-04-28
/// upload, repo revision `daeb8e2d`). A download must match both.
pub const MODEL_SHA256: &str = "9860780f3a1fab1f8f909a1b549ea3e62c22d19ab1a492b3a1026b38c5bd3ec3";
/// Its size, for the "downloads ~2.2 GB" note.
pub const MODEL_BYTES: u64 = 2_165_039_200;
/// The model's id on the server (`--alias`) and the profile's `model`.
pub const MODEL_ID: &str = "qwen3-1.7b";
/// How the UI names the model.
pub const MODEL_LABEL: &str = "Qwen3 1.7B";
/// The server's context window (one slot), and the profile's
/// `context_tokens`.
pub const CONTEXT_TOKENS: u32 = 8192;
/// The profile's stored address: loopback, so it reads as local
/// everywhere. The real port is the running sidecar's.
pub const PLACEHOLDER_URL: &str = "http://127.0.0.1";
/// The server stops after this long without a request (as the transcriber).
pub const IDLE_STOP: Duration = Duration::from_secs(15 * 60);
/// A failed request counts as a crash when the process exits within this.
const CRASH_GRACE: Duration = Duration::from_millis(500);

const NO_SIDECAR: &str =
    "this build has no bundled llama-server, which the “Local (bundled)” profile needs — pick another LLM profile";
const NOT_DOWNLOADED: &str =
    "the bundled model is not downloaded yet — choose “Use the bundled model” in Settings → Cleanup or Models";

/// The built-in profile, exactly as `Settings::normalize` keeps it.
pub fn profile() -> LlmProfile {
    LlmProfile {
        context_tokens: CONTEXT_TOKENS,
        bundled: true,
        external: false,
        ..LlmProfile::new(
            PROFILE_ID,
            PROFILE_NAME,
            CleanupApi::Openai,
            PLACEHOLDER_URL,
            "",
            MODEL_ID,
        )
    }
}

/// The model file is in `models_dir`.
pub fn model_exists(models_dir: &Path) -> bool {
    models_dir.join(MODEL_FILE).is_file()
}

/// Download the model if missing (blocking, ~1.3 GB), verified against the
/// HuggingFace tree's SHA-256 and [`MODEL_SHA256`].
pub fn ensure_model(models_dir: &Path) -> Result<PathBuf> {
    crate::stt::models::ensure_hf_file_pinned(models_dir, REPO, MODEL_FILE, MODEL_SHA256)
}

/// The sidecar configuration for the model in `models_dir`.
pub fn sidecar_config(paths: &SidecarPaths, models_dir: &Path, log_file: Option<PathBuf>) -> SidecarConfig {
    let mut cfg = SidecarConfig::chat(
        paths.binary.clone(),
        paths.lib_dir.clone(),
        models_dir.join(MODEL_FILE),
        CONTEXT_TOKENS,
        MODEL_ID,
    );
    cfg.log_file = log_file;
    cfg
}

/// What the UI shows about the bundled model.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct BundledStatus {
    /// This build ships the `llama-server` sidecar.
    pub available: bool,
    /// The model file is in the models folder.
    pub downloaded: bool,
    /// The server is running right now.
    pub running: bool,
    pub model: String,
    pub download_bytes: u64,
}

impl BundledStatus {
    pub fn new(available: bool, downloaded: bool, running: bool) -> Self {
        Self {
            available,
            downloaded,
            running,
            model: MODEL_LABEL.to_string(),
            download_bytes: MODEL_BYTES,
        }
    }
}

type Resolver = Box<dyn Fn() -> Result<SidecarConfig> + Send + Sync>;

/// The bundled LLM's server: started on demand, shared by every chat
/// request, stopped when idle. The app has one ([`global`]); tests make
/// their own with a fake server.
pub struct BundledLlm {
    inner: Mutex<Inner>,
    /// What to run now: resolved on every use, so a changed models folder
    /// restarts the server on its next use.
    resolve: Resolver,
}

#[derive(Default)]
struct Inner {
    sidecar: Option<Sidecar>,
    /// Requests being answered: never idle while > 0.
    in_flight: usize,
    last_used: Option<Instant>,
}

/// Same executable, libraries, model and arguments.
fn same_server(a: &SidecarConfig, b: &SidecarConfig) -> bool {
    a.binary == b.binary && a.lib_dir == b.lib_dir && a.model == b.model && a.role == b.role
}

impl BundledLlm {
    pub fn new(resolve: impl Fn() -> Result<SidecarConfig> + Send + Sync + 'static) -> Self {
        Self {
            inner: Mutex::new(Inner::default()),
            resolve: Box::new(resolve),
        }
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The running server's base URL, starting (or restarting) it first;
    /// counts one request in flight. Concurrent callers wait for the same
    /// start.
    fn acquire(&self) -> Result<String> {
        let cfg = (self.resolve)()?;
        let mut g = self.lock();
        if g.sidecar.as_ref().is_some_and(|s| !same_server(s.config(), &cfg)) {
            g.sidecar = None; // another models folder: stop the old one
        }
        if g.sidecar.is_none() {
            g.sidecar = Some(Sidecar::stopped(cfg)?);
        }
        let sidecar = g.sidecar.as_mut().expect("set above");
        sidecar.ensure_running()?;
        let url = sidecar.base_url();
        g.in_flight += 1;
        g.last_used = Some(Instant::now());
        Ok(url)
    }

    /// Run `request` against the server's base URL. A crash during the
    /// request restarts the server (with the sidecar's backoff) and retries
    /// once; an error from a live server (bad answer, timeout) is returned
    /// as is and the server kept.
    pub fn with_server<T>(&self, request: impl Fn(&str) -> Result<T>) -> Result<T> {
        let mut last = None;
        for _ in 0..2 {
            let url = self.acquire()?;
            let result = request(&url);
            let mut g = self.lock();
            g.in_flight = g.in_flight.saturating_sub(1);
            g.last_used = Some(Instant::now());
            match result {
                Ok(v) => {
                    if let Some(s) = g.sidecar.as_mut() {
                        s.note_ok();
                    }
                    return Ok(v);
                }
                Err(e) => {
                    let crashed = g.sidecar.as_mut().is_some_and(|s| s.crashed_within(CRASH_GRACE));
                    if !crashed {
                        return Err(e);
                    }
                    eprintln!("bundled LLM sidecar crashed during a request: {e:#}; restarting");
                    last = Some(e);
                }
            }
        }
        Err(last
            .unwrap_or_else(|| anyhow!("bundled LLM request failed"))
            .context("the bundled LLM crashed twice on this request"))
    }

    /// Start the server now, without a request (a dictation just started
    /// on this profile: its cleanup then doesn't wait for the model load).
    pub fn warm(&self) -> Result<()> {
        self.acquire()?;
        let mut g = self.lock();
        g.in_flight = g.in_flight.saturating_sub(1);
        Ok(())
    }

    /// Stop the server if nothing used it for `threshold` and no request is
    /// in flight. Never waits: a busy lock (a start in progress) skips the
    /// round. Returns whether it stopped.
    pub fn stop_if_idle(&self, threshold: Duration) -> bool {
        let Ok(mut g) = self.inner.try_lock() else {
            return false;
        };
        if g.in_flight > 0 || g.sidecar.is_none() {
            return false;
        }
        if g.last_used.is_none_or(|t| t.elapsed() < threshold) {
            return false;
        }
        g.sidecar = None; // Drop kills the process
        g.last_used = None;
        true
    }

    /// Stop the server now. Returns whether one was there.
    pub fn stop(&self) -> bool {
        self.lock().sidecar.take().is_some()
    }

    /// The server's process id, while it runs.
    pub fn pid(&self) -> Option<u32> {
        self.lock().sidecar.as_ref().and_then(Sidecar::pid)
    }

    /// The server's port, while it runs.
    pub fn port(&self) -> Option<u16> {
        let g = self.lock();
        g.sidecar
            .as_ref()
            .filter(|s| s.is_running())
            .map(Sidecar::port)
    }

    pub fn is_running(&self) -> bool {
        self.lock().sidecar.as_ref().is_some_and(Sidecar::is_running)
    }
}

/// The app's bundled LLM server.
pub fn global() -> &'static BundledLlm {
    static GLOBAL: OnceLock<BundledLlm> = OnceLock::new();
    GLOBAL.get_or_init(|| BundledLlm::new(app_config))
}

/// Start the app's server in the background if the model is there (a
/// dictation on this profile just started). Errors are left for the
/// request itself to report.
pub fn prewarm() {
    // Never on the caller's thread: a start in progress holds the lock.
    std::thread::spawn(|| {
        if let Err(e) = global().warm() {
            eprintln!("bundled LLM not pre-started: {e:#}");
        }
    });
}

/// The app's sidecar paths and models folder. Takes the settings lock
/// briefly — callers must not hold it.
fn app_paths() -> Result<(SidecarPaths, PathBuf, Option<PathBuf>)> {
    use tauri::Manager;
    let app = crate::app_handle().ok_or_else(|| anyhow!(NO_SIDECAR))?;
    let paths = crate::stt::sidecar::locate(&app).ok_or_else(|| anyhow!(NO_SIDECAR))?;
    let models_dir = {
        let state = app.state::<crate::state::AppState>();
        let settings = state.settings.lock().unwrap_or_else(|e| e.into_inner());
        crate::state::resolve_models_dir(&state.paths, &settings)
    };
    let log = app
        .path()
        .app_log_dir()
        .ok()
        .map(|d| d.join("llama-server-llm.log"));
    Ok((paths, models_dir, log))
}

fn app_config() -> Result<SidecarConfig> {
    let (paths, models_dir, log) = app_paths()?;
    if !model_exists(&models_dir) {
        return Err(anyhow!(NOT_DOWNLOADED));
    }
    Ok(sidecar_config(&paths, &models_dir, log))
}

/// The profile's "models on the server" without starting it: the model's
/// id once the sidecar and the file are there, else why not.
pub fn list_models() -> Result<Vec<String>> {
    app_config().map(|_| vec![MODEL_ID.to_string()])
}

#[cfg(test)]
mod tests;
