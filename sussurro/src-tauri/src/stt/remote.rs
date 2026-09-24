//! Qwen3-ASR through the bundled `llama-server` sidecar (plan §8, E9, #117).
//!
//! whisper-rs links its own ggml, so any other ggml-based engine runs in a
//! separate process: the pinned upstream `llama-server` that #116 bundles
//! ([`super::sidecar`]). This module owns that process and talks to it.
//!
//! **Lifecycle** — [`Sidecar`]:
//! - started on demand, when the transcriber is loaded (first dictation or
//!   long-form segment with the Qwen3-ASR engine), on a random free port
//!   bound to `127.0.0.1` only (`--host 127.0.0.1`), with the model and its
//!   audio encoder (`--mmproj`);
//! - spawned directly, never through a shell: the executable path and each
//!   argument are separate `Command` arguments ([`command`]);
//! - the lib folder is the working directory and is prepended to the
//!   platform library path ([`library_path_var`]), as #116's layout needs;
//! - health-checked (`GET /health` answers 200 once the model is loaded)
//!   before it is used;
//! - restarted when it crashed, with an exponential [`backoff`]: a short
//!   wait is slept through, a long one fails the request fast instead of
//!   holding the transcriber for seconds;
//! - stopped with the transcriber that owns it: the idle unload
//!   (`pipeline::unload_transcriber_if_idle`, same "in use" rules as every
//!   engine), an engine/model change, and `Drop`;
//! - never left behind on exit: every live child is registered and
//!   [`kill_all`] runs on the Tauri exit event and when Tauri clears its
//!   resource table ([`ExitGuard`] — which also covers the updater's
//!   restart and its Windows install path, both of which exit without the
//!   exit event). If Sussurro itself is killed, the OS takes the sidecar
//!   down on Windows (a kill-on-close job object) and Linux
//!   (`PR_SET_PDEATHSIG`); macOS has no equivalent, so only a crash or a
//!   force-quit of Sussurro can leave it running there.
//!
//! The process side ([`Sidecar`], [`SidecarConfig`], [`kill_all`]) also runs
//! the chat model of the "Local (bundled)" LLM profile (#118, a second,
//! separate process — see `crate::llm::bundled` for why not one shared
//! server): only the arguments differ ([`SidecarRole`]).
//!
//! **Client** — one multipart `POST /v1/audio/transcriptions` per ≤ 30 s
//! piece of audio, sent as a 16 kHz mono WAV. Qwen3-ASR has no dictionary
//! prompt and ignores the language hint (it detects the language itself):
//! dictionary words only reach the cleanup prompt. Longer input (a long
//! hotkey dictation) is split at pauses with #194's splitter
//! ([`input_pieces`]), because Qwen3-ASR returns empty output on long
//! inputs (llama.cpp #21847). The long-form engine's segments are already
//! ≤ 30 s and go as they are.
//!
//! **Output** — the model prefixes every transcript with
//! `language <Name><asr_text>` (llama.cpp #26749); [`sanitize`] strips it
//! and any other marker, and reads the detected language.

use anyhow::{bail, Context, Result};
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, Weak};
use std::time::{Duration, Instant};

/// The one Qwen3-ASR size that passed the gate (#109, #152): 1.7B Q8.
pub const QWEN3_ASR_REPO: &str = "ggml-org/Qwen3-ASR-1.7B-GGUF";
/// The language model (2.2 GB).
pub const QWEN3_ASR_MODEL: &str = "Qwen3-ASR-1.7B-Q8_0.gguf";
/// Its audio encoder, loaded with `--mmproj` (356 MB).
pub const QWEN3_ASR_MMPROJ: &str = "mmproj-Qwen3-ASR-1.7B-Q8_0.gguf";
/// Both files the engine needs, downloaded into the models folder.
pub const QWEN3_ASR_FILES: [&str; 2] = [QWEN3_ASR_MODEL, QWEN3_ASR_MMPROJ];
/// How items and diagnostics name the engine.
pub const QWEN3_ASR_LABEL: &str = "qwen3-asr-1.7b-q8";

const RATE: usize = 16_000;
/// Longest audio sent in one request (llama.cpp #21847).
pub const MAX_INPUT_SAMPLES: usize = 30 * RATE;
/// Waits up to this long are slept through before a restart; longer ones
/// fail the request instead.
const MAX_INLINE_BACKOFF: Duration = Duration::from_secs(2);

// ---------------------------------------------------------------- output --

