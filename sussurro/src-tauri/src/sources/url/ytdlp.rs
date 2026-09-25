//! Video platforms through `yt-dlp` (E10): found on PATH or in the usual
//! install folders, never bundled. It is started with an argument vector
//! (no shell), asked for the m4a audio track only (`bestaudio[ext=m4a]`, so
//! symphonia decodes it without ffmpeg), no playlists, written to the run's
//! temporary file. Its progress is parsed from its output; a cancel kills
//! it (its whole process group / tree).
//!
//! **Network (#216)** — yt-dlp does its own networking, so the link rules
//! are enforced on it from outside: it runs only for known video platforms
//! ([`super::yt_dlp_allowed`]), with the generic extractor excluded
//! (`--use-extractors default,-generic`: no embedded `<video src>` /
//! `og:video` URLs scraped from arbitrary pages), and every connection it
//! makes goes through the in-process [`GuardProxy`] (`--proxy`), which
//! resolves, checks and pins each host exactly like a direct download.
//! Native downloaders only (`--downloader native`: no ffmpeg or aria2c
//! with networking of their own), and the proxy variables of the
//! environment are removed. A yt-dlp too old for `--use-extractors`
//! (added in 2022.08) is retried without it — still behind the proxy.

use super::proxy::GuardProxy;
use super::{Progress, TempDownload};
use anyhow::{anyhow, bail, Context, Result};
use reqwest::Url;
use std::ffi::{OsStr, OsString};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// The executable's file name on this OS.
pub const BINARY: &str = if cfg!(windows) {
    "yt-dlp.exe"
} else {
    "yt-dlp"
};
/// The format asked for: the m4a audio track, else the best audio-only one.
pub const FORMAT: &str = "bestaudio[ext=m4a]/bestaudio";
/// Every extractor but the generic one, which scrapes any page for media
/// URLs (embedded ones included).
pub const EXTRACTORS: &str = "default,-generic";
/// Environment variables that would point yt-dlp at another proxy.
const PROXY_VARS: &[&str] = &[
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "ALL_PROXY",
    "NO_PROXY",
    "http_proxy",
    "https_proxy",
    "all_proxy",
    "no_proxy",
];
/// What an old yt-dlp says about an option it doesn't know.
const UNKNOWN_EXTRACTORS_OPTION: &str = "no such option: --use-extractors";

/// Markers of the lines Sussurro asks yt-dlp to print.
const TITLE_MARK: &str = "sussurro-title ";
const FORMAT_MARK: &str = "sussurro-format ";
const FILE_MARK: &str = "sussurro-file ";
const PROGRESS_MARK: &str = "sussurro-progress ";

// ---- detection ---------------------------------------------------------------

/// Folders searched after PATH: a GUI app on macOS does not inherit the
/// shell's PATH, so Homebrew's folder must be looked at explicitly; on
/// Windows winget, scoop, Chocolatey and pip put the binary in folders that
/// are often missing from PATH. `env` reads an environment variable. Pure.
pub fn extra_dirs(home: Option<&Path>, env: &dyn Fn(&str) -> Option<OsString>) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if cfg!(windows) {
        if let Some(local) = env("LOCALAPPDATA").map(PathBuf::from) {
            dirs.push(local.join("Microsoft").join("WinGet").join("Links"));
            dirs.extend(python_scripts(&local.join("Programs").join("Python")));
        }
        if let Some(appdata) = env("APPDATA").map(PathBuf::from) {
            dirs.extend(python_scripts(&appdata.join("Python")));
        }
        if let Some(home) = home {
            dirs.push(home.join("scoop").join("shims"));
            dirs.push(home.join(".local").join("bin"));
        }
        let program_data = env("ProgramData")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(r"C:\ProgramData"));
        dirs.push(program_data.join("chocolatey").join("bin"));
    } else {
        if cfg!(target_os = "macos") {
            dirs.push(PathBuf::from("/opt/homebrew/bin"));
            dirs.push(PathBuf::from("/opt/local/bin"));
        }
        dirs.push(PathBuf::from("/usr/local/bin"));
        dirs.push(PathBuf::from("/usr/bin"));
        if cfg!(target_os = "linux") {
            dirs.push(PathBuf::from("/snap/bin"));
        }
        if let Some(home) = home {
            dirs.push(home.join(".local").join("bin"));
        }
    }
    dirs
}

/// `<root>/Python3xx/Scripts` for every installed Python (Windows pip).
fn python_scripts(root: &Path) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = std::fs::read_dir(root)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with("Python"))
        .map(|e| e.path().join("Scripts"))
        .collect();
    out.sort();
    out.reverse(); // newest Python first
    out
}

