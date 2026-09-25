//! URL source (0.8, #123, plan §4.1, E10): transcribe from a link. The link
//! is fetched to a temporary file, which then goes through the ordinary
//! [`super::file::FileSource`] — the engine never sees the network.
//!
//! - **Direct media** ([`direct`]): an http(s) link to an audio or video
//!   file, downloaded with `reqwest` using the model downloads' timeouts
//!   (connect 10 s, 60 s per read — never unbounded), a size limit
//!   ([`MAX_DOWNLOAD_BYTES`]) and content sniffing to pick the decoder.
//! - **Video platforms** ([`ytdlp`]): `yt-dlp` found on PATH (or in the
//!   usual Homebrew / winget / scoop folders), not bundled (E10), asked for
//!   the m4a audio track so symphonia decodes it without ffmpeg. It is run
//!   with an argument vector, never through a shell, and **only for the
//!   known video platforms** ([`PLATFORM_HOSTS`], [`yt_dlp_allowed`]) — a
//!   pasted link to a platform, or a direct link on a platform host that
//!   turns out to be a web page. Any other web page is refused, never
//!   handed to yt-dlp's generic extractor.
//!
//! **Which addresses a link may reach.** Only `http` and `https`, no user
//! name or password in the link (it is saved in the item's `source`). Hosts
//! on this computer or the local network — loopback, private, link-local
//! (cloud metadata), CGNAT, unique-local IPv6, `localhost` — are refused
//! unless the user ticks *Allow local network addresses* for that run: a
//! pasted link must not make Sussurro probe the router or a local service.
//! [`resolve_checked`] covers IP literals and every address the host name
//! resolves to, and the connection is pinned to the addresses that were
//! checked (no DNS rebinding between check and connect):
//!
//! - direct downloads check every redirect hop themselves, with a client
//!   that ignores the system proxy (`HTTP(S)_PROXY` would otherwise take
//!   the connection past the pinning);
//! - yt-dlp does its own networking (it follows redirects, calls platform
//!   APIs and fetches media from CDNs), so the check on the pasted link
//!   alone would not cover it: every connection it makes goes through the
//!   in-process [`proxy::GuardProxy`], which applies the same check and
//!   pinning to each host it is asked for (#216). The generic extractor is
//!   excluded too (`--use-extractors default,-generic`), so a platform
//!   page's embedded or `og:video` URLs are never followed blindly.
//!   Residual: yt-dlp connects to whatever *public* hosts a platform's
//!   extractor names — the same trust as opening the video in a browser.
//!
//! **Temporary files** live in `<app data>/link-downloads/`, never in the
//! archive, named `link-<pid>-<session>.<ext>` ([`TempDownload`]). They are
//! removed when the run ends — done, failed or cancelled — and a crashed
//! run's leftovers are swept at the next start ([`sweep_stale`]), like the
//! engine's mic spools.

pub mod direct;
pub mod proxy;
pub mod ytdlp;

use anyhow::{anyhow, bail, Context, Result};
use reqwest::Url;
use serde::Serialize;
use std::net::{IpAddr, SocketAddr, ToSocketAddrs};
use std::path::{Path, PathBuf};

/// Largest download a link may produce: a few hours of WAV. Longer audio can
/// still be downloaded by hand and transcribed from the File tab.
pub const MAX_DOWNLOAD_BYTES: u64 = 2 * 1024 * 1024 * 1024;
/// Redirects followed by a direct download (each hop is checked).
pub const MAX_REDIRECTS: usize = 5;
/// Links longer than this are refused outright.
pub const MAX_URL_LEN: usize = 4096;
/// Folder of the temporary downloads, in the app data dir.
pub const TEMP_DIR: &str = "link-downloads";
/// Prefix of the temporary downloads: `link-<pid>-<session>.<ext>`.
pub const TEMP_PREFIX: &str = "link-";

/// How a link is fetched.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LinkKind {
    /// An audio or video file, or any other link: fetched directly (a web
    /// page on a known video platform falls back to yt-dlp).
    Direct,
    /// A known video platform: through yt-dlp.
    Platform,
}

/// A validated link and how it will be fetched.
#[derive(Debug, Clone, PartialEq)]
pub struct Link {
    pub url: Url,
    pub kind: LinkKind,
}

/// Validate and classify what the user pasted. Pure (no DNS).
pub fn parse_link(input: &str) -> Result<Link> {
    let url = validate_url(input)?;
    let kind = classify(&url);
    Ok(Link { url, kind })
}