/// A sanitised transcript and the language the model detected.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AsrOutput {
    pub text: String,
    /// ISO 639-1 code, `None` when the model said nothing or a name we
    /// don't map.
    pub language: Option<String>,
}

const ASR_OPEN: &str = "<asr_text>";
const ASR_CLOSE: &str = "</asr_text>";

fn special_tokens() -> &'static regex::Regex {
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    // `<|im_end|>`, `<|endoftext|>`, `<|audio_pad|>`…
    RE.get_or_init(|| regex::Regex::new(r"<\|[^<>|]*\|>").expect("valid regex"))
}

fn language_prefix() -> &'static regex::Regex {
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    RE.get_or_init(|| regex::Regex::new(r"(?i)language\s+([A-Za-z]+)\s*$").expect("valid regex"))
}

/// Strip Qwen3-ASR's `language <Name><asr_text>` prefix (llama.cpp #26749)
/// and every other marker, and read the detected language. The text is
/// what follows the *last* `<asr_text>`; an output that is only
/// `language <Name>` (silence) is empty. Pure.
pub fn sanitize(raw: &str) -> AsrOutput {
    let raw = raw.trim();
    let (head, body) = match raw.rfind(ASR_OPEN) {
        Some(i) => (&raw[..i], &raw[i + ASR_OPEN.len()..]),
        None if is_bare_language(raw) => (raw, ""),
        None => ("", raw),
    };
    let head = special_tokens().replace_all(head, " ");
    let language = language_prefix()
        .captures(head.trim_end())
        .and_then(|c| language_code(&c[1]))
        .map(str::to_string);
    let body = special_tokens().replace_all(body, " ");
    let text = body
        .replace(ASR_CLOSE, " ")
        .replace(ASR_OPEN, " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    AsrOutput { text, language }
}

/// `language English` and nothing else: the model's answer to silence.
fn is_bare_language(raw: &str) -> bool {
    let mut words = raw.split_whitespace();
    matches!(
        (words.next(), words.next(), words.next()),
        (Some(l), Some(name), None)
            if l.eq_ignore_ascii_case("language") && name.chars().all(|c| c.is_ascii_alphabetic())
    )
}

/// The ISO 639-1 code of a language name as Qwen3-ASR spells it. Only the
/// languages Sussurro's picker offers plus the other common ones; `None`
/// otherwise (including the model's own `None`).
pub fn language_code(name: &str) -> Option<&'static str> {
    let code = match name.to_ascii_lowercase().as_str() {
        "english" => "en",
        "italian" => "it",
        "french" => "fr",
        "german" => "de",
        "spanish" => "es",
        "portuguese" => "pt",
        "dutch" => "nl",
        "polish" => "pl",
        "russian" => "ru",
        "ukrainian" => "uk",
        "czech" => "cs",
        "swedish" => "sv",
        "danish" => "da",
        "finnish" => "fi",
        "greek" => "el",
        "romanian" => "ro",
        "hungarian" => "hu",
        "turkish" => "tr",
        "arabic" => "ar",
        "hindi" => "hi",
        "chinese" | "cantonese" => "zh",
        "japanese" => "ja",
        "korean" => "ko",
        "vietnamese" => "vi",
        "indonesian" => "id",
        "thai" => "th",
        _ => return None,
    };
    Some(code)
}

// ----------------------------------------------------------------- input --

/// How input longer than [`MAX_INPUT_SAMPLES`] is cut, with the #194
/// pause splitter ([`super::pauses`]): a piece closes at the first pause
/// (≥ 300 ms, relative to the audio's own loudness) after 20 s, and one
/// without a pause is cut at its quietest frame of the last 8 s before
/// 28 s — so every piece stays under the 30 s limit. No tail re-decode:
/// that is a Parakeet workaround.
pub const QWEN3_ASR_SPLIT: super::pauses::PauseSplit = super::pauses::PauseSplit {
    soft_max_ms: 20_000,
    min_pause_ms: 300,
    hard_max_ms: 28_000,
    cap_search_ms: 8_000,
    tail_grace_ms: 0,
    tail_speech_ms: 0,
};

/// The request pieces of `samples`: the whole input up to 30 s (the
/// long-form engine's segments), else [`QWEN3_ASR_SPLIT`]'s cuts. The
/// ranges tile the input. Pure.
// One range for "send it whole" is the intended result.
#[allow(clippy::single_range_in_vec_init)]
pub fn input_pieces(samples: &[f32]) -> Vec<std::ops::Range<usize>> {
    if samples.len() <= MAX_INPUT_SAMPLES {
        return vec![0..samples.len()];
    }
    super::pauses::split_ranges(samples, &QWEN3_ASR_SPLIT)
}

