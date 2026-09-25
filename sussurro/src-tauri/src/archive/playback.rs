//! Serving an item's saved audio to the *Audio* tab (0.10, #142).
//!
//! The player is a plain HTML `<audio>` element. Its source is the custom
//! URI scheme [`SCHEME`], answered here with HTTP range support, so a
//! one-hour WAV (~115 MB) is streamed in [`MAX_CHUNK`] pieces as the element
//! asks for them — never read whole, never copied through `invoke` as bytes.
//!
//! **URL** — what `convertFileSrc("<id>/<file>", "sussurro-audio")` builds:
//!
//! ```text
//! sussurro-audio://localhost/<percent-encoded "<item id>/<file name>">   macOS, Linux
//! http://sussurro-audio.localhost/<same>                                 Windows
//! ```
//!
//! **Confinement** — the URL names an item and a file, never a path: the id
//! goes through the archive's own id validation and folder confinement
//! ([`super::store::existing_item_dir`]), the file name must be one of the
//! audio names of #141 ([`is_audio_file_name`]: `audio.wav` or
//! `audio-<letters>.wav`, and their `.opus` twins of #247), and the file
//! must be a regular file (not a link)
//! directly inside that item folder. Nothing else in the archive, and
//! nothing outside it, can be read through the scheme.
//!
//! **Why a scheme and not Tauri's asset protocol**: the asset protocol
//! serves any path its scope allows, and the archive can be moved anywhere
//! (an Obsidian vault, a synced folder); scoping it would mean granting the
//! whole archive folder, transcripts included, to every `<img>`/`<audio>`
//! of the page. This scheme serves audio files of archive items and nothing
//! else. The CSP gains exactly one directive for it:
//! `media-src sussurro-audio: http://sussurro-audio.localhost` (#91 stays
//! otherwise untouched: scripts, styles, connections and images unchanged).
//!
//! **Opus is served as WAV** (#248, E15): the WebView never sees Ogg Opus
//! (macOS before 15.4 can't play it, and 15.4+ only estimates its
//! duration). An `.opus` file is answered as a *virtual* WAV — the 44-byte
//! header of its exact decoded length, then 16-bit PCM — and a byte range
//! maps to a sample range that [`OpusReader`] decodes from its page index.
//! A reader is kept per file ([`OPUS_CACHE`], a few entries, the file
//! closed between requests), so the element's consecutive ranges decode on
//! from where the last one stopped (bit-exact, about 50 ms per 1 MiB); a
//! jump restarts 200 ms before the target. Every item plays through the
//! same `audio/wav` path with an exact duration and sample-exact seeks.
//!
//! **Without a `Range` header** a request gets the whole resource (`200`),
//! up to [`MAX_WHOLE`] bytes; a bigger one is refused (`413`) rather than
//! answered with part of it: the webview's scheme API takes whole bodies
//! only, so a stream isn't possible, and media elements send ranges (a
//! `206` to a request without one, what this answered before, made WebKit
//! stop playing after the first MiB).

use super::audio::{is_audio_file_name, AudioFormat};
use super::opus::OpusReader;
use anyhow::{bail, Context, Result};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// The URI scheme of saved audio.
pub const SCHEME: &str = "sussurro-audio";
/// Largest body of one ranged response. An open-ended range (`bytes=0-`,
/// what media elements send) gets this much; the element asks for the rest
/// as it plays. 1 MiB is about 33 s of 16 kHz 16-bit mono.
pub const MAX_CHUNK: u64 = 1024 * 1024;
/// Largest resource sent whole to a request without a `Range` header:
/// 128 MiB, a WAV of about 70 minutes (see the module docs).
pub const MAX_WHOLE: u64 = 128 * 1024 * 1024;
/// Every response is a WAV, including decoded Opus.
const CONTENT_TYPE: &str = "audio/wav";
/// Opus readers kept between requests: two channels of two items.
const OPUS_CACHE_SIZE: usize = 4;

/// Decode `%XX` escapes (UTF-8). `+` is kept as is (a path, not a form).
/// Refuses malformed escapes, invalid UTF-8 and NUL. Pure.
pub fn percent_decode(s: &str) -> Result<String> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = bytes
                .get(i + 1..i + 3)
                .and_then(|h| std::str::from_utf8(h).ok())
                .and_then(|h| u8::from_str_radix(h, 16).ok());
            match hex {
                Some(b) => out.push(b),
                None => bail!("malformed escape in '{s}'"),
            }
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    let decoded = String::from_utf8(out).context("not UTF-8")?;
    if decoded.contains('\0') {
        bail!("NUL in path");
    }
    Ok(decoded)
}