/// Syntax rules: http/https only, a host, no credentials, a sane length.
/// Pure (no DNS: the address rules are in [`resolve_checked`]).
pub fn validate_url(input: &str) -> Result<Url> {
    let s = input.trim();
    if s.is_empty() {
        bail!("paste a link to an audio or video file, or to a video page");
    }
    if s.len() > MAX_URL_LEN {
        bail!("the link is too long");
    }
    let url =
        Url::parse(s).map_err(|_| anyhow!("not a valid link — it should start with https://"))?;
    match url.scheme() {
        "http" | "https" => {}
        other => bail!("only http and https links are supported, not {other}:"),
    }
    if !url.username().is_empty() || url.password().is_some() {
        bail!(
            "links with a user name or password are not supported (the link is saved in the item)"
        );
    }
    if url.host_str().is_none_or(str::is_empty) {
        bail!("the link has no host");
    }
    Ok(url)
}

/// Video platforms handled through yt-dlp (a host or any subdomain of it).
/// The only sites yt-dlp is ever run for (#216): a web page elsewhere is
/// refused.
pub const PLATFORM_HOSTS: &[&str] = &[
    "youtube.com",
    "youtu.be",
    "youtube-nocookie.com",
    "vimeo.com",
    "dailymotion.com",
    "dai.ly",
    "twitch.tv",
    "soundcloud.com",
    "mixcloud.com",
    "bandcamp.com",
    "tiktok.com",
    "twitter.com",
    "x.com",
    "facebook.com",
    "fb.watch",
    "instagram.com",
    "rumble.com",
    "bilibili.com",
    "ted.com",
    "streamable.com",
    "odysee.com",
];

/// File extensions taken as a direct media link.
pub const MEDIA_EXTENSIONS: &[&str] = &[
    "mp3", "wav", "wave", "m4a", "m4b", "aac", "mp4", "m4v", "mov", "flac", "ogg", "oga", "opus",
    "webm", "mkv",
];

fn is_platform_host(host: &str) -> bool {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    PLATFORM_HOSTS
        .iter()
        .any(|d| host == *d || host.ends_with(&format!(".{d}")))
}

/// Whether yt-dlp may be run for `url`: its host is a known video platform
/// ([`PLATFORM_HOSTS`]). Pure.
pub fn yt_dlp_allowed(url: &Url) -> bool {
    url.host_str().is_some_and(is_platform_host)
}

/// The last path segment's extension, lowercase, if it looks like one.
fn path_extension(url: &Url) -> Option<String> {
    let last = url.path_segments()?.rev().find(|s| !s.is_empty())?;
    let (_, ext) = last.rsplit_once('.')?;
    let ext = ext.to_ascii_lowercase();
    (!ext.is_empty() && ext.len() <= 5 && ext.chars().all(|c| c.is_ascii_alphanumeric()))
        .then_some(ext)
}

/// Direct when the path names a media file, platform when the host is a
/// known video platform, direct otherwise. Pure.
pub fn classify(url: &Url) -> LinkKind {
    if path_extension(url).is_some_and(|e| MEDIA_EXTENSIONS.contains(&e.as_str())) {
        return LinkKind::Direct;
    }
    match url.host_str() {
        Some(h) if is_platform_host(h) => LinkKind::Platform,
        _ => LinkKind::Direct,
    }
}

// ---- address rules ---------------------------------------------------------

/// An address on this computer or a local/special network: loopback,
/// private, link-local (incl. 169.254.169.254 cloud metadata), CGNAT,
/// unspecified, broadcast, multicast, documentation, benchmarking,
/// reserved; for IPv6 also unique-local and site-local, and IPv4-mapped or
/// NAT64 forms of any of those. Pure.
pub fn is_local_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let o = v4.octets();
            v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_unspecified()
                || v4.is_broadcast()
                || v4.is_multicast()
                || v4.is_documentation()
                || o[0] == 0
                || (o[0] == 100 && (o[1] & 0xc0) == 64) // 100.64.0.0/10 CGNAT
                || (o[0] == 192 && o[1] == 0 && o[2] == 0) // 192.0.0.0/24
                || (o[0] == 198 && (o[1] & 0xfe) == 18) // 198.18.0.0/15
                || o[0] >= 240 // reserved
        }
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_local_ip(IpAddr::V4(v4));
            }
            let s = v6.segments();
            // NAT64 (64:ff9b::/96) embeds an IPv4 address.
            if s[0] == 0x64 && s[1] == 0xff9b && s[2..6] == [0, 0, 0, 0] {
                let [a, b] = s[6].to_be_bytes();
                let [c, d] = s[7].to_be_bytes();
                return is_local_ip(IpAddr::from([a, b, c, d]));
            }
            v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_multicast()
                || (s[0] & 0xfe00) == 0xfc00 // unique local
                || (s[0] & 0xffc0) == 0xfe80 // link local
                || (s[0] & 0xffc0) == 0xfec0 // site local (deprecated)
                || (s[0] == 0x2001 && s[1] == 0x0db8) // documentation
        }
    }
}