/// A `multipart/form-data` body: the text `fields`, then `wav` as the
/// `file` part. Pure.
pub fn multipart_body(
    boundary: &str,
    fields: &[(&str, &str)],
    file_name: &str,
    wav: &[u8],
) -> Vec<u8> {
    let mut body = Vec::with_capacity(wav.len() + 512);
    for (name, value) in fields {
        body.extend_from_slice(
            format!("--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n")
                .as_bytes(),
        );
    }
    body.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{file_name}\"\r\nContent-Type: audio/wav\r\n\r\n"
        )
        .as_bytes(),
    );
    body.extend_from_slice(wav);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    body
}

fn new_boundary() -> Result<String> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).map_err(|e| anyhow::anyhow!("no OS randomness: {e}"))?;
    Ok(format!(
        "sussurro-{}",
        bytes.iter().map(|b| format!("{b:02x}")).collect::<String>()
    ))
}

// --------------------------------------------------------------- process --

/// What a sidecar serves. The process lifecycle is the same for both; only
/// the server arguments and the name in messages differ.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SidecarRole {
    /// Qwen3-ASR: the model plus its audio encoder ([`SidecarConfig::mmproj`]),
    /// `/v1/audio/transcriptions` (#117).
    Asr,
    /// A chat model for the "Local (bundled)" LLM profile (#118),
    /// `/v1/chat/completions`, listed as `alias` with a `ctx_tokens` context
    /// window. See `crate::llm::bundled`.
    Chat { ctx_tokens: u32, alias: String },
}

/// What to run and how. [`SidecarConfig::new`] (Qwen3-ASR) and
/// [`SidecarConfig::chat`] have the app's defaults.
#[derive(Debug, Clone)]
pub struct SidecarConfig {
    pub binary: PathBuf,
    pub lib_dir: PathBuf,
    pub model: PathBuf,
    /// The audio encoder; empty and unused for [`SidecarRole::Chat`].
    pub mmproj: PathBuf,
    pub role: SidecarRole,
    /// stdout + stderr of the server; `None` discards them.
    pub log_file: Option<PathBuf>,
    /// Spawn → `/health` 200 (model load; 2–3 s warm on an M1, much more
    /// from a cold disk or on CPU).
    pub start_timeout: Duration,
    /// One transcription request.
    pub request_timeout: Duration,
    /// Restart delay after the second consecutive failure; doubles with
    /// each further one ([`backoff`]).
    pub backoff_base: Duration,
    /// Arguments placed before the server's own. Empty in the app; the
    /// tests run the test binary itself as a fake server through it.
    pub prefix_args: Vec<OsString>,
}

impl SidecarConfig {
    pub fn new(binary: PathBuf, lib_dir: PathBuf, model: PathBuf, mmproj: PathBuf) -> Self {
        Self {
            binary,
            lib_dir,
            model,
            mmproj,
            role: SidecarRole::Asr,
            log_file: None,
            start_timeout: Duration::from_secs(180),
            request_timeout: Duration::from_secs(180),
            backoff_base: Duration::from_millis(500),
            prefix_args: Vec::new(),
        }
    }

    /// A chat-model sidecar (#118): `model` served as `alias` with a
    /// `ctx_tokens` window. Same timeouts and backoff as Qwen3-ASR.
    pub fn chat(
        binary: PathBuf,
        lib_dir: PathBuf,
        model: PathBuf,
        ctx_tokens: u32,
        alias: &str,
    ) -> Self {
        Self {
            role: SidecarRole::Chat {
                ctx_tokens,
                alias: alias.to_string(),
            },
            ..Self::new(binary, lib_dir, model, PathBuf::new())
        }
    }

    /// The server's own arguments for `port`: [`server_args`] or
    /// [`chat_server_args`]. Pure.
    pub fn server_args(&self, port: u16) -> Vec<OsString> {
        match &self.role {
            SidecarRole::Asr => server_args(&self.model, &self.mmproj, port),
            SidecarRole::Chat { ctx_tokens, alias } => {
                chat_server_args(&self.model, alias, *ctx_tokens, port)
            }
        }
    }