fn is_executable(p: &Path) -> bool {
    let Ok(meta) = std::fs::metadata(p) else {
        return false;
    };
    if !meta.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        meta.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

/// The first `yt-dlp` executable in the PATH entries (`path_var`), then
/// in `extra`. Only absolute folders count (a relative PATH entry would
/// depend on the app's working directory).
pub fn find_in(path_var: Option<&OsStr>, extra: &[PathBuf]) -> Option<PathBuf> {
    let from_path = path_var
        .map(|p| std::env::split_paths(p).collect::<Vec<_>>())
        .unwrap_or_default();
    from_path
        .iter()
        .chain(extra)
        .filter(|d| d.is_absolute())
        .map(|d| d.join(BINARY))
        .find(|p| is_executable(p))
}

/// Find `yt-dlp` for this process: PATH, then the usual install folders.
pub fn find() -> Option<PathBuf> {
    let home =
        std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(PathBuf::from);
    let extra = extra_dirs(home.as_deref(), &|k| std::env::var_os(k));
    find_in(std::env::var_os("PATH").as_deref(), &extra)
}

/// How to install yt-dlp, for `os` as in `std::env::consts::OS`. Pure.
pub fn install_instructions_for(os: &str) -> String {
    let how = match os {
        "macos" => "with Homebrew: `brew install yt-dlp` (or `pipx install yt-dlp`)",
        "windows" => "with `winget install yt-dlp.yt-dlp` (or scoop: `scoop install yt-dlp`), then restart Sussurro",
        _ => "with your package manager (e.g. `sudo apt install yt-dlp`) or `pipx install yt-dlp` — distribution packages can be old; pipx keeps it current",
    };
    format!(
        "Install yt-dlp {how}. It is not bundled with Sussurro: video sites change often and \
         yt-dlp is updated to follow them. See https://github.com/yt-dlp/yt-dlp#installation"
    )
}

pub fn install_instructions() -> String {
    install_instructions_for(std::env::consts::OS)
}

/// The error of a video-platform link without yt-dlp.
pub fn missing_error() -> anyhow::Error {
    anyhow!(
        "links to video sites need yt-dlp, which was not found. {}",
        install_instructions()
    )
}

// ---- invocation ----------------------------------------------------------------

/// How yt-dlp reaches the network (#216).
#[derive(Debug, Clone, PartialEq)]
pub struct NetArgs {
    /// The guard proxy's URL (`--proxy`).
    pub proxy: String,
    /// `--use-extractors default,-generic`; off only for a yt-dlp too old
    /// to know the option.
    pub restrict_extractors: bool,
}

/// The argument vector (no shell is ever involved): audio only, no
/// playlist, the user's yt-dlp config ignored (it could add conversions
/// needing ffmpeg, a proxy or cookies), every connection through `net`'s
/// proxy, no generic extractor, native downloaders only, machine-readable
/// progress, title and output path. The link comes last, after `--`, so it
/// can never be read as an option. Pure.
pub fn build_args(
    url: &Url,
    output_template: &Path,
    max_bytes: u64,
    net: &NetArgs,
) -> Vec<OsString> {
    let mut args: Vec<OsString> = [
        "--ignore-config",
        "--proxy",
        &net.proxy,
        "--downloader",
        "native",
        "--no-playlist",
        "--no-simulate",
        "--newline",
        "--progress",
        "--no-mtime",
        "-f",
        FORMAT,
        "--max-filesize",
        &max_bytes.to_string(),
        "--progress-template",
        &format!(
            "download:{PROGRESS_MARK}%(progress.downloaded_bytes)s %(progress.total_bytes)s \
             %(progress.total_bytes_estimate)s"
        ),
        "--print",
        &format!("before_dl:{TITLE_MARK}%(title)s"),
        "--print",
        &format!("before_dl:{FORMAT_MARK}%(ext)s %(acodec)s"),
        "--print",
        &format!("after_move:{FILE_MARK}%(filepath)s"),
    ]
    .iter()
    .map(OsString::from)
    .collect();
    if net.restrict_extractors {
        args.push("--use-extractors".into());
        args.push(EXTRACTORS.into());
    }
    args.push("-o".into());
    args.push(output_template.as_os_str().to_owned());
    args.push("--".into());
    args.push(url.as_str().into());
    args
}

/// One line of yt-dlp's output, understood.
#[derive(Debug, Clone, PartialEq)]
pub enum Line {
    Progress { downloaded: u64, total: Option<u64> },
    Title(String),
    Format { ext: String, acodec: String },
    File(PathBuf),
    Other,
}

fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            if chars.peek() == Some(&'[') {
                chars.next();
                for c in chars.by_ref() {
                    if c.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
            continue;
        }
        out.push(c);
    }
    out
}

fn number(s: &str) -> Option<u64> {
    let v: f64 = s.trim().parse().ok()?;
    (v.is_finite() && v >= 0.0).then_some(v as u64)
}

/// `12.34MiB` → bytes.
fn size(s: &str) -> Option<u64> {
    let s = s.trim().trim_start_matches('~').trim();
    let split = s.find(|c: char| c.is_ascii_alphabetic())?;
    let (num, unit) = s.split_at(split);
    let v: f64 = num.trim().parse().ok()?;
    let mult = match unit {
        "B" => 1.0,
        "KiB" => 1024.0,
        "MiB" => 1024.0 * 1024.0,
        "GiB" => 1024.0 * 1024.0 * 1024.0,
        "KB" | "kB" => 1e3,
        "MB" => 1e6,
        "GB" => 1e9,
        _ => return None,
    };
    Some((v * mult) as u64)
}