/// The URL's host as an IP address, when it is a literal. Pure.
fn literal_ip(url: &Url) -> Option<IpAddr> {
    literal_host(url.host_str()?)
}

fn literal_host(host: &str) -> Option<IpAddr> {
    let host = host
        .strip_prefix('[')
        .and_then(|h| h.strip_suffix(']'))
        .unwrap_or(host);
    host.parse().ok()
}

fn is_local_name(host: &str) -> bool {
    let h = host.trim_end_matches('.').to_ascii_lowercase();
    h == "localhost" || h.ends_with(".localhost")
}

/// Whether the link visibly points to this computer or the local network
/// (an IP literal or `localhost`) — known without DNS. Pure.
pub fn is_visibly_local(url: &Url) -> bool {
    literal_ip(url).is_some_and(is_local_ip) || url.host_str().is_some_and(is_local_name)
}

/// A link refused by the address rules of [`resolve_checked`]: its host is
/// on this computer or the local network. Typed so other users of the
/// shared client (the calendar link, #252) can word it their way.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalAddressRefused {
    pub host: String,
    pub ip: Option<IpAddr>,
}

impl std::fmt::Display for LocalAddressRefused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let at = self
            .ip
            .filter(|ip| literal_host(&self.host) != Some(*ip))
            .map(|ip| format!(" ({ip})"))
            .unwrap_or_default();
        write!(
            f,
            "{} is on this computer or the local network{at} — tick “Allow local network \
             addresses” to transcribe from it",
            self.host
        )
    }
}

impl std::error::Error for LocalAddressRefused {}

fn local_refused(host: &str, ip: Option<IpAddr>) -> anyhow::Error {
    LocalAddressRefused {
        host: host.to_string(),
        ip,
    }
    .into()
}

/// Resolve the link's host and apply the address rules: unless
/// `allow_local`, refuse a local host name, IP literal, or a name with
/// **any** local address. Returns the checked addresses, to pin the
/// connection to them.
pub fn resolve_checked(url: &Url, allow_local: bool) -> Result<Vec<SocketAddr>> {
    let host = url
        .host_str()
        .ok_or_else(|| anyhow!("the link has no host"))?;
    let port = url.port_or_known_default().unwrap_or(80);
    if !allow_local && is_local_name(host) {
        return Err(local_refused(host, None));
    }
    let addrs: Vec<SocketAddr> = match literal_ip(url) {
        Some(ip) => vec![SocketAddr::new(ip, port)],
        None => (host, port)
            .to_socket_addrs()
            .with_context(|| format!("could not find the server {host}"))?
            .collect(),
    };
    if addrs.is_empty() {
        bail!("could not find the server {host}");
    }
    if !allow_local {
        if let Some(a) = addrs.iter().find(|a| is_local_ip(a.ip())) {
            return Err(local_refused(host, Some(a.ip())));
        }
    }
    Ok(addrs)
}

// ---- labels ------------------------------------------------------------------

/// `url:<link>` for the archive frontmatter (without the `#fragment`).
pub fn source_label(url: &Url) -> String {
    let mut u = url.clone();
    u.set_fragment(None);
    format!("url:{u}")
}

/// A short label for the UI: host (no `www.`) and decoded path, at most 60
/// chars.
pub fn display_label(url: &Url) -> String {
    let host = url.host_str().unwrap_or_default();
    let host = host.strip_prefix("www.").unwrap_or(host);
    let mut s = format!("{host}{}", percent_decode(url.path().trim_end_matches('/')));
    if let Some(q) = url.query() {
        s.push('?');
        s.push_str(q);
    }
    if s.chars().count() > 60 {
        s = s.chars().take(59).collect::<String>() + "…";
    }
    s
}