    /// How messages name this sidecar.
    pub fn label(&self) -> &'static str {
        match self.role {
            SidecarRole::Asr => "Qwen3-ASR",
            SidecarRole::Chat { .. } => "bundled LLM",
        }
    }
}

/// `llama-server` arguments for Qwen3-ASR on `port`, as benchmarked in
/// #109: all layers on the GPU (ignored by the CPU build), one slot with a
/// 4096-token context (a 30 s chunk is ~370 input tokens), no prompt cache,
/// no web UI, loopback only. Pure.
pub fn server_args(model: &Path, mmproj: &Path, port: u16) -> Vec<OsString> {
    let mut args: Vec<OsString> = Vec::new();
    let mut push = |a: &OsStr| args.push(a.to_os_string());
    push("-m".as_ref());
    push(model.as_os_str());
    push("--mmproj".as_ref());
    push(mmproj.as_os_str());
    for a in [
        "--host",
        "127.0.0.1",
        "--port",
        &port.to_string(),
        "-ngl",
        "99",
        "-c",
        "4096",
        "-np",
        "1",
        "--cache-ram",
        "0",
        "--no-webui",
    ] {
        push(a.as_ref());
    }
    args
}

/// `llama-server` arguments for a chat model on `port` (#118): the same
/// loopback-only, all-layers-on-GPU, one-slot, no-prompt-cache, no-web-UI
/// shape as [`server_args`], plus the model's id in `/v1/models`
/// (`--alias`) and reasoning off: Qwen3 would otherwise think before every
/// cleanup, which costs seconds for nothing. `ctx_tokens` is the whole
/// window (one slot). Pure.
pub fn chat_server_args(model: &Path, alias: &str, ctx_tokens: u32, port: u16) -> Vec<OsString> {
    let mut args: Vec<OsString> = vec!["-m".into(), model.as_os_str().to_os_string()];
    for a in [
        "--alias",
        alias,
        "--host",
        "127.0.0.1",
        "--port",
        &port.to_string(),
        "-ngl",
        "99",
        "-c",
        &ctx_tokens.to_string(),
        "-np",
        "1",
        "--cache-ram",
        "0",
        "--reasoning",
        "off",
        "--no-webui",
    ] {
        args.push(a.into());
    }
    args
}

/// The variable the dynamic loader searches on `os`
/// (`std::env::consts::OS`). Pure.
pub fn library_path_var(os: &str) -> &'static str {
    match os {
        "macos" => "DYLD_LIBRARY_PATH",
        "windows" => "PATH",
        _ => "LD_LIBRARY_PATH",
    }
}

/// `lib_dir` prepended to the current value of the library path on `os`.
/// Pure.
pub fn library_path_value(os: &str, lib_dir: &Path, current: Option<&OsStr>) -> OsString {
    let sep = if os == "windows" { ";" } else { ":" };
    let mut value = lib_dir.as_os_str().to_os_string();
    if let Some(cur) = current.filter(|c| !c.is_empty()) {
        value.push(sep);
        value.push(cur);
    }
    value
}

/// The sidecar command for `port`: the executable itself (no shell), its
/// arguments, the lib folder as working directory and first on the library
/// path. Stdio is set by the caller.
pub fn command(cfg: &SidecarConfig, port: u16) -> Command {
    let os = std::env::consts::OS;
    let var = library_path_var(os);
    let mut cmd = Command::new(&cfg.binary);
    cmd.args(&cfg.prefix_args)
        .args(cfg.server_args(port))
        .current_dir(&cfg.lib_dir)
        .env(
            var,
            library_path_value(os, &cfg.lib_dir, std::env::var_os(var).as_deref()),
        )
        .stdin(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

/// Delay before restarting after `failures` consecutive failures: none
/// after the first (usually a one-off), then `base`, doubling, capped at
/// 60 s. Pure.
pub fn backoff(base: Duration, failures: u32) -> Duration {
    if failures <= 1 {
        return Duration::ZERO;
    }
    let exp = (failures - 2).min(16);
    base.saturating_mul(1u32 << exp)
        .min(Duration::from_secs(60))
}

fn free_loopback_port() -> Result<u16> {
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0))
        .context("no free loopback port for the sidecar")?;
    Ok(listener.local_addr()?.port())
}

type SharedChild = Arc<Mutex<Option<Child>>>;
type Registry = Mutex<Vec<Weak<Mutex<Option<Child>>>>>;

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Every sidecar process this app started and has not stopped yet.
fn registry() -> &'static Registry {
    static LIVE: OnceLock<Registry> = OnceLock::new();
    LIVE.get_or_init(|| Mutex::new(Vec::new()))
}