/// Parse one output line: Sussurro's own markers (progress template,
/// `--print`) and, as a fallback, yt-dlp's default progress line
/// (`[download]  42.0% of ~  3.50MiB at …`). Pure.
pub fn parse_line(raw: &str) -> Line {
    let line = strip_ansi(raw);
    let line = line.trim();
    if let Some(rest) = line.strip_prefix(PROGRESS_MARK) {
        let mut f = rest.split_whitespace();
        let downloaded = f.next().and_then(number);
        let total = f.next().and_then(number);
        let estimate = f.next().and_then(number);
        return match downloaded {
            Some(downloaded) => Line::Progress {
                downloaded,
                total: total.or(estimate).filter(|&t| t > 0),
            },
            None => Line::Other,
        };
    }
    if let Some(t) = line.strip_prefix(TITLE_MARK) {
        let t = t.trim();
        return if t.is_empty() || t == "NA" {
            Line::Other
        } else {
            Line::Title(t.to_string())
        };
    }
    if let Some(rest) = line.strip_prefix(FORMAT_MARK) {
        let mut f = rest.split_whitespace();
        return Line::Format {
            ext: f.next().unwrap_or_default().to_ascii_lowercase(),
            acodec: f.next().unwrap_or_default().to_ascii_lowercase(),
        };
    }
    if let Some(p) = line.strip_prefix(FILE_MARK) {
        return Line::File(PathBuf::from(p.trim()));
    }
    if let Some(rest) = line.strip_prefix("[download]") {
        // "  42.0% of ~  3.50MiB at  1.00MiB/s ETA 00:02"
        let rest = rest.trim();
        if let Some((pct, after)) = rest.split_once('%') {
            if let (Ok(pct), Some(of)) =
                (pct.trim().parse::<f64>(), after.trim().strip_prefix("of"))
            {
                let total_str = of.trim().trim_start_matches('~').trim();
                let total_str = total_str.split_whitespace().next().unwrap_or_default();
                if let Some(total) = size(total_str) {
                    return Line::Progress {
                        downloaded: (total as f64 * pct.clamp(0.0, 100.0) / 100.0) as u64,
                        total: Some(total),
                    };
                }
            }
        }
    }
    Line::Other
}

/// Audio codecs symphonia can't decode (no ffmpeg is bundled). Pure.
pub fn is_undecodable_codec(ext: &str, acodec: &str) -> bool {
    ext == "opus" || acodec.starts_with("opus") || acodec == "ec-3" || acodec == "ac-3"
}

/// Last lines of yt-dlp's stderr, for the error message.
const STDERR_TAIL: usize = 20;

/// The error to show for a failed run: yt-dlp's last `ERROR:` line. Pure.
pub fn failure_message(stderr_tail: &[String], status: &str) -> String {
    let err = stderr_tail
        .iter()
        .rev()
        .map(|l| strip_ansi(l))
        .find(|l| l.trim_start().starts_with("ERROR:"))
        .map(|l| l.trim().trim_start_matches("ERROR:").trim().to_string());
    match err {
        Some(e) if e.contains("Requested format is not available") => format!(
            "yt-dlp found no audio-only track for this video ({e}). Sussurro needs an audio \
             track it can decode without ffmpeg"
        ),
        Some(e) if e.contains("File is larger than max-filesize") => {
            super::too_large(None, super::MAX_DOWNLOAD_BYTES).to_string()
        }
        Some(e) => format!("yt-dlp could not download the audio: {e}"),
        None => format!("yt-dlp stopped with {status}"),
    }
}

/// What yt-dlp fetched.
#[derive(Debug)]
pub struct Fetched {
    pub path: PathBuf,
    /// The platform's title for the video, if it gave one.
    pub title: Option<String>,
}