/// Decode `%XX` escapes (invalid UTF-8 becomes U+FFFD). Pure.
fn percent_decode(s: &str) -> String {
    let hex = |c: u8| (c as char).to_digit(16).map(|d| d as u8);
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let (Some(h), Some(l)) = (hex(b[i + 1]), hex(b[i + 2])) {
                out.push(h << 4 | l);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Title when neither the user nor yt-dlp gave one: the file name without a
/// media extension (like the File tab's file stem), else the host. Pure.
pub fn title_from_url(url: &Url) -> String {
    let last = url
        .path_segments()
        .and_then(|mut s| s.rfind(|p| !p.is_empty()))
        .map(percent_decode)
        .unwrap_or_default();
    let stem = match last.rsplit_once('.') {
        Some((stem, ext)) if MEDIA_EXTENSIONS.contains(&ext.to_ascii_lowercase().as_str()) => {
            stem.to_string()
        }
        _ => last,
    };
    let stem = stem.trim();
    if stem.is_empty() {
        let host = url.host_str().unwrap_or("Link");
        host.strip_prefix("www.").unwrap_or(host).to_string()
    } else {
        stem.to_string()
    }
}

// ---- content sniffing --------------------------------------------------------

/// The container from the first bytes of a download. Pure.
pub fn sniff_extension(head: &[u8]) -> Option<&'static str> {
    if head.len() >= 12 && &head[0..4] == b"RIFF" && &head[8..12] == b"WAVE" {
        return Some("wav");
    }
    if head.starts_with(b"fLaC") {
        return Some("flac");
    }
    if head.starts_with(b"OggS") {
        return Some("ogg");
    }
    if head.starts_with(b"ID3") {
        return Some("mp3");
    }
    if head.len() >= 8 && &head[4..8] == b"ftyp" {
        return Some("m4a");
    }
    if head.starts_with(&[0x1a, 0x45, 0xdf, 0xa3]) {
        return Some("webm");
    }
    if head.len() >= 2 && head[0] == 0xff && (head[1] & 0xe0) == 0xe0 {
        // MPEG sync word: layer bits 00 = ADTS AAC, else MPEG audio.
        return Some(if head[1] & 0x06 == 0 { "aac" } else { "mp3" });
    }
    None
}

/// The container from a `Content-Type`, when it names one. Pure.
pub fn extension_for_content_type(content_type: &str) -> Option<&'static str> {
    let mime = content_type
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    Some(match mime.as_str() {
        "audio/mpeg" | "audio/mp3" | "audio/mpeg3" | "audio/x-mpeg" => "mp3",
        "audio/wav" | "audio/x-wav" | "audio/wave" | "audio/vnd.wave" => "wav",
        "audio/mp4" | "audio/x-m4a" | "audio/m4a" | "video/mp4" | "video/quicktime"
        | "audio/x-m4b" => "m4a",
        "audio/aac" | "audio/x-aac" | "audio/aacp" => "aac",
        "audio/flac" | "audio/x-flac" => "flac",
        "audio/ogg" | "application/ogg" | "audio/vorbis" => "ogg",
        "audio/opus" => "opus",
        "audio/webm" | "video/webm" | "video/x-matroska" | "audio/x-matroska" => "webm",
        _ => return None,
    })
}

/// A web page rather than media: an HTML content type or HTML bytes. Pure.
pub fn is_web_page(content_type: Option<&str>, head: &[u8]) -> bool {
    let html_type = content_type.is_some_and(|ct| {
        let ct = ct.to_ascii_lowercase();
        ct.starts_with("text/html") || ct.starts_with("application/xhtml")
    });
    if html_type {
        return true;
    }
    let start: String = String::from_utf8_lossy(&head[..head.len().min(256)])
        .trim_start_matches('\u{feff}')
        .trim_start()
        .chars()
        .take(15)
        .collect::<String>()
        .to_ascii_lowercase();
    start.starts_with("<!doctype html") || start.starts_with("<html")
}

/// The extension the temporary file gets, which is the decoder hint:
/// sniffed bytes first, then the content type, then the link's own
/// extension, else `bin` (symphonia still probes the content). Pure.
pub fn pick_extension(content_type: Option<&str>, url: &Url, head: &[u8]) -> String {
    if let Some(e) = sniff_extension(head) {
        return e.to_string();
    }
    if let Some(e) = content_type.and_then(extension_for_content_type) {
        return e.to_string();
    }
    match path_extension(url) {
        Some(e) if MEDIA_EXTENSIONS.contains(&e.as_str()) => e,
        _ => "bin".to_string(),
    }
}

/// Why a fetched file could not be opened, in words the user can act on.
pub fn undecodable(path: &Path, via_yt_dlp: bool, err: &anyhow::Error) -> anyhow::Error {
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    let what = if ext.is_empty() || ext == "bin" {
        "an audio format".to_string()
    } else {
        format!("{ext} audio")
    };
    let hint = if via_yt_dlp {
        "This video has no m4a audio track, and Sussurro does not bundle ffmpeg to convert it."
    } else {
        "Convert it to mp3 or m4a and use the File tab."
    };
    anyhow!(
        "the link delivered {what} that Sussurro can't decode ({err:#}). Supported: mp3, \
         m4a/aac, wav, flac, ogg vorbis. {hint}"
    )
}

/// "The file is larger than the 2 GB limit for links (2.4 GB)."
pub fn too_large(size: Option<u64>, max: u64) -> anyhow::Error {
    let gb = |b: u64| format!("{:.1} GB", b as f64 / (1u64 << 30) as f64);
    let size = size.map(|s| format!(" ({})", gb(s))).unwrap_or_default();
    anyhow!(
        "the file is larger than the {} limit for links{size} — download it and use the File tab",
        gb(max)
    )
}