/// The request path (`/<encoded "id/file">`) → `(item id, file name)`. The
/// split is at the last `/`, since ids contain slashes (`2026/09/…`) and
/// file names never do. Pure.
pub fn parse_path(path: &str) -> Result<(String, String)> {
    let decoded = percent_decode(path.trim_start_matches('/'))?;
    let Some((id, file)) = decoded.rsplit_once('/') else {
        bail!("expected <item id>/<file name>");
    };
    if id.is_empty() || !is_audio_file_name(file) {
        bail!("'{decoded}' is not an item's audio file");
    }
    Ok((id.to_string(), file.to_string()))
}

/// The audio file `file` of item `id`, confined to the item folder (see the
/// module docs). Errors for anything else, including a missing file.
pub fn resolve(archive: &Path, id: &str, file: &str) -> Result<PathBuf> {
    if !is_audio_file_name(file) {
        bail!("'{file}' is not an audio file name");
    }
    let dir = super::store::existing_item_dir(archive, id)?;
    let path = dir.join(file);
    let meta =
        std::fs::symlink_metadata(&path).with_context(|| format!("no audio '{file}' in '{id}'"))?;
    if !meta.file_type().is_file() {
        bail!("'{file}' in '{id}' is not a regular file");
    }
    let canon_dir = dir.canonicalize()?;
    let canon = path.canonicalize()?;
    if canon.parent() != Some(canon_dir.as_path()) {
        bail!("'{file}' resolves outside the item folder");
    }
    Ok(path)
}

/// What a `Range` header asks of a file of `len` bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RangeAsk {
    /// No usable range: the whole file (the header is absent or malformed —
    /// RFC 9110 says to ignore a range the server can't parse).
    Whole,
    /// Bytes `start..=end`, already clamped to the file and capped at
    /// [`MAX_CHUNK`].
    Part { start: u64, end: u64 },
    /// Nothing of the file (416).
    Unsatisfiable,
}

/// Parse `Range: bytes=…` against a file of `len` bytes: `a-b`, `a-` and
/// the suffix form `-n`. With several ranges only the first is served (a
/// media element never asks for more). Pure.
pub fn parse_range(header: Option<&str>, len: u64) -> RangeAsk {
    let Some(h) = header else {
        return RangeAsk::Whole;
    };
    let Some(spec) = h.trim().strip_prefix("bytes=") else {
        return RangeAsk::Whole;
    };
    let first = spec.split(',').next().unwrap_or("").trim();
    let Some((a, b)) = first.split_once('-') else {
        return RangeAsk::Whole;
    };
    let num = |s: &str| -> Option<u64> {
        let s = s.trim();
        if s.is_empty() || !s.bytes().all(|c| c.is_ascii_digit()) {
            None
        } else {
            s.parse().ok()
        }
    };
    let (start, end) = match (a.trim().is_empty(), num(a), num(b)) {
        // `-n`: the last n bytes.
        (true, _, Some(n)) => {
            if n == 0 || len == 0 {
                return RangeAsk::Unsatisfiable;
            }
            (len - n.min(len), len - 1)
        }
        (false, Some(start), end) => {
            if !b.trim().is_empty() && end.is_none() {
                return RangeAsk::Whole;
            }
            if start >= len {
                return RangeAsk::Unsatisfiable;
            }
            match end {
                Some(e) if e < start => return RangeAsk::Whole,
                Some(e) => (start, e.min(len - 1)),
                None => (start, len - 1),
            }
        }
        _ => return RangeAsk::Whole,
    };
    RangeAsk::Part {
        start,
        end: end.min(start + MAX_CHUNK - 1),
    }
}

/// A response, independent of the webview's HTTP types (tests read it).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reply {
    pub status: u16,
    pub headers: Vec<(&'static str, String)>,
    pub body: Vec<u8>,
}