fn register(child: &SharedChild) {
    let mut live = lock(registry());
    live.retain(|w| w.strong_count() > 0);
    live.push(Arc::downgrade(child));
}

/// Kill every sidecar still running (app exit). Returns how many it killed.
/// Never waits for the transcriber: an in-flight request just fails.
pub fn kill_all() -> usize {
    // Entries stay: a sidecar used again after this restarts into the same
    // slot, and must still be found by the next call.
    let live: Vec<SharedChild> = lock(registry()).iter().filter_map(Weak::upgrade).collect();
    let mut killed = 0;
    for shared in live {
        if let Some(mut child) = lock(&shared).take() {
            let _ = child.kill();
            let _ = child.wait();
            killed += 1;
        }
    }
    killed
}

/// Kills the sidecars when Tauri drops it. Tauri clears the app's resource
/// table in `cleanup_before_exit`, which runs on every way out: the exit
/// event, `restart()` (the updater's relaunch) and the updater's Windows
/// install, which then calls `process::exit` without an exit event.
pub struct ExitGuard;

impl Drop for ExitGuard {
    fn drop(&mut self) {
        let n = kill_all();
        if n > 0 {
            eprintln!("stopped {n} llama-server sidecar(s) on exit");
        }
    }
}

impl tauri::Resource for ExitGuard {}

/// Spawn `cmd`. On Linux the child gets `PR_SET_PDEATHSIG`, so the kernel
/// kills it if Sussurro dies. The signal fires when the *thread* that
/// spawned it ends, and transcriptions run on short-lived threads, so the
/// spawn happens on one dedicated thread that lives as long as the app.
#[cfg(target_os = "linux")]
fn spawn_child(mut cmd: Command) -> std::io::Result<Child> {
    use std::os::unix::process::CommandExt;
    use std::sync::mpsc;
    type Job = (Command, mpsc::Sender<std::io::Result<Child>>);
    static SPAWNER: OnceLock<Mutex<mpsc::Sender<Job>>> = OnceLock::new();

    let parent = std::process::id() as libc::pid_t;
    // SAFETY: prctl and getppid are async-signal-safe, as pre_exec needs.
    unsafe {
        cmd.pre_exec(move || {
            if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            // The parent died between fork and prctl: don't outlive it.
            if libc::getppid() != parent {
                return Err(std::io::Error::other("Sussurro exited"));
            }
            Ok(())
        });
    }
    let tx = SPAWNER.get_or_init(|| {
        let (tx, rx) = mpsc::channel::<Job>();
        std::thread::Builder::new()
            .name("sidecar-spawner".into())
            .spawn(move || {
                for (mut cmd, reply) in rx {
                    let _ = reply.send(cmd.spawn());
                }
            })
            .expect("start the sidecar spawner thread");
        Mutex::new(tx)
    });
    let (reply_tx, reply_rx) = mpsc::channel();
    let gone = || std::io::Error::other("the sidecar spawner thread stopped");
    lock(tx).send((cmd, reply_tx)).map_err(|_| gone())?;
    reply_rx.recv().map_err(|_| gone())?
}

#[cfg(not(target_os = "linux"))]
fn spawn_child(mut cmd: Command) -> std::io::Result<Child> {
    cmd.spawn()
}

/// Put the child in a job object that kills its processes when its last
/// handle closes — which the OS does when Sussurro exits, however it
/// exits. Best effort: a failure is logged and the exit hooks remain.
#[cfg(windows)]
fn kill_with_parent(child: &Child) {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
        SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };
    // The job handle, as an integer (raw handles are not Sync); 0 = none.
    // Never closed: the OS closes it at exit, which is the point.
    static JOB: OnceLock<usize> = OnceLock::new();
    let job = *JOB.get_or_init(|| {
        // SAFETY: plain Win32 calls on a handle this function owns.
        unsafe {
            let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if job.is_null() {
                return 0;
            }
            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            let ok = SetInformationJobObject(
                job,
                JobObjectExtendedLimitInformation,
                &info as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION as *const core::ffi::c_void,
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            );
            if ok == 0 {
                CloseHandle(job);
                return 0;
            }
            job as usize
        }
    });
    if job == 0 {
        eprintln!("llama-server sidecar: no kill-on-close job object; relying on the exit hooks");
        return;
    }
    // SAFETY: both handles are valid for the duration of the call.
    let ok = unsafe { AssignProcessToJobObject(job as HANDLE, child.as_raw_handle() as HANDLE) };
    if ok == 0 {
        eprintln!(
            "llama-server sidecar: could not join the kill-on-close job ({})",
            std::io::Error::last_os_error()
        );
    }
}