// ---- temporary files -------------------------------------------------------

/// A run's temporary download(s): `<dir>/link-<pid>-<session>.*`. Every
/// such file (incl. yt-dlp's `.part`) is removed on drop, so a run that
/// fails or is cancelled leaves nothing behind.
#[derive(Debug)]
pub struct TempDownload {
    dir: PathBuf,
    stem: String,
}

impl TempDownload {
    pub fn create(dir: &Path, session_id: u64) -> Result<Self> {
        std::fs::create_dir_all(dir)
            .with_context(|| format!("creating the download folder {}", dir.display()))?;
        Ok(Self {
            dir: dir.to_path_buf(),
            stem: format!("{TEMP_PREFIX}{}-{session_id}", std::process::id()),
        })
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// The download's path with this extension.
    pub fn path(&self, ext: &str) -> PathBuf {
        self.dir.join(format!("{}.{ext}", self.stem))
    }

    /// yt-dlp's output template: the extension is the format's.
    pub fn template(&self) -> PathBuf {
        self.dir.join(format!("{}.%(ext)s", self.stem))
    }

    /// This run's files, sorted.
    pub fn files(&self) -> Vec<PathBuf> {
        let prefix = format!("{}.", self.stem);
        let mut out: Vec<PathBuf> = std::fs::read_dir(&self.dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().starts_with(&prefix))
            .map(|e| e.path())
            .collect();
        out.sort();
        out
    }

    /// Remove this run's files; returns how many were removed.
    pub fn remove_all(&self) -> usize {
        self.files()
            .into_iter()
            .filter(|p| std::fs::remove_file(p).is_ok())
            .count()
    }
}

impl Drop for TempDownload {
    fn drop(&mut self) {
        self.remove_all();
    }
}

/// `link-<pid>-<session>.<ext>` → `pid`.
fn temp_pid(name: &str) -> Option<u32> {
    name.strip_prefix(TEMP_PREFIX)?
        .split('-')
        .next()?
        .parse()
        .ok()
}

/// At app start: delete the downloads of dead processes (a crash or forced
/// quit mid-run), leaving `current_pid`'s and a running second instance's.
/// Returns how many files were removed.
pub fn sweep_stale(dir: &Path, current_pid: u32) -> usize {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    let mut removed = 0;
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let Some(pid) = temp_pid(&name) else {
            continue;
        };
        if pid == current_pid || crate::engine::checkpoint::process_alive(pid) {
            continue;
        }
        if e.file_type().is_ok_and(|t| t.is_file()) && std::fs::remove_file(e.path()).is_ok() {
            removed += 1;
        }
    }
    removed
}

// ---- progress ----------------------------------------------------------------

/// Download progress reported to the run (and on to the UI).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Progress {
    pub downloaded: u64,
    /// Known (or estimated by yt-dlp) total size.
    pub total: Option<u64>,
    /// The video's title, once yt-dlp has read it.
    pub title: Option<String>,
}

// ---- fetching ----------------------------------------------------------------

/// How a link was fetched.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Via {
    Direct,
    YtDlp,
}

/// A link fetched to a temporary file, ready for the file source.
#[derive(Debug)]
pub struct Fetched {
    pub path: PathBuf,
    /// The platform's title (yt-dlp), if any.
    pub title: Option<String>,
    pub via: Via,
}

/// The error for a web page that is not on a known video platform.
fn not_a_platform_page(url: &Url) -> anyhow::Error {
    anyhow!(
        "the link opens a web page on {}, not an audio or video file. Sussurro fetches videos \
         from web pages only on known video sites (YouTube, Vimeo, SoundCloud, …); for this \
         page, link the audio or video file itself",
        url.host_str().unwrap_or("the site")
    )
}