fn command(bin: &Path) -> Command {
    let mut cmd = Command::new(bin);
    for var in PROXY_VARS {
        cmd.env_remove(var);
    }
    cmd.stdin(Stdio::null())
        // Titles in UTF-8 whatever the console code page (Windows).
        .env("PYTHONUTF8", "1")
        .env("PYTHONIOENCODING", "utf-8");
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // Its own process group, so a cancel also stops what it spawned.
        cmd.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

/// Stop the process and anything it started.
fn kill_tree(child: &mut Child) {
    #[cfg(unix)]
    {
        if let Ok(pid) = libc::pid_t::try_from(child.id()) {
            // SAFETY: signals the process group created by `process_group(0)`.
            unsafe {
                libc::kill(-pid, libc::SIGKILL);
            }
        }
    }
    #[cfg(windows)]
    {
        // yt-dlp.exe is a PyInstaller bootloader that runs a child process.
        let mut tk = Command::new("taskkill");
        tk.args(["/PID", &child.id().to_string(), "/T", "/F"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        {
            use std::os::windows::process::CommandExt;
            tk.creation_flags(0x0800_0000);
        }
        let _ = tk.status();
    }
    let _ = child.kill();
    let _ = child.wait();
}

/// Read `r` and send each line (split on `\n` or `\r`).
fn pump_lines(mut r: impl Read, tx: impl Fn(String)) {
    let mut buf = [0u8; 8192];
    let mut line = Vec::new();
    loop {
        let n = match r.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        for &b in &buf[..n] {
            if b == b'\n' || b == b'\r' {
                if !line.is_empty() {
                    tx(String::from_utf8_lossy(&line).into_owned());
                    line.clear();
                }
            } else {
                line.push(b);
            }
        }
    }
    if !line.is_empty() {
        tx(String::from_utf8_lossy(&line).into_owned());
    }
}

/// The installed version (`yt-dlp --version`), within `timeout`.
pub fn version(bin: &Path, timeout: Duration) -> Result<String> {
    let mut child = command(bin)
        .arg("--version")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .with_context(|| format!("could not run {}", bin.display()))?;
    let started = Instant::now();
    loop {
        if child.try_wait()?.is_some() {
            break;
        }
        if started.elapsed() > timeout {
            kill_tree(&mut child);
            bail!("{} did not answer", bin.display());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let mut out = String::new();
    if let Some(mut s) = child.stdout.take() {
        let _ = s.read_to_string(&mut out);
    }
    let v = out.lines().next().unwrap_or_default().trim().to_string();
    if v.is_empty() {
        bail!("{} printed no version", bin.display());
    }
    Ok(v)
}

/// Download the audio of `url` with yt-dlp `bin` into `dest`, every
/// connection through a [`GuardProxy`] applying the link rules
/// (`allow_local` = the run's *Allow local network addresses*). Blocks
/// until yt-dlp exits; `cancel` kills it (checked every 100 ms).
pub fn download(
    bin: &Path,
    url: &Url,
    allow_local: bool,
    max_bytes: u64,
    dest: &TempDownload,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(&Progress),
) -> Result<Fetched> {
    let guard = GuardProxy::start(allow_local)?;
    let mut net = NetArgs {
        proxy: guard.url(),
        restrict_extractors: true,
    };
    loop {
        let args = build_args(url, &dest.template(), max_bytes, &net);
        match run(bin, args, max_bytes, dest, cancel, progress)? {
            RunEnd::Done(f) => return Ok(f),
            RunEnd::Failed { tail, .. }
                if net.restrict_extractors
                    && tail.iter().any(|l| l.contains(UNKNOWN_EXTRACTORS_OPTION)) =>
            {
                eprintln!(
                    "yt-dlp is too old for --use-extractors (2022.08+): retrying without it, \
                     still behind the address guard — please update yt-dlp"
                );
                net.restrict_extractors = false;
            }
            RunEnd::Failed { tail, status } => {
                if let Some(why) = guard.last_refusal() {
                    bail!("yt-dlp was sent to an address Sussurro refused: {why}");
                }
                bail!("{}", failure_message(&tail, &status));
            }
        }
    }
}

/// How one yt-dlp run ended.
enum RunEnd {
    Done(Fetched),
    /// A non-zero exit: its last stderr lines and exit status.
    Failed { tail: Vec<String>, status: String },
}

fn run(
    bin: &Path,
    args: Vec<OsString>,
    max_bytes: u64,
    dest: &TempDownload,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(&Progress),
) -> Result<RunEnd> {
    let mut child = command(bin)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("could not start yt-dlp ({})", bin.display()))?;

    let (tx, rx) = mpsc::channel::<String>();
    let stdout = child.stdout.take().expect("piped stdout");
    let out_thread = std::thread::spawn(move || pump_lines(stdout, |l| drop(tx.send(l))));
    let tail = Arc::new(Mutex::new(Vec::<String>::new()));
    let stderr = child.stderr.take().expect("piped stderr");
    let err_thread = {
        let tail = tail.clone();
        std::thread::spawn(move || {
            pump_lines(stderr, |l| {
                let mut t = tail.lock().unwrap();
                t.push(l);
                if t.len() > STDERR_TAIL {
                    t.remove(0);
                }
            })
        })
    };

    let mut state = Progress::default();
    let mut file: Option<PathBuf> = None;
    let abort = |child: &mut Child, e: anyhow::Error| -> Result<RunEnd> {
        kill_tree(child);
        Err(e)
    };
    loop {
        if cancel.load(Ordering::Relaxed) {
            return abort(&mut child, anyhow!("cancelled"));
        }
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(line) => match parse_line(&line) {
                Line::Progress { downloaded, total } => {
                    if downloaded > max_bytes {
                        return abort(&mut child, super::too_large(None, max_bytes));
                    }
                    state.downloaded = downloaded;
                    state.total = total;
                    progress(&state);
                }
                Line::Title(t) => {
                    state.title = Some(t);
                    progress(&state);
                }
                Line::Format { ext, acodec } if is_undecodable_codec(&ext, &acodec) => {
                    return abort(
                        &mut child,
                        anyhow!(
                            "this video only offers {acodec} audio ({ext}), which Sussurro can't \
                             decode without ffmpeg (not bundled) — it needs an m4a/aac audio track"
                        ),
                    );
                }
                Line::File(p) => file = Some(p),
                Line::Format { .. } | Line::Other => {}
            },
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            // stdout closed: yt-dlp is exiting.
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    let status = loop {
        if let Some(s) = child.try_wait()? {
            break s;
        }
        if cancel.load(Ordering::Relaxed) {
            return abort(&mut child, anyhow!("cancelled"));
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    let _ = out_thread.join();
    let _ = err_thread.join();
    if !status.success() {
        let tail = tail.lock().unwrap().clone();
        return Ok(RunEnd::Failed {
            tail,
            status: status.to_string(),
        });
    }
    // The printed path, if it is ours; else the finished file on disk.
    let path = file
        .filter(|p| p.parent() == Some(dest.dir()) && p.is_file())
        .or_else(|| {
            dest.files().into_iter().find(|p| {
                let ext = p
                    .extension()
                    .map(|e| e.to_string_lossy().to_string())
                    .unwrap_or_default();
                !matches!(ext.as_str(), "part" | "ytdl" | "temp")
            })
        })
        .ok_or_else(|| anyhow!("yt-dlp finished but produced no audio file"))?;
    Ok(RunEnd::Done(Fetched {
        path,
        title: state.title,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn args_are_a_vector_with_the_link_last_after_a_separator() {
        let url = Url::parse("https://www.youtube.com/watch?v=abc&list=PL1;rm -rf ~").unwrap();
        let out = Path::new("/data/link-downloads/link-1-2.%(ext)s");
        let net = NetArgs {
            proxy: "http://127.0.0.1:4321".into(),
            restrict_extractors: true,
        };
        let args = build_args(&url, out, 1000, &net);
        let s: Vec<String> = args
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        // One element per argument; the link verbatim (URL-encoded), last.
        assert_eq!(s[s.len() - 2], "--");
        assert_eq!(s[s.len() - 1], url.as_str());
        assert!(s.last().unwrap().contains(";rm%20-rf%20~"));
        let after = |flag: &str| s[s.iter().position(|a| a == flag).unwrap() + 1].clone();
        assert_eq!(after("-f"), "bestaudio[ext=m4a]/bestaudio");
        assert_eq!(after("-o"), "/data/link-downloads/link-1-2.%(ext)s");
        assert_eq!(after("--max-filesize"), "1000");
        for flag in [
            "--no-playlist",
            "--ignore-config",
            "--newline",
            "--no-simulate",
            "--progress",
        ] {
            assert!(s.iter().any(|a| a == flag), "{flag}");
        }
        assert!(
            !s.iter().any(|a| a == "-x" || a.contains("ffmpeg")),
            "no conversion"
        );
        // #216: every connection through the guard, no generic extractor,
        // native downloaders only.
        assert_eq!(after("--proxy"), "http://127.0.0.1:4321");
        assert_eq!(after("--use-extractors"), "default,-generic");
        assert_eq!(after("--downloader"), "native");
        assert_eq!(
            s.iter().filter(|a| *a == "--proxy").count(),
            1,
            "one proxy"
        );
        // A yt-dlp too old for `--use-extractors`: the rest is unchanged.
        let old = build_args(
            &url,
            out,
            1000,
            &NetArgs {
                restrict_extractors: false,
                ..net
            },
        );
        let without: Vec<&OsString> = args
            .iter()
            .filter(|a| *a != "--use-extractors" && *a != EXTRACTORS)
            .collect();
        assert_eq!(old.iter().collect::<Vec<_>>(), without);
        assert!(old.iter().any(|a| a == "--proxy"));
    }

    #[test]
    fn yt_dlp_never_inherits_proxy_variables() {
        let cmd = command(Path::new("/bin/yt-dlp"));
        for var in PROXY_VARS {
            assert!(
                cmd.get_envs().any(|(k, v)| k == OsStr::new(var) && v.is_none()),
                "{var} is removed"
            );
        }
    }

    #[test]
    fn progress_title_and_file_are_parsed_from_captured_output() {
        // Captured from `yt-dlp` 2025.x with Sussurro's arguments, plus
        // the default progress lines of a run without the template.
        let captured = "\
sussurro-title Rick Astley - Never Gonna Give You Up (Official Video)
sussurro-format m4a mp4a.40.2
sussurro-progress 1024 3456789 NA
sussurro-progress 1728394 3456789 NA
sussurro-progress 2048 NA 4000000.5
sussurro-progress NA NA NA
[download]   0.0% of    3.30MiB at  Unknown B/s ETA Unknown
[download]  50.0% of ~   2.00MiB at    1.00MiB/s ETA 00:01 (frag 3/10)
\x1b[0;94m[download]\x1b[0m 100% of    1.00KiB in 00:00:01 at 1.00KiB/s
[youtube] Extracting URL: https://www.youtube.com/watch?v=dQw4w9WgXcQ
sussurro-file /data/link-downloads/link-1-2.m4a
";
        let lines: Vec<Line> = captured.lines().map(parse_line).collect();
        assert_eq!(
            lines[0],
            Line::Title("Rick Astley - Never Gonna Give You Up (Official Video)".into())
        );
        assert_eq!(
            lines[1],
            Line::Format {
                ext: "m4a".into(),
                acodec: "mp4a.40.2".into()
            }
        );
        assert_eq!(
            lines[2],
            Line::Progress {
                downloaded: 1024,
                total: Some(3_456_789)
            }
        );
        assert_eq!(
            lines[3],
            Line::Progress {
                downloaded: 1_728_394,
                total: Some(3_456_789)
            }
        );
        assert_eq!(
            lines[4],
            Line::Progress {
                downloaded: 2048,
                total: Some(4_000_000)
            }
        );
        assert_eq!(lines[5], Line::Other);
        assert_eq!(
            lines[6],
            Line::Progress {
                downloaded: 0,
                total: Some(3_460_300)
            }
        );
        assert_eq!(
            lines[7],
            Line::Progress {
                downloaded: 1_048_576,
                total: Some(2_097_152)
            }
        );
        assert_eq!(
            lines[8],
            Line::Progress {
                downloaded: 1024,
                total: Some(1024)
            }
        );
        assert_eq!(lines[9], Line::Other);
        assert_eq!(
            lines[10],
            Line::File("/data/link-downloads/link-1-2.m4a".into())
        );
        assert!(is_undecodable_codec("webm", "opus"));
        assert!(!is_undecodable_codec("m4a", "mp4a.40.2"));
        assert!(!is_undecodable_codec("webm", "vorbis"));
    }

    #[test]
    fn failures_are_explained() {
        let tail = vec![
            "WARNING: something".to_string(),
            "ERROR: [youtube] abc: Video unavailable".to_string(),
        ];
        assert_eq!(
            failure_message(&tail, "exit status: 1"),
            "yt-dlp could not download the audio: [youtube] abc: Video unavailable"
        );
        let e = failure_message(
            &["ERROR: [x] y: Requested format is not available".into()],
            "1",
        );
        assert!(e.contains("no audio-only track"), "{e}");
        assert_eq!(
            failure_message(&[], "signal: 9"),
            "yt-dlp stopped with signal: 9"
        );
    }

    #[test]
    fn missing_binary_gives_install_instructions_per_os() {
        assert!(install_instructions_for("macos").contains("brew install yt-dlp"));
        assert!(install_instructions_for("windows").contains("winget install yt-dlp.yt-dlp"));
        assert!(install_instructions_for("linux").contains("pipx install yt-dlp"));
        let e = missing_error().to_string();
        assert!(
            e.contains("not found") && e.contains("Install yt-dlp"),
            "{e}"
        );
    }

    fn fake_bin(dir: &Path, script: &str) -> PathBuf {
        let p = dir.join(BINARY);
        std::fs::write(&p, script).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        p
    }

    #[test]
    fn yt_dlp_is_found_on_path_or_in_extra_folders() {
        let root = tempfile::tempdir().unwrap();
        let empty = root.path().join("empty");
        let bin_dir = root.path().join("bin");
        let brew = root.path().join("homebrew");
        for d in [&empty, &bin_dir, &brew] {
            std::fs::create_dir_all(d).unwrap();
        }
        let path_var = std::env::join_paths([&empty, &bin_dir]).unwrap();
        assert_eq!(find_in(Some(&path_var), &[]), None);
        let fake = fake_bin(&bin_dir, "#!/bin/sh\necho 2025.09.26\n");
        assert_eq!(find_in(Some(&path_var), &[]), Some(fake.clone()));
        // Not on PATH, but in a known install folder (e.g. Homebrew).
        let only_empty = std::env::join_paths([&empty]).unwrap();
        assert_eq!(
            find_in(Some(&only_empty), std::slice::from_ref(&brew)),
            None
        );
        let in_brew = fake_bin(&brew, "#!/bin/sh\necho 2025.09.26\n");
        assert_eq!(
            find_in(Some(&only_empty), std::slice::from_ref(&brew)),
            Some(in_brew)
        );
        assert_eq!(find_in(None, &[]), None);
        // Relative PATH entries are ignored.
        assert_eq!(find_in(Some(OsStr::new("bin")), &[]), None);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            // Not executable: not a candidate.
            std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o644)).unwrap();
            assert_eq!(find_in(Some(&path_var), &[]), None);
        }
        let dirs = extra_dirs(Some(Path::new("/home/u")), &|_| None);
        if cfg!(target_os = "macos") {
            assert!(dirs.contains(&PathBuf::from("/opt/homebrew/bin")));
        }
        if !cfg!(windows) {
            assert!(dirs.contains(&PathBuf::from("/home/u/.local/bin")));
        }
    }

    /// A fake yt-dlp (a shell script) exercises the real process handling:
    /// arguments, output parsing, the downloaded file, errors, cancel.
    #[cfg(unix)]
    mod process {
        use super::*;

        /// Writes `$SRC` to the `-o` template with extension `wav`.
        /// Its arguments are appended to `$ROOT/args`, one per line, after
        /// a `run` line.
        const FAKE_OK: &str = r#"#!/bin/sh
echo run >> "$ROOT/args"
for a in "$@"; do printf '%s\n' "$a" >> "$ROOT/args"; done
out=""; prev=""; last=""
for a in "$@"; do
  if [ "$prev" = "-o" ]; then out="$a"; fi
  prev="$a"; last="$a"
done
case "$last" in https://*) ;; *) echo "ERROR: bad link $last" >&2; exit 2;; esac
file=$(printf '%s' "$out" | sed 's/%(ext)s/wav/')
echo "sussurro-title A talk: about \"quotes\" & things"
echo "sussurro-format wav pcm_s16le"
echo "sussurro-progress 10 20 NA"
cp "$SRC" "$file"
echo "sussurro-progress 20 20 NA"
echo "sussurro-file $file"
"#;

        /// `$SRC` in `script` is a short WAV file, `$ROOT` the run's temp
        /// folder. With `cancel_on_progress` the run is cancelled from the
        /// first progress report — an event, never a timer (#187).
        fn run(
            script: &str,
            cancel_on_progress: bool,
        ) -> (
            Result<Fetched>,
            Vec<Progress>,
            TempDownload,
            tempfile::TempDir,
        ) {
            let root = tempfile::tempdir().unwrap();
            let src = root.path().join("src.wav");
            std::fs::write(&src, crate::sources::url::direct::tests::wav_bytes(0.5)).unwrap();
            let script = script
                .replace("$SRC", &src.to_string_lossy())
                .replace("$ROOT", &root.path().to_string_lossy());
            let bin = fake_bin(root.path(), &script);
            let dest = TempDownload::create(&root.path().join("dl"), 5).unwrap();
            let cancel = AtomicBool::new(false);
            let mut seen = Vec::new();
            let url = Url::parse("https://www.youtube.com/watch?v=abc").unwrap();
            let r = retry_text_file_busy(|| {
                download(&bin, &url, false, 1 << 30, &dest, &cancel, &mut |p| {
                    seen.push(p.clone());
                    if cancel_on_progress {
                        cancel.store(true, Ordering::Relaxed);
                    }
                })
            });
            (r, seen, dest, root)
        }

        /// Linux refuses to exec a file some process still holds open for
        /// writing (ETXTBSY). A test writes its fake yt-dlp and runs it at
        /// once, while other test threads fork: a child forked in that
        /// window briefly inherits the write descriptor. The spawn then
        /// fails before anything ran, so trying again is safe (#187).
        fn retry_text_file_busy<T>(mut f: impl FnMut() -> Result<T>) -> Result<T> {
            let busy = |e: &anyhow::Error| {
                e.chain().any(|c| {
                    c.downcast_ref::<std::io::Error>()
                        .and_then(std::io::Error::raw_os_error)
                        == Some(libc::ETXTBSY)
                })
            };
            let mut attempts = 0;
            loop {
                match f() {
                    Err(e) if busy(&e) && attempts < 50 => {
                        attempts += 1;
                        std::thread::sleep(Duration::from_millis(20));
                    }
                    r => return r,
                }
            }
        }

        /// Whether `pid` is still a running process (a zombie is not:
        /// it has exited and only waits to be reaped by its new parent).
        fn running(pid: libc::pid_t) -> bool {
            // SAFETY: signal 0 only checks that the process exists.
            if unsafe { libc::kill(pid, 0) } != 0 {
                return false;
            }
            match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
                // "pid (comm) S ...": the state follows the last ')'.
                Ok(stat) => stat
                    .rsplit_once(')')
                    .map(|(_, rest)| rest.trim_start())
                    .is_none_or(|rest| !rest.starts_with('Z')),
                // No procfs (macOS): launchd reaps orphans at once.
                Err(_) => true,
            }
        }

        #[test]
        fn a_fake_yt_dlp_downloads_with_title_and_progress() {
            let (r, seen, dest, _root) = run(FAKE_OK, false);
            let f = r.unwrap();
            assert_eq!(f.path, dest.path("wav"));
            assert_eq!(
                f.title.as_deref(),
                Some("A talk: about \"quotes\" & things")
            );
            assert_eq!(seen.last().unwrap().downloaded, 20);
            assert_eq!(seen.last().unwrap().total, Some(20));
            assert!(crate::sources::file::FileSource::open(&f.path).is_ok());
            // Pointed at the guard proxy, which was listening during the run.
            let args = std::fs::read_to_string(_root.path().join("args")).unwrap();
            let args: Vec<&str> = args.lines().collect();
            let after = |f: &str| args[args.iter().position(|a| *a == f).unwrap() + 1];
            assert!(after("--proxy").starts_with("http://127.0.0.1:"));
            assert_eq!(after("--use-extractors"), EXTRACTORS);
            assert_eq!(args.iter().filter(|a| **a == "run").count(), 1);
            let v = fake_bin(_root.path(), "#!/bin/sh\necho 2025.09.26\n");
            assert_eq!(
                retry_text_file_busy(|| version(&v, Duration::from_secs(30))).unwrap(),
                "2025.09.26"
            );
        }

        #[test]
        fn an_old_yt_dlp_is_retried_without_use_extractors_but_behind_the_guard() {
            // optparse's answer to an unknown option, as yt-dlp < 2022.08
            // prints it.
            let script = FAKE_OK.replacen(
                "echo run >> \"$ROOT/args\"\n",
                "echo run >> \"$ROOT/args\"\nfor a in \"$@\"; do if [ \"$a\" = --use-extractors ]; then \
                 echo 'Usage: yt-dlp [OPTIONS] URL [URL...]' >&2; \
                 echo 'yt-dlp: error: no such option: --use-extractors' >&2; exit 2; fi; done\n",
                1,
            );
            let (r, _, _, root) = run(&script, false);
            assert!(r.is_ok(), "{r:?}");
            let args = std::fs::read_to_string(root.path().join("args")).unwrap();
            assert_eq!(args.lines().filter(|l| *l == "run").count(), 2, "{args}");
            // The first run stopped at the option check; the second one's
            // arguments are recorded.
            let runs: Vec<&str> = args.split("run\n").filter(|r| !r.is_empty()).collect();
            assert_eq!(runs.len(), 1);
            assert!(!runs[0].contains("--use-extractors"));
            assert!(runs[0].contains("--proxy\nhttp://127.0.0.1:"), "{}", runs[0]);
        }

        #[test]
        fn a_failing_yt_dlp_reports_its_error() {
            let script = "#!/bin/sh\necho 'ERROR: [youtube] abc: Video unavailable' >&2\nexit 1\n";
            let e = run(script, false).0.unwrap_err().to_string();
            assert!(e.contains("Video unavailable"), "{e}");
        }

        #[test]
        fn an_opus_only_video_is_refused_before_downloading() {
            // yt-dlp would run for a minute: the refusal must not wait for
            // it (the bound is half of that, far above any start-up delay).
            let script = "#!/bin/sh\necho 'sussurro-format webm opus'\nsleep 60\n";
            let started = Instant::now();
            let e = run(script, false).0.unwrap_err().to_string();
            assert!(e.contains("opus") && e.contains("ffmpeg"), "{e}");
            assert!(
                started.elapsed() < Duration::from_secs(30),
                "{:?}",
                started.elapsed()
            );
        }

        #[test]
        fn cancel_kills_yt_dlp_and_its_children() {
            // The background `sleep` keeps stdout open: only a group kill
            // ends it. Both pids are written before the progress line, and
            // the cancel comes from that line — no timers (#187).
            let script = "#!/bin/sh\nsleep 60 &\necho $! > \"$ROOT/pids\"\n\
                          echo $$ >> \"$ROOT/pids\"\necho 'sussurro-progress 1 100 NA'\nwait\n";
            let (r, seen, dest, root) = run(script, true);
            let e = r.unwrap_err().to_string();
            assert!(e.contains("cancelled"), "{e}");
            assert_eq!(seen.len(), 1);
            let pids: Vec<libc::pid_t> = std::fs::read_to_string(root.path().join("pids"))
                .unwrap()
                .lines()
                .map(|l| l.trim().parse().unwrap())
                .collect();
            assert_eq!(pids.len(), 2, "{pids:?}");
            // yt-dlp itself was reaped by `download`; its orphaned child is
            // reaped by init/launchd once killed, which may take a moment.
            let deadline = Instant::now() + Duration::from_secs(30);
            for pid in pids {
                while running(pid) {
                    assert!(
                        Instant::now() < deadline,
                        "process {pid} survived the cancel"
                    );
                    std::thread::sleep(Duration::from_millis(10));
                }
            }
            drop(dest);
        }
    }

    /// One real video-platform link (plan §11, 0.8). Needs yt-dlp and the
    /// network: `SUSSURRO_LIVE_YTDLP=<url> cargo test live_yt_dlp -- --ignored`.
    #[test]
    #[ignore]
    fn live_yt_dlp_link() {
        let bin = find().expect("yt-dlp on PATH");
        let url = std::env::var("SUSSURRO_LIVE_YTDLP")
            .unwrap_or_else(|_| "https://www.youtube.com/watch?v=jNQXAC9IVRw".into());
        let root = tempfile::tempdir().unwrap();
        let dest = TempDownload::create(root.path(), 1).unwrap();
        let f = download(
            &bin,
            &Url::parse(&url).unwrap(),
            false,
            super::super::MAX_DOWNLOAD_BYTES,
            &dest,
            &AtomicBool::new(false),
            &mut |_| {},
        )
        .unwrap();
        assert!(f.title.is_some());
        assert!(
            crate::sources::file::FileSource::open(&f.path).is_ok(),
            "{:?}",
            f.path
        );
    }
}