#[cfg(not(windows))]
fn kill_with_parent(_child: &Child) {}

/// Extra words for errors where macOS may have blocked the nested binary.
fn quarantine_hint() -> &'static str {
    if cfg!(target_os = "macos") {
        " — if macOS blocked it (downloaded app), run `xattr -cr /Applications/sussurro.app` once and retry"
    } else {
        ""
    }
}

/// The last `n` lines of the server log, for error messages.
fn log_tail(path: Option<&Path>, n: usize) -> String {
    let Some(text) = path.and_then(|p| std::fs::read(p).ok()) else {
        return String::new();
    };
    let text = String::from_utf8_lossy(&text);
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    let tail = lines[lines.len().saturating_sub(n)..].join("\n");
    if tail.is_empty() {
        String::new()
    } else {
        format!("\nlast lines of the llama-server log:\n{tail}")
    }
}

enum PostError {
    /// No answer: connection refused or reset, timeout (a crash or a hang).
    Transport(anyhow::Error),
    /// The server answered, with an error or something unreadable.
    Server(anyhow::Error),
}

/// A running `llama-server`, restarted when it crashes and killed when
/// dropped. See the module docs.
pub struct Sidecar {
    cfg: SidecarConfig,
    child: SharedChild,
    port: u16,
    http: reqwest::blocking::Client,
    /// Consecutive failed starts and crashes; reset by a good answer.
    failures: u32,
    /// No restart before this.
    retry_at: Option<Instant>,
}

impl Sidecar {
    /// Spawn the server and wait until it is healthy.
    pub fn start(cfg: SidecarConfig) -> Result<Self> {
        let mut s = Self::stopped(cfg)?;
        s.launch()?;
        Ok(s)
    }

    /// A sidecar that is not running yet: [`Sidecar::ensure_running`]
    /// starts it. Unlike [`Sidecar::start`], a failed first start keeps the
    /// failure count, so the backoff applies to it too (#118).
    pub fn stopped(cfg: SidecarConfig) -> Result<Self> {
        if let Some(log) = &cfg.log_file {
            if let Some(dir) = log.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            let _ = std::fs::File::create(log); // a fresh log per load
        }
        let http = reqwest::blocking::Client::builder()
            .no_proxy() // loopback: never through a system proxy
            .connect_timeout(Duration::from_secs(2))
            .timeout(cfg.request_timeout)
            .build()?;
        let s = Self {
            cfg,
            child: Arc::new(Mutex::new(None)),
            port: 0,
            http,
            failures: 0,
            retry_at: None,
        };
        register(&s.child);
        Ok(s)
    }

    /// Start the process if it is not running (never started, crashed or
    /// stopped), honouring the backoff; no-op while it runs.
    pub fn ensure_running(&mut self) -> Result<()> {
        if self.is_running() {
            return Ok(());
        }
        self.launch()
    }

    /// What it runs.
    pub fn config(&self) -> &SidecarConfig {
        &self.cfg
    }

    /// The loopback port it listens on.
    pub fn port(&self) -> u16 {
        self.port
    }