impl Reply {
    fn error(status: u16, msg: &str) -> Self {
        Self {
            status,
            headers: vec![("Content-Type", "text/plain; charset=utf-8".into())],
            body: msg.as_bytes().to_vec(),
        }
    }

    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

fn read_span(path: &Path, start: u64, len: u64) -> std::io::Result<Vec<u8>> {
    let mut f = std::fs::File::open(path)?;
    f.seek(SeekFrom::Start(start))?;
    let mut buf = Vec::with_capacity(len as usize);
    f.take(len).read_to_end(&mut buf)?;
    Ok(buf)
}

/// Answer a request with `range` (the `Range` header) on a resource of
/// `len` bytes whose bytes `read(start, count)` returns. `head`: headers
/// only. Without a usable range: the whole resource up to [`MAX_WHOLE`],
/// else 413 (see the module docs).
fn serve(
    len: u64,
    range: Option<&str>,
    head: bool,
    read: impl FnOnce(u64, u64) -> Result<Vec<u8>>,
) -> Reply {
    let (status, start, count) = match parse_range(range, len) {
        RangeAsk::Unsatisfiable => {
            let mut r = Reply::error(416, "range not satisfiable");
            r.headers.push(("Content-Range", format!("bytes */{len}")));
            r.headers.push(("Accept-Ranges", "bytes".into()));
            return r;
        }
        RangeAsk::Whole if len > MAX_WHOLE && !head => {
            let mut r = Reply::error(413, "too large to send whole: ask for byte ranges");
            r.headers.push(("Accept-Ranges", "bytes".into()));
            return r;
        }
        RangeAsk::Whole => (200, 0, len),
        RangeAsk::Part { start, end } => (206, start, end - start + 1),
    };
    let body = if head || count == 0 {
        Vec::new()
    } else {
        match read(start, count) {
            Ok(b) => b,
            Err(e) => {
                eprintln!("audio: read failed ({e:#})");
                return Reply::error(500, "read failed");
            }
        }
    };
    // The file may have shrunk since its length was taken (Delete audio
    // racing a request): describe what was actually read.
    let sent = if head { count } else { body.len() as u64 };
    let mut headers = vec![
        ("Content-Type", CONTENT_TYPE.to_string()),
        ("Accept-Ranges", "bytes".to_string()),
        ("Content-Length", sent.to_string()),
        // A file can still change (a session writing it, Delete audio,
        // Compress audio).
        ("Cache-Control", "no-cache".to_string()),
        ("X-Content-Type-Options", "nosniff".to_string()),
    ];
    if status == 206 {
        let last = start + sent.max(1) - 1;
        headers.push(("Content-Range", format!("bytes {start}-{last}/{len}")));
    }
    Reply {
        status,
        headers,
        body,
    }
}

/// Serve the saved audio file at `path` (a WAV as it is, an Opus file as a
/// decoded WAV) for a request with `range`; `head`: headers only.
pub fn serve_file(path: &Path, range: Option<&str>, head: bool) -> Reply {
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    if AudioFormat::of_name(name) == Some(AudioFormat::Opus) {
        return serve_opus(path, range, head);
    }
    let len = match std::fs::metadata(path) {
        Ok(m) => m.len(),
        Err(_) => return Reply::error(404, "not found"),
    };
    serve(len, range, head, |start, count| {
        Ok(read_span(path, start, count)?)
    })
}

// ---- Opus as a virtual WAV ---------------------------------------------------

/// An Opus file read as a WAV: its reader and what the file was when the
/// reader indexed it (a file still being written, or replaced, is
/// re-indexed).
struct OpusWav {
    reader: OpusReader,
    stamp: (u64, Option<std::time::SystemTime>),
}

impl OpusWav {
    /// Samples of the virtual WAV (a WAV's 32-bit sizes cap them, as the
    /// writers do).
    fn samples(&self) -> u64 {
        self.reader.total_samples().min(super::audio::MAX_SAMPLES)
    }

    fn len(&self) -> u64 {
        super::audio::HEADER_LEN + self.samples() * 2
    }

    /// Bytes `start..start + count` of the virtual WAV; the file is closed
    /// again however the read ends.
    fn read(&mut self, start: u64, count: u64) -> Result<Vec<u8>> {
        let out = self.read_open(start, count);
        self.reader.close();
        out
    }