/// Fetch `link` into `dest`: a direct download, or yt-dlp for a video
/// platform — and for a direct link on a platform host that turns out to
/// be a web page, when yt-dlp is installed. Other web pages are refused
/// (#216). `find_yt_dlp` locates the binary ([`ytdlp::find`] in the app);
/// `progress` hears which way the download goes and how far.
pub fn fetch(
    link: &Link,
    allow_local: bool,
    max_bytes: u64,
    dest: &TempDownload,
    cancel: &std::sync::atomic::AtomicBool,
    find_yt_dlp: &dyn Fn() -> Option<PathBuf>,
    progress: &mut dyn FnMut(Via, &Progress),
) -> Result<Fetched> {
    let with_yt_dlp = |bin: PathBuf, progress: &mut dyn FnMut(Via, &Progress)| {
        if !yt_dlp_allowed(&link.url) {
            return Err(not_a_platform_page(&link.url));
        }
        // A fast, clear refusal before starting it; the guard proxy then
        // checks every connection it makes.
        resolve_checked(&link.url, allow_local)?;
        progress(Via::YtDlp, &Progress::default());
        let f = ytdlp::download(
            &bin,
            &link.url,
            allow_local,
            max_bytes,
            dest,
            cancel,
            &mut |p| progress(Via::YtDlp, p),
        )?;
        Ok(Fetched {
            path: f.path,
            title: f.title,
            via: Via::YtDlp,
        })
    };
    match link.kind {
        LinkKind::Platform => {
            let bin = find_yt_dlp().ok_or_else(ytdlp::missing_error)?;
            with_yt_dlp(bin, progress)
        }
        LinkKind::Direct => {
            progress(Via::Direct, &Progress::default());
            match direct::download(&link.url, allow_local, max_bytes, dest, cancel, &mut |p| {
                progress(Via::Direct, p)
            }) {
                Ok(d) => Ok(Fetched {
                    path: d.path,
                    title: None,
                    via: Via::Direct,
                }),
                Err(e) if e.is::<direct::WebPage>() && !yt_dlp_allowed(&link.url) => {
                    Err(not_a_platform_page(&link.url))
                }
                Err(e) if e.is::<direct::WebPage>() => match find_yt_dlp() {
                    Some(bin) => with_yt_dlp(bin, progress),
                    None => Err(anyhow!(
                        "the link opens a web page, not an audio or video file. If the page \
                         holds a video, yt-dlp can fetch its audio, but it was not found. {}",
                        ytdlp::install_instructions()
                    )),
                },
                Err(e) => Err(e),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(s: &str) -> Url {
        Url::parse(s).unwrap()
    }

    #[test]
    fn only_http_and_https_links_with_a_host_are_accepted() {
        assert!(validate_url(" https://example.com/a.mp3 ").is_ok());
        assert!(validate_url("http://example.com/a.mp3").is_ok());
        for bad in [
            "",
            "   ",
            "example.com/a.mp3",
            "ftp://example.com/a.mp3",
            "file:///etc/passwd",
            "javascript:alert(1)",
            "data:audio/wav;base64,AAAA",
            "https://",
            "https://user:secret@example.com/a.mp3",
            "https://user@example.com/a.mp3",
        ] {
            assert!(validate_url(bad).is_err(), "{bad:?} must be refused");
        }
        let long = format!("https://example.com/{}", "a".repeat(MAX_URL_LEN));
        assert!(validate_url(&long).is_err());
        let e = validate_url("ftp://example.com/x").unwrap_err().to_string();
        assert!(e.contains("http"), "{e}");
    }

    #[test]
    fn links_are_classified_direct_or_platform() {
        let kind = |s: &str| parse_link(s).unwrap().kind;
        assert_eq!(
            kind("https://www.youtube.com/watch?v=dQw4w9WgXcQ"),
            LinkKind::Platform
        );
        assert_eq!(kind("https://youtu.be/dQw4w9WgXcQ"), LinkKind::Platform);
        assert_eq!(kind("https://m.youtube.com/watch?v=x"), LinkKind::Platform);
        assert_eq!(kind("https://vimeo.com/123456"), LinkKind::Platform);
        assert_eq!(
            kind("https://soundcloud.com/artist/track"),
            LinkKind::Platform
        );
        assert_eq!(kind("https://cdn.example.com/ep12.mp3"), LinkKind::Direct);
        assert_eq!(kind("https://example.com/talk.M4A?sig=1"), LinkKind::Direct);
        // A media file on a platform host is still a file.
        assert_eq!(kind("https://x.com/media/clip.mp4"), LinkKind::Direct);
        // Unknown pages are tried directly (a web page there is refused).
        assert_eq!(kind("https://example.com/episodes/12"), LinkKind::Direct);
        // Look-alike hosts are not platforms.
        assert_eq!(kind("https://notyoutube.com/watch?v=x"), LinkKind::Direct);
        assert_eq!(
            kind("https://youtube.com.evil.example/watch"),
            LinkKind::Direct
        );
    }

    #[test]
    fn yt_dlp_runs_only_for_known_platform_hosts() {
        for ok in [
            "https://www.youtube.com/watch?v=x",
            "https://youtu.be/x",
            "https://player.vimeo.com/video/1",
            "https://x.com/media/clip.mp4",
            "https://SoundCloud.com./a/b",
        ] {
            assert!(yt_dlp_allowed(&url(ok)), "{ok}");
        }
        for no in [
            "https://example.com/episodes/12",
            "https://notyoutube.com/watch?v=x",
            "https://youtube.com.evil.example/watch",
            "http://192.168.1.10/video",
            "http://localhost/watch?v=x",
            "http://169.254.169.254/latest/meta-data",
        ] {
            assert!(!yt_dlp_allowed(&url(no)), "{no}");
        }
    }

    /// #216: a web page that is not on a video platform never reaches
    /// yt-dlp (and its generic extractor), even when it is installed.
    #[test]
    fn a_web_page_elsewhere_is_refused_without_running_yt_dlp() {
        use crate::sources::url::direct::tests::serve;
        let srv = serve(vec![(
            "/page",
            (
                200,
                vec![("Content-Type", "text/html".into())],
                b"<html><video src=\"http://192.168.1.1/x.mp4\"></video></html>".to_vec(),
            ),
        )]);
        let dir = tempfile::tempdir().unwrap();
        let dest = TempDownload::create(dir.path(), 7).unwrap();
        let link = parse_link(&format!("{}/page", srv.base)).unwrap();
        assert_eq!(link.kind, LinkKind::Direct);
        let asked = std::cell::Cell::new(false);
        let find = || {
            asked.set(true);
            Some(PathBuf::from("/nonexistent/yt-dlp"))
        };
        let e = fetch(
            &link,
            true,
            MAX_DOWNLOAD_BYTES,
            &dest,
            &std::sync::atomic::AtomicBool::new(false),
            &find,
            &mut |_, _| {},
        )
        .unwrap_err()
        .to_string();
        assert!(e.contains("known video sites"), "{e}");
        assert!(!asked.get(), "yt-dlp was not even looked for");
    }

    #[test]
    fn local_and_special_addresses_are_recognized() {
        for ip in [
            "127.0.0.1",
            "10.1.2.3",
            "172.16.0.1",
            "192.168.1.1",
            "169.254.169.254",
            "100.64.0.1",
            "0.0.0.0",
            "255.255.255.255",
            "224.0.0.1",
            "198.18.0.1",
            "::1",
            "::",
            "fc00::1",
            "fd12:3456::1",
            "fe80::1",
            "::ffff:127.0.0.1",
            "::ffff:192.168.0.1",
            "64:ff9b::7f00:1",
        ] {
            assert!(is_local_ip(ip.parse().unwrap()), "{ip} is local");
        }
        for ip in [
            "93.184.216.34",
            "8.8.8.8",
            "2606:4700::6810:85e5",
            "::ffff:8.8.8.8",
        ] {
            assert!(!is_local_ip(ip.parse().unwrap()), "{ip} is public");
        }
    }

    #[test]
    fn local_hosts_are_refused_unless_allowed() {
        for s in [
            "http://127.0.0.1:8080/a.wav",
            "http://[::1]/a.wav",
            "http://localhost/a.wav",
            "http://api.localhost/a.wav",
            "http://192.168.1.10/a.wav",
            "http://169.254.169.254/latest/meta-data",
            // Alternative IPv4 spellings are normalized by the URL parser.
            "http://0x7f.1/a.wav",
            "http://2130706433/a.wav",
        ] {
            let u = url(s);
            assert!(is_visibly_local(&u), "{s}");
            let e = resolve_checked(&u, false).unwrap_err().to_string();
            assert!(e.contains("Allow local network addresses"), "{s}: {e}");
        }
        // Allowed explicitly: resolved and returned for pinning.
        let addrs = resolve_checked(&url("http://127.0.0.1:8080/a.wav"), true).unwrap();
        assert_eq!(addrs, ["127.0.0.1:8080".parse::<SocketAddr>().unwrap()]);
        // A public literal needs no DNS and passes.
        let addrs = resolve_checked(&url("https://93.184.216.34/a.wav"), false).unwrap();
        assert_eq!(addrs[0].port(), 443);
        assert!(!is_visibly_local(&url("https://example.com/a.wav")));
    }

    #[test]
    fn labels_and_titles_come_from_the_link() {
        let u = url("https://www.example.com/podcast/Episodio%2012%20-%20Intervista.mp3?x=1#t=30");
        assert_eq!(
            source_label(&u),
            "url:https://www.example.com/podcast/Episodio%2012%20-%20Intervista.mp3?x=1"
        );
        assert_eq!(title_from_url(&u), "Episodio 12 - Intervista");
        assert_eq!(
            title_from_url(&url("https://example.com/talks/keynote/")),
            "keynote"
        );
        assert_eq!(
            title_from_url(&url("https://www.example.com/")),
            "example.com"
        );
        assert_eq!(title_from_url(&url("https://example.com/a%ZZb")), "a%ZZb");
        assert_eq!(
            display_label(&url("https://www.youtube.com/watch?v=dQw4w9WgXcQ")),
            "youtube.com/watch?v=dQw4w9WgXcQ"
        );
        assert_eq!(
            display_label(&url("https://example.com/podcast/Episodio%2012.mp3")),
            "example.com/podcast/Episodio 12.mp3"
        );
        let long = display_label(&url(&format!("https://example.com/{}", "x".repeat(100))));
        assert_eq!(long.chars().count(), 60);
        assert!(long.ends_with('…'));
    }

    #[test]
    fn content_is_sniffed_to_pick_the_decoder() {
        let mut wav = b"RIFF\0\0\0\0WAVEfmt ".to_vec();
        wav.extend([0; 8]);
        let mut m4a = vec![0, 0, 0, 0x20];
        m4a.extend(b"ftypM4A ");
        let plain = url("https://example.com/download?id=3");
        assert_eq!(
            pick_extension(Some("application/octet-stream"), &plain, &wav),
            "wav"
        );
        assert_eq!(pick_extension(None, &plain, &m4a), "m4a");
        assert_eq!(pick_extension(None, &plain, b"ID3\x04"), "mp3");
        assert_eq!(pick_extension(None, &plain, &[0xff, 0xfb, 0x90]), "mp3");
        assert_eq!(pick_extension(None, &plain, &[0xff, 0xf1, 0x50]), "aac");
        assert_eq!(pick_extension(None, &plain, b"fLaC"), "flac");
        assert_eq!(pick_extension(None, &plain, b"OggS"), "ogg");
        assert_eq!(
            pick_extension(None, &plain, &[0x1a, 0x45, 0xdf, 0xa3]),
            "webm"
        );
        // Unknown bytes: the content type, then the link's extension.
        assert_eq!(pick_extension(Some("audio/mpeg"), &plain, b"????"), "mp3");
        assert_eq!(
            pick_extension(Some("audio/x-m4a; x=1"), &plain, b"????"),
            "m4a"
        );
        assert_eq!(
            pick_extension(None, &url("https://e.com/a.FLAC"), b"????"),
            "flac"
        );
        assert_eq!(pick_extension(None, &plain, b"????"), "bin");

        assert!(is_web_page(Some("text/html; charset=utf-8"), b""));
        assert!(is_web_page(None, b"\n  <!DOCTYPE html><html>"));
        assert!(is_web_page(
            Some("application/octet-stream"),
            b"<html><body>"
        ));
        assert!(!is_web_page(Some("audio/mpeg"), b"ID3"));
    }

    #[test]
    fn temp_downloads_are_removed_on_drop_and_swept_after_a_crash() {
        let dir = tempfile::tempdir().unwrap();
        let temp = TempDownload::create(dir.path(), 4).unwrap();
        std::fs::write(temp.path("m4a"), b"a").unwrap();
        std::fs::write(temp.path("m4a.part"), b"b").unwrap();
        // Another session of this process (`-45` must not match `-4.`).
        let other = dir
            .path()
            .join(format!("link-{}-45.mp3", std::process::id()));
        std::fs::write(&other, b"c").unwrap();
        assert_eq!(temp.files().len(), 2);
        assert!(temp.template().to_string_lossy().ends_with("-4.%(ext)s"));
        drop(temp);
        assert!(other.exists());
        let left: Vec<_> = std::fs::read_dir(dir.path()).unwrap().flatten().collect();
        assert_eq!(left.len(), 1);

        // Sweep: a dead process's files go; ours, a live one's and
        // unrelated files stay.
        let dead = dir.path().join("link-999999999-1.wav");
        let dead_part = dir.path().join("link-999999999-1.m4a.part");
        let unrelated = dir.path().join("notes.txt");
        for p in [&dead, &dead_part, &unrelated] {
            std::fs::write(p, b"x").unwrap();
        }
        // A second instance still running (here: our parent process).
        #[cfg(unix)]
        let live = {
            let p = dir.path().join(format!(
                "link-{}-2.wav",
                std::os::unix::process::parent_id()
            ));
            std::fs::write(&p, b"x").unwrap();
            p
        };
        assert_eq!(sweep_stale(dir.path(), std::process::id()), 2);
        assert!(!dead.exists() && !dead_part.exists());
        assert!(other.exists() && unrelated.exists());
        #[cfg(unix)]
        assert!(live.exists());
        assert_eq!(sweep_stale(&dir.path().join("missing"), 1), 0);
    }

    #[test]
    fn size_and_format_errors_are_explained() {
        let e = too_large(Some(3_000_000_000), MAX_DOWNLOAD_BYTES).to_string();
        assert!(
            e.contains("2.0 GB limit") && e.contains("2.8 GB") && e.contains("File tab"),
            "{e}"
        );
        let e =
            undecodable(Path::new("/t/link-1-2.webm"), true, &anyhow!("no decoder")).to_string();
        assert!(
            e.contains("webm audio") && e.contains("m4a") && e.contains("ffmpeg"),
            "{e}"
        );
    }
}