    /// `http://127.0.0.1:<port>`: the server's base URL while it runs.
    pub fn base_url(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    /// A request answered: the failure count starts over.
    pub fn note_ok(&mut self) {
        self.failures = 0;
    }

    /// Whether the process died within `d` of a failed request — a crash,
    /// counted towards the backoff — rather than a slow or refused answer
    /// from a live server.
    pub fn crashed_within(&mut self, d: Duration) -> bool {
        if self.exits_within(d) {
            self.note_failure();
            return true;
        }
        false
    }

    /// Consecutive failed starts and crashes.
    pub fn failures(&self) -> u32 {
        self.failures
    }

    /// The process id, while it runs.
    pub fn pid(&self) -> Option<u32> {
        lock(&self.child).as_ref().map(Child::id)
    }

    /// Whether the process is alive (reaps it if it exited).
    pub fn is_running(&self) -> bool {
        self.exit_status().is_none() && lock(&self.child).is_some()
    }

    /// `Some` once the process has exited (and was reaped).
    fn exit_status(&self) -> Option<ExitStatus> {
        let mut guard = lock(&self.child);
        let status = guard.as_mut()?.try_wait().ok().flatten()?;
        *guard = None;
        Some(status)
    }

    /// Kill the process now; the next request restarts it.
    pub fn stop(&mut self) {
        if let Some(mut child) = lock(&self.child).take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }

    fn note_failure(&mut self) {
        self.failures += 1;
        self.retry_at = Some(Instant::now() + backoff(self.cfg.backoff_base, self.failures));
    }

    /// (Re)start, honouring the backoff.
    fn launch(&mut self) -> Result<()> {
        if let Some(at) = self.retry_at {
            let wait = at.saturating_duration_since(Instant::now());
            if wait > MAX_INLINE_BACKOFF {
                bail!(
                    "the {} sidecar failed {} times in a row; next try in {} s",
                    self.cfg.label(),
                    self.failures,
                    wait.as_secs().max(1)
                );
            }
            std::thread::sleep(wait);
        }
        self.stop();
        match self.try_launch() {
            Ok(()) => {
                self.retry_at = None;
                Ok(())
            }
            Err(e) => {
                self.stop();
                self.note_failure();
                Err(e)
            }
        }
    }

    fn try_launch(&mut self) -> Result<()> {
        let port = free_loopback_port()?;
        let mut cmd = command(&self.cfg, port);
        match &self.cfg.log_file {
            Some(path) => {
                let log = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(path)
                    .with_context(|| format!("opening {}", path.display()))?;
                cmd.stdout(log.try_clone()?).stderr(log);
            }
            None => {
                cmd.stdout(Stdio::null()).stderr(Stdio::null());
            }
        }
        let child = spawn_child(cmd).with_context(|| {
            format!(
                "could not start the llama-server sidecar {}{}",
                self.cfg.binary.display(),
                quarantine_hint()
            )
        })?;
        kill_with_parent(&child);
        *lock(&self.child) = Some(child);
        self.port = port;
        self.wait_healthy()
    }

    fn wait_healthy(&self) -> Result<()> {
        let started = Instant::now();
        let url = format!("http://127.0.0.1:{}/health", self.port);
        let probe = reqwest::blocking::Client::builder()
            .no_proxy()
            .timeout(Duration::from_secs(2))
            .build()?;
        loop {
            if let Some(status) = self.exit_status() {
                bail!(
                    "the llama-server sidecar exited while starting ({status}){}{}",
                    quarantine_hint(),
                    log_tail(self.cfg.log_file.as_deref(), 5)
                );
            }
            // 503 while the model loads, 200 once ready.
            if probe
                .get(&url)
                .send()
                .is_ok_and(|r| r.status().is_success())
            {
                return Ok(());
            }
            if started.elapsed() >= self.cfg.start_timeout {
                bail!(
                    "the llama-server sidecar was not ready after {} s{}",
                    self.cfg.start_timeout.as_secs(),
                    log_tail(self.cfg.log_file.as_deref(), 5)
                );
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    fn post(&self, wav: &[u8]) -> std::result::Result<String, PostError> {
        let boundary = new_boundary().map_err(PostError::Server)?;
        let body = multipart_body(
            &boundary,
            &[("model", "qwen3-asr"), ("response_format", "json")],
            "audio.wav",
            wav,
        );
        let resp = self
            .http
            .post(format!(
                "http://127.0.0.1:{}/v1/audio/transcriptions",
                self.port
            ))
            .header(
                reqwest::header::CONTENT_TYPE,
                format!("multipart/form-data; boundary={boundary}"),
            )
            .body(body)
            .send()
            .map_err(|e| {
                PostError::Transport(anyhow::Error::new(e).context("sidecar request failed"))
            })?;
        let status = resp.status();
        let text = resp.text().map_err(|e| {
            PostError::Transport(anyhow::Error::new(e).context("reading the sidecar answer"))
        })?;
        if !status.is_success() {
            let detail: String = text.chars().take(300).collect();
            return Err(PostError::Server(anyhow::anyhow!(
                "the sidecar answered {status}: {detail}"
            )));
        }
        let v: serde_json::Value = serde_json::from_str(&text).map_err(|e| {
            PostError::Server(anyhow::anyhow!("the sidecar answer is not JSON: {e}"))
        })?;
        v["text"]
            .as_str()
            .map(str::to_string)
            .ok_or_else(|| PostError::Server(anyhow::anyhow!("the sidecar answer has no text")))
    }

    /// Whether the process exits within `d` (a crash takes a moment to be
    /// seen after the connection drops).
    fn exits_within(&self, d: Duration) -> bool {
        let until = Instant::now() + d;
        loop {
            if !self.is_running() {
                return true;
            }
            if Instant::now() >= until {
                return false;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// Transcribe one WAV (≤ 30 s): the raw model output. Restarts a
    /// crashed server (with backoff) and retries once.
    pub fn transcribe_wav(&mut self, wav: &[u8]) -> Result<String> {
        let mut last = None;
        for _ in 0..2 {
            self.ensure_running()
                .context("restarting the Qwen3-ASR sidecar")?;
            match self.post(wav) {
                Ok(text) => {
                    self.failures = 0;
                    return Ok(text);
                }
                Err(PostError::Server(e)) => return Err(e),
                Err(PostError::Transport(e)) => {
                    if !self.exits_within(Duration::from_millis(500)) {
                        // Alive but not answering (timeout): don't trust it.
                        self.stop();
                        return Err(e.context(
                            "the Qwen3-ASR sidecar did not answer; it will restart on the next use",
                        ));
                    }
                    eprintln!("llama-server sidecar crashed during a request: {e:#}; restarting");
                    self.note_failure();
                    last = Some(e);
                }
            }
        }
        Err(last
            .unwrap_or_else(|| anyhow::anyhow!("sidecar request failed"))
            .context(format!(
                "the Qwen3-ASR sidecar crashed twice on this audio{}",
                log_tail(self.cfg.log_file.as_deref(), 5)
            )))
    }
}

impl Drop for Sidecar {
    fn drop(&mut self) {
        self.stop();
    }
}

// ----------------------------------------------------------- transcriber --

/// `AnyTranscriber::Remote`: Qwen3-ASR in the sidecar.
pub struct RemoteTranscriber {
    /// Boxed: keeps `AnyTranscriber` small (clippy `large_enum_variant`).
    sidecar: Box<Sidecar>,
}

impl RemoteTranscriber {
    /// Start the sidecar (seconds: model load) and keep it for the
    /// transcriber's lifetime.
    pub fn start(cfg: SidecarConfig) -> Result<Self> {
        Ok(Self {
            sidecar: Box::new(Sidecar::start(cfg)?),
        })
    }

    pub fn sidecar(&self) -> &Sidecar {
        &self.sidecar
    }

    /// samples: 16 kHz mono f32. No prompt, no language hint (see the
    /// module docs).
    pub fn transcribe(&mut self, samples: &[f32]) -> Result<String> {
        Ok(self.transcribe_detect(samples)?.text)
    }

    /// Long-form variant: no word timings (Qwen3-ASR needs a separate
    /// aligner for them), so the engine splits the segment proportionally.
    pub fn transcribe_timed(&mut self, samples: &[f32]) -> Result<super::TimedTranscript> {
        let out = self.transcribe_detect(samples)?;
        Ok(super::TimedTranscript {
            text: out.text,
            words: Vec::new(),
            words_estimated: false,
            language: out.language,
        })
    }

    fn transcribe_detect(&mut self, samples: &[f32]) -> Result<AsrOutput> {
        let mut texts = Vec::new();
        let mut language = None;
        for range in input_pieces(samples) {
            let piece = &samples[range];
            if piece.is_empty() {
                continue;
            }
            let raw = self
                .sidecar
                .transcribe_wav(&crate::archive::audio::wav_bytes(piece))?;
            let out = sanitize(&raw);
            language = language.or(out.language);
            if !out.text.is_empty() {
                texts.push(out.text);
            }
        }
        Ok(AsrOutput {
            text: texts.join(" "),
            language,
        })
    }
}

/// The app's sidecar configuration for the Qwen3-ASR files in `models_dir`.
pub fn qwen3_asr_config(
    paths: &super::sidecar::SidecarPaths,
    models_dir: &Path,
    log_file: Option<PathBuf>,
) -> SidecarConfig {
    let mut cfg = SidecarConfig::new(
        paths.binary.clone(),
        paths.lib_dir.clone(),
        models_dir.join(QWEN3_ASR_MODEL),
        models_dir.join(QWEN3_ASR_MMPROJ),
    );
    cfg.log_file = log_file;
    cfg
}

#[cfg(test)]
#[allow(clippy::single_range_in_vec_init)]
pub(crate) mod tests;