    fn read_open(&mut self, start: u64, count: u64) -> Result<Vec<u8>> {
        let header_len = super::audio::HEADER_LEN;
        let end = (start + count).min(self.len());
        let mut out = Vec::with_capacity((end - start.min(end)) as usize);
        if start < header_len {
            let header = super::audio::header((self.samples() * 2) as u32);
            out.extend_from_slice(&header[start as usize..end.min(header_len) as usize]);
        }
        if end > header_len {
            // Data bytes d0..d1 are samples d0 / 2 up to (d1 + 1) / 2.
            let (d0, d1) = (start.max(header_len) - header_len, end - header_len);
            let (s0, s1) = (d0 / 2, d1.div_ceil(2));
            self.reader.seek(s0)?;
            let mut samples = Vec::with_capacity((s1 - s0) as usize);
            self.reader.read(&mut samples, (s1 - s0) as usize)?;
            let skip = (d0 - s0 * 2) as usize;
            let mut pcm = Vec::with_capacity(samples.len() * 2);
            for s in samples {
                pcm.extend_from_slice(&super::audio::to_i16(s).to_le_bytes());
            }
            let take = ((d1 - d0) as usize).min(pcm.len().saturating_sub(skip));
            out.extend_from_slice(&pcm[skip..skip + take]);
        }
        Ok(out)
    }
}

/// Opus readers by file, most recently used last (see the module docs).
static OPUS_CACHE: Mutex<Vec<(PathBuf, Arc<Mutex<OpusWav>>)>> = Mutex::new(Vec::new());

fn file_stamp(path: &Path) -> std::io::Result<(u64, Option<std::time::SystemTime>)> {
    let meta = std::fs::metadata(path)?;
    Ok((meta.len(), meta.modified().ok()))
}

/// The cached reader of `path`, or a new one (indexing the file) when there
/// is none or the file changed since.
fn opus_wav(path: &Path) -> Result<Arc<Mutex<OpusWav>>> {
    let stamp = file_stamp(path)?;
    {
        let mut cache = OPUS_CACHE.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(i) = cache.iter().position(|(p, _)| p == path) {
            let (p, entry) = cache.remove(i);
            let fresh = entry.lock().map(|w| w.stamp == stamp).unwrap_or(false);
            if fresh {
                cache.push((p, entry.clone()));
                return Ok(entry);
            }
        }
    }
    let entry = Arc::new(Mutex::new(OpusWav {
        reader: OpusReader::open(path)?,
        stamp,
    }));
    let mut cache = OPUS_CACHE.lock().unwrap_or_else(|e| e.into_inner());
    cache.retain(|(p, _)| p != path);
    if cache.len() >= OPUS_CACHE_SIZE {
        cache.remove(0);
    }
    cache.push((path.to_path_buf(), entry.clone()));
    Ok(entry)
}

/// Serve an Opus file as a decoded 16-bit WAV (see the module docs).
fn serve_opus(path: &Path, range: Option<&str>, head: bool) -> Reply {
    if !path.is_file() {
        return Reply::error(404, "not found");
    }
    let entry = match opus_wav(path) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("audio: can't read {} ({e:#})", path.display());
            return Reply::error(500, "cannot decode this audio file");
        }
    };
    let mut wav = entry.lock().unwrap_or_else(|e| e.into_inner());
    let len = wav.len();
    serve(len, range, head, |start, count| wav.read(start, count))
}

/// Answer one request of the scheme: `method` (GET/HEAD), the URL `path`,
/// its `Range` header, and the archive folder (or why it's unavailable).
pub fn handle(
    archive: Result<PathBuf, String>,
    method: &str,
    path: &str,
    range: Option<&str>,
) -> Reply {
    let head = match method {
        "GET" => false,
        "HEAD" => true,
        _ => {
            let mut r = Reply::error(405, "method not allowed");
            r.headers.push(("Allow", "GET, HEAD".into()));
            return r;
        }
    };
    let Ok((id, file)) = parse_path(path) else {
        return Reply::error(400, "bad audio path");
    };
    let Ok(archive) = archive else {
        return Reply::error(503, "archive unavailable");
    };
    match resolve(&archive, &id, &file) {
        Ok(p) => serve_file(&p, range, head),
        Err(e) => {
            eprintln!("audio: refused {id}/{file} ({e:#})");
            Reply::error(404, "not found")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::archive::{create_item, ItemMeta, SegmentsFile};

    #[test]
    fn percent_decoding() {
        assert_eq!(percent_decode("2026%2F09%2Fa%20b").unwrap(), "2026/09/a b");
        assert_eq!(percent_decode("caf%C3%A9+x").unwrap(), "café+x");
        assert!(percent_decode("bad%2").is_err());
        assert!(percent_decode("bad%zz").is_err());
        assert!(percent_decode("%FF").is_err());
        assert!(percent_decode("a%00b").is_err());
    }

    #[test]
    fn path_parsing_splits_at_the_last_slash() {
        assert_eq!(
            parse_path("/2026%2F09%2F2026-09-24-sync%2Faudio-mic.wav").unwrap(),
            ("2026/09/2026-09-24-sync".into(), "audio-mic.wav".into())
        );
        // Already decoded by the webview: same result.
        assert_eq!(
            parse_path("/2026/09/x/audio.wav").unwrap(),
            ("2026/09/x".into(), "audio.wav".into())
        );
        for bad in [
            "/",
            "/audio.wav",
            "/2026%2F09%2Fx%2Ftranscript.md",
            "/2026%2F09%2Fx%2F.sussurro%2Fsegments.json",
            "/2026%2F09%2Fx%2Faudio-MIC.wav",
            "/2026%2F09%2Fx%2F..%2F..%2Fetc%2Fpasswd",
            "/x%2Faudio.wav%00",
        ] {
            assert!(parse_path(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn range_forms_and_bounds() {
        use RangeAsk::*;
        let len = 10_000;
        assert_eq!(parse_range(None, len), Whole);
        assert_eq!(
            parse_range(Some("bytes=0-99"), len),
            Part { start: 0, end: 99 }
        );
        assert_eq!(
            parse_range(Some("bytes=0-1"), len),
            Part { start: 0, end: 1 }
        );
        assert_eq!(
            parse_range(Some("bytes=9000-"), len),
            Part {
                start: 9000,
                end: 9999
            }
        );
        // End past the file is clamped.
        assert_eq!(
            parse_range(Some("bytes=9000-20000"), len),
            Part {
                start: 9000,
                end: 9999
            }
        );
        // Suffix: the last n bytes (all of them when n > len).
        assert_eq!(
            parse_range(Some("bytes=-500"), len),
            Part {
                start: 9500,
                end: 9999
            }
        );
        assert_eq!(
            parse_range(Some("bytes=-50000"), len),
            Part {
                start: 0,
                end: 9999
            }
        );
        // Only the first of several ranges.
        assert_eq!(
            parse_range(Some("bytes=10-19, 30-39"), len),
            Part { start: 10, end: 19 }
        );
        // Past the end / empty file / zero suffix: 416.
        assert_eq!(parse_range(Some("bytes=10000-"), len), Unsatisfiable);
        assert_eq!(parse_range(Some("bytes=0-"), 0), Unsatisfiable);
        assert_eq!(parse_range(Some("bytes=-0"), len), Unsatisfiable);
        // Malformed: ignored (whole file).
        for bad in [
            "items=0-1",
            "bytes=abc",
            "bytes=5-2",
            "bytes=-",
            "bytes=1-x",
            "bytes=+1-2",
        ] {
            assert_eq!(parse_range(Some(bad), len), Whole, "{bad}");
        }
    }

    #[test]
    fn open_ranges_are_capped_to_a_chunk() {
        let len = 10 * MAX_CHUNK;
        assert_eq!(
            parse_range(Some("bytes=0-"), len),
            RangeAsk::Part {
                start: 0,
                end: MAX_CHUNK - 1
            }
        );
        assert_eq!(
            parse_range(Some("bytes=5-"), len),
            RangeAsk::Part {
                start: 5,
                end: 5 + MAX_CHUNK - 1
            }
        );
        assert_eq!(
            parse_range(Some(&format!("bytes=0-{}", len - 1)), len),
            RangeAsk::Part {
                start: 0,
                end: MAX_CHUNK - 1
            }
        );
    }

    fn file_of(dir: &Path, name: &str, len: usize) -> PathBuf {
        let p = dir.join(name);
        std::fs::write(&p, (0..len).map(|i| (i % 251) as u8).collect::<Vec<_>>()).unwrap();
        p
    }

    #[test]
    fn serves_ranges_with_the_right_headers() {
        let tmp = tempfile::tempdir().unwrap();
        let p = file_of(tmp.path(), "audio.wav", 1000);

        let r = serve_file(&p, Some("bytes=100-199"), false);
        assert_eq!(r.status, 206);
        assert_eq!(r.body.len(), 100);
        assert_eq!(r.body[0], 100);
        assert_eq!(r.header("Content-Range"), Some("bytes 100-199/1000"));
        assert_eq!(r.header("Content-Length"), Some("100"));
        assert_eq!(r.header("Content-Type"), Some("audio/wav"));
        assert_eq!(r.header("Accept-Ranges"), Some("bytes"));

        // The probe WebKit sends first.
        let r = serve_file(&p, Some("bytes=0-1"), false);
        assert_eq!((r.status, r.body.len()), (206, 2));
        assert_eq!(r.header("Content-Range"), Some("bytes 0-1/1000"));

        let r = serve_file(&p, Some("bytes=-10"), false);
        assert_eq!(r.header("Content-Range"), Some("bytes 990-999/1000"));
        assert_eq!(r.body.len(), 10);

        let r = serve_file(&p, None, false);
        assert_eq!((r.status, r.body.len()), (200, 1000));
        assert_eq!(r.header("Content-Range"), None);

        let r = serve_file(&p, Some("bytes=1000-"), false);
        assert_eq!(r.status, 416);
        assert_eq!(r.header("Content-Range"), Some("bytes */1000"));

        // HEAD: headers only.
        let r = serve_file(&p, Some("bytes=0-99"), true);
        assert_eq!((r.status, r.body.len()), (206, 0));
        assert_eq!(r.header("Content-Length"), Some("100"));

        assert_eq!(
            serve_file(&tmp.path().join("missing.wav"), None, false).status,
            404
        );
    }

    #[test]
    fn a_request_without_range_gets_the_whole_file() {
        let tmp = tempfile::tempdir().unwrap();
        // Over a chunk: before #248 this got only the first MiB, as a 206.
        let len = MAX_CHUNK as usize * 2 + 7;
        let p = file_of(tmp.path(), "audio.wav", len);
        let r = serve_file(&p, None, false);
        assert_eq!(r.status, 200);
        assert!(r.body == std::fs::read(&p).unwrap());
        assert_eq!(r.header("Content-Length"), Some(len.to_string().as_str()));
        assert_eq!(r.header("Content-Range"), None);
        assert_eq!(r.header("Accept-Ranges"), Some("bytes"));
        // A malformed range is ignored the same way.
        assert_eq!(serve_file(&p, Some("bytes=x-"), false).status, 200);
        // Ranges are still served in chunks.
        let r = serve_file(&p, Some("bytes=0-"), false);
        assert_eq!(r.status, 206);
        assert_eq!(r.body.len() as u64, MAX_CHUNK);
        // The tail, from the middle of the last chunk.
        let from = len as u64 - 3;
        let r = serve_file(&p, Some(&format!("bytes={from}-")), false);
        assert_eq!(r.body.len(), 3);
        assert_eq!(
            r.header("Content-Range"),
            Some(format!("bytes {from}-{}/{len}", len - 1).as_str())
        );
        // An empty file.
        let empty = file_of(tmp.path(), "audio-e.wav", 0);
        let r = serve_file(&empty, None, false);
        assert_eq!((r.status, r.body.len()), (200, 0));
        assert_eq!(r.header("Content-Length"), Some("0"));
    }

    #[test]
    fn too_big_to_send_whole_is_refused_not_cut() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("audio.wav");
        // Sparse: nothing is written or read.
        std::fs::File::create(&p)
            .unwrap()
            .set_len(MAX_WHOLE + 1)
            .unwrap();
        let r = serve_file(&p, None, false);
        assert_eq!(r.status, 413);
        assert_eq!(r.header("Accept-Ranges"), Some("bytes"));
        // HEAD reads nothing: the length is fine to announce.
        let r = serve_file(&p, None, true);
        assert_eq!(r.status, 200);
        assert_eq!(
            r.header("Content-Length"),
            Some((MAX_WHOLE + 1).to_string().as_str())
        );
        let r = serve_file(&p, Some("bytes=100-199"), false);
        assert_eq!((r.status, r.body.len()), (206, 100));
    }

    // ---- Opus served as WAV (#248) ----

    /// A finished `name` in `dir` holding `n` samples of a sweep with a
    /// slow envelope (speech-like, no period), and its libopus decode.
    fn opus_file(dir: &Path, name: &str, n: usize) -> (PathBuf, Vec<f32>) {
        let path = dir.join(name);
        let mut w = crate::archive::opus::OpusWriter::create_capped(&path, u64::MAX).unwrap();
        let audio: Vec<f32> = (0..n)
            .map(|i| {
                let t = i as f32 / 16_000.0;
                let f = 400.0 + 150.0 * (t * 0.7).sin();
                (0.2 + 0.1 * (t * 2.0).sin()) * (t * f * std::f32::consts::TAU).sin()
            })
            .collect();
        w.write(&audio).unwrap();
        w.finish().unwrap();
        let (pcm, complete) = crate::archive::opus::decode_file(&path).unwrap();
        assert!(complete);
        assert_eq!(pcm.len(), n);
        (path, pcm)
    }

    /// The WAV of a full decode: what the scheme must serve.
    fn wav_of(pcm: &[f32]) -> Vec<u8> {
        crate::archive::audio::wav_bytes(pcm)
    }

    fn i16s(bytes: &[u8]) -> Vec<f32> {
        bytes
            .chunks_exact(2)
            .map(|b| i16::from_le_bytes([b[0], b[1]]) as f32)
            .collect()
    }

    fn snr_db(want: &[f32], got: &[f32]) -> f64 {
        let s: f64 = want.iter().map(|&x| (x as f64).powi(2)).sum();
        let n: f64 = want
            .iter()
            .zip(got)
            .map(|(&a, &b)| (a as f64 - b as f64).powi(2))
            .sum();
        10.0 * (s / n.max(1e-9)).log10()
    }

    #[test]
    fn opus_is_served_as_the_wav_of_its_decode() {
        let tmp = tempfile::tempdir().unwrap();
        // 40 s: 1.28 MB of WAV, more than a chunk.
        let (p, pcm) = opus_file(tmp.path(), "audio.opus", 40 * 16_000 + 3);
        let want = wav_of(&pcm);
        let len = want.len() as u64;

        // No range: the whole virtual WAV, exact.
        let r = serve_file(&p, None, false);
        assert_eq!(r.status, 200);
        assert_eq!(r.header("Content-Type"), Some("audio/wav"));
        assert_eq!(r.header("Content-Length"), Some(len.to_string().as_str()));
        assert!(r.body == want, "whole body differs from a full decode");

        // HEAD: the exact length, nothing decoded.
        let r = serve_file(&p, None, true);
        assert_eq!((r.status, r.body.len()), (200, 0));
        assert_eq!(r.header("Content-Length"), Some(len.to_string().as_str()));

        // The element's sequence: the probe, then open ranges one after the
        // other — bit-exact with the full decode.
        let r = serve_file(&p, Some("bytes=0-1"), false);
        assert_eq!((r.status, r.body.as_slice()), (206, &b"RI"[..]));
        let expected = format!("bytes 0-1/{len}");
        assert_eq!(r.header("Content-Range"), Some(expected.as_str()));
        let mut got = Vec::new();
        let mut at = 0u64;
        while at < len {
            let r = serve_file(&p, Some(&format!("bytes={at}-")), false);
            assert_eq!(r.status, 206);
            let end = (at + MAX_CHUNK).min(len) - 1;
            let range = format!("bytes {at}-{end}/{len}");
            assert_eq!(r.header("Content-Range"), Some(range.as_str()));
            let count = (end - at + 1).to_string();
            assert_eq!(r.header("Content-Length"), Some(count.as_str()));
            got.extend_from_slice(&r.body);
            at = end + 1;
        }
        assert!(got == want, "chunked bytes differ from a full decode");

        // Ranges across the header's end and on odd bytes, from the start.
        let r = serve_file(&p, Some("bytes=40-49"), false);
        assert_eq!(r.body, want[40..50]);
        let r = serve_file(&p, Some("bytes=45-1044"), false);
        assert_eq!(r.body, want[45..1045]);

        // At and past the end.
        let r = serve_file(&p, Some(&format!("bytes={len}-")), false);
        assert_eq!(r.status, 416);
        let expected = format!("bytes */{len}");
        assert_eq!(r.header("Content-Range"), Some(expected.as_str()));
        let r = serve_file(&p, Some(&format!("bytes={}-{}", len - 5, len + 100)), false);
        assert_eq!(r.body.len(), 5);
        let r = serve_file(&p, Some("bytes=-4"), false);
        let expected = format!("bytes {}-{}/{len}", len - 4, len - 1);
        assert_eq!(r.header("Content-Range"), Some(expected.as_str()));
        assert_eq!(r.body.len(), 4);
    }

    #[test]
    fn opus_seeks_decode_close_to_a_full_decode() {
        let tmp = tempfile::tempdir().unwrap();
        let (p, pcm) = opus_file(tmp.path(), "audio-remote.opus", 30 * 16_000);
        let want = wav_of(&pcm);
        // Random 64 KiB reads, backwards and forwards, odd offsets.
        for start in [900_001u64, 120_000, 640_445, 44, 300_000, 900_000] {
            let end = (start + 65_535).min(want.len() as u64 - 1);
            let r = serve_file(&p, Some(&format!("bytes={start}-{end}")), false);
            assert_eq!(r.status, 206);
            assert_eq!(r.body.len() as u64, end - start + 1, "at {start}");
            // Compared as PCM, on whole samples of the range.
            let a = (start + (start % 2)) as usize;
            let off = a - start as usize;
            let n = (r.body.len() - off) / 2 * 2;
            let db = snr_db(&i16s(&want[a..a + n]), &i16s(&r.body[off..off + n]));
            // A wrong position would be near 0 dB (see the reader's tests).
            assert!(db > 30.0, "at {start}: {db:.1} dB");
        }
    }

    #[test]
    fn broken_opus_is_an_error_and_a_changed_file_is_reindexed() {
        let tmp = tempfile::tempdir().unwrap();
        let junk = file_of(tmp.path(), "audio-x.opus", 500);
        assert_eq!(serve_file(&junk, Some("bytes=0-1"), false).status, 500);
        let missing = tmp.path().join("audio-y.opus");
        assert_eq!(serve_file(&missing, None, false).status, 404);

        // Served once, then replaced by a longer file at the same path.
        let (p, short) = opus_file(tmp.path(), "audio.opus", 16_000);
        let r = serve_file(&p, None, true);
        let expected = (44 + short.len() * 2).to_string();
        assert_eq!(r.header("Content-Length"), Some(expected.as_str()));
        std::fs::remove_file(&p).unwrap();
        let (p, long) = opus_file(tmp.path(), "audio.opus", 3 * 16_000 + 1);
        let r = serve_file(&p, None, false);
        assert!(r.body == wav_of(&long));
    }

    const DATE: &str = "2026-09-24T10:00:00+02:00";

    fn item(archive: &Path, title: &str) -> (String, PathBuf) {
        let meta = ItemMeta {
            title: title.into(),
            date: DATE.into(),
            ..Default::default()
        };
        let id = create_item(archive, &meta, &SegmentsFile::default()).unwrap();
        let dir = archive.join(&id);
        (id, dir)
    }

    #[test]
    fn resolution_is_confined_to_the_item_folder() {
        let tmp = tempfile::tempdir().unwrap();
        let archive = tmp.path().join("Sussurro");
        let (id, dir) = item(&archive, "Sync");
        file_of(&dir, "audio.wav", 64);
        file_of(&dir, "audio-mic.wav", 64);
        // A WAV outside the archive.
        let outside = file_of(tmp.path(), "audio.wav", 64);

        assert_eq!(
            resolve(&archive, &id, "audio.wav").unwrap(),
            dir.join("audio.wav")
        );
        assert!(resolve(&archive, &id, "audio-mic.wav").is_ok());
        // Opus files (#247) are audio names too.
        file_of(&dir, "audio-mic.opus", 64);
        assert!(resolve(&archive, &id, "audio-mic.opus").is_ok());
        // Not an audio name, even though the file exists.
        assert!(resolve(&archive, &id, "transcript.md").is_err());
        // Absent channel.
        assert!(resolve(&archive, &id, "audio-remote.wav").is_err());
        // Ids that are not items, or escape the archive.
        assert!(resolve(&archive, "2026/09", "audio.wav").is_err());
        assert!(resolve(&archive, "../audio.wav", "audio.wav").is_err());
        assert!(resolve(&archive, "2026/09/../../..", "audio.wav").is_err());
        assert!(resolve(&archive, outside.to_str().unwrap(), "audio.wav").is_err());
        assert!(resolve(&archive, "2026/09/nope", "audio.wav").is_err());
        // A directory with an audio name.
        std::fs::create_dir(dir.join("audio-x.wav")).unwrap();
        assert!(resolve(&archive, &id, "audio-x.wav").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn links_are_not_followed() {
        let tmp = tempfile::tempdir().unwrap();
        let archive = tmp.path().join("Sussurro");
        let (id, dir) = item(&archive, "Sync");
        let secret = file_of(tmp.path(), "secret.bin", 64);
        std::os::unix::fs::symlink(&secret, dir.join("audio-remote.wav")).unwrap();
        assert!(resolve(&archive, &id, "audio-remote.wav").is_err());
        // A linked item folder pointing outside the archive.
        let elsewhere = tmp.path().join("elsewhere");
        std::fs::create_dir(&elsewhere).unwrap();
        std::fs::write(elsewhere.join("transcript.md"), "---\ntitle: x\n---\n").unwrap();
        file_of(&elsewhere, "audio.wav", 64);
        std::os::unix::fs::symlink(&elsewhere, archive.join("2026/09/linked")).unwrap();
        assert!(resolve(&archive, "2026/09/linked", "audio.wav").is_err());
    }

    #[test]
    fn requests_end_to_end() {
        let tmp = tempfile::tempdir().unwrap();
        let archive = tmp.path().join("Sussurro");
        let (id, dir) = item(&archive, "Weekly sync");
        file_of(&dir, "audio-remote.wav", 500);
        let url_path = format!("/{}", id.replace('/', "%2F") + "%2Faudio-remote.wav");

        let r = handle(Ok(archive.clone()), "GET", &url_path, Some("bytes=0-9"));
        assert_eq!((r.status, r.body.len()), (206, 10));
        assert_eq!(
            handle(Ok(archive.clone()), "HEAD", &url_path, None).status,
            200
        );
        assert_eq!(
            handle(Ok(archive.clone()), "POST", &url_path, None).status,
            405
        );
        assert_eq!(
            handle(Err("no archive".into()), "GET", &url_path, None).status,
            503
        );
        // The transcript next to it is not reachable.
        let md = format!("/{}", id.replace('/', "%2F") + "%2Ftranscript.md");
        assert_eq!(handle(Ok(archive.clone()), "GET", &md, None).status, 400);
        // Traversal in the id.
        let up = "/..%2F..%2Faudio-remote.wav";
        assert_eq!(handle(Ok(archive.clone()), "GET", up, None).status, 404);
        let missing = format!("/{}", id.replace('/', "%2F") + "%2Faudio-mic.wav");
        assert_eq!(handle(Ok(archive), "GET", &missing, None).status, 404);
    }
}
