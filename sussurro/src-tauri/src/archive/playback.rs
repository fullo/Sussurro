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
//! `audio-<letters>.wav`), and the file must be a regular file (not a link)
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

use super::audio::is_audio_file_name;
use anyhow::{bail, Context, Result};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

/// The URI scheme of saved audio.
pub const SCHEME: &str = "sussurro-audio";
/// Largest body of one response. An open-ended range (`bytes=0-`, what
/// media elements send) gets this much; the element asks for the rest as it
/// plays. 1 MiB is about 33 s of 16 kHz 16-bit mono.
pub const MAX_CHUNK: u64 = 1024 * 1024;
const CONTENT_TYPE: &str = "audio/wav";

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
    let meta = std::fs::symlink_metadata(&path)
        .with_context(|| format!("no audio '{file}' in '{id}'"))?;
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

/// Serve the file at `path` for a request with `range` (the `Range`
/// header). `head`: headers only. A request without a usable range gets the
/// whole file when it fits in [`MAX_CHUNK`], else its first chunk as a 206
/// (a media element always sends a range; this only bounds the memory of a
/// stray plain GET).
pub fn serve_file(path: &Path, range: Option<&str>, head: bool) -> Reply {
    let len = match std::fs::metadata(path) {
        Ok(m) => m.len(),
        Err(_) => return Reply::error(404, "not found"),
    };
    let (status, start, end) = match parse_range(range, len) {
        RangeAsk::Unsatisfiable => {
            let mut r = Reply::error(416, "range not satisfiable");
            r.headers.push(("Content-Range", format!("bytes */{len}")));
            r.headers.push(("Accept-Ranges", "bytes".into()));
            return r;
        }
        RangeAsk::Whole if len == 0 => (200, 0, 0),
        RangeAsk::Whole if len <= MAX_CHUNK => (200, 0, len - 1),
        RangeAsk::Whole => (206, 0, MAX_CHUNK - 1),
        RangeAsk::Part { start, end } => (206, start, end),
    };
    let count = if len == 0 { 0 } else { end - start + 1 };
    let body = if head || count == 0 {
        Vec::new()
    } else {
        match read_span(path, start, count) {
            Ok(b) => b,
            Err(_) => return Reply::error(500, "read failed"),
        }
    };
    // The file may have shrunk since `metadata` (Delete audio racing a
    // request): describe what was actually read.
    let sent = if head { count } else { body.len() as u64 };
    let mut headers = vec![
        ("Content-Type", CONTENT_TYPE.to_string()),
        ("Accept-Ranges", "bytes".to_string()),
        ("Content-Length", sent.to_string()),
        // A file can still change (a session writing it, Delete audio).
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

/// Answer one request of the scheme: `method` (GET/HEAD), the URL `path`,
/// its `Range` header, and the archive folder (or why it's unavailable).
pub fn handle(archive: Result<PathBuf, String>, method: &str, path: &str, range: Option<&str>) -> Reply {
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
        assert_eq!(parse_range(Some("bytes=0-99"), len), Part { start: 0, end: 99 });
        assert_eq!(parse_range(Some("bytes=0-1"), len), Part { start: 0, end: 1 });
        assert_eq!(parse_range(Some("bytes=9000-"), len), Part { start: 9000, end: 9999 });
        // End past the file is clamped.
        assert_eq!(parse_range(Some("bytes=9000-20000"), len), Part { start: 9000, end: 9999 });
        // Suffix: the last n bytes (all of them when n > len).
        assert_eq!(parse_range(Some("bytes=-500"), len), Part { start: 9500, end: 9999 });
        assert_eq!(parse_range(Some("bytes=-50000"), len), Part { start: 0, end: 9999 });
        // Only the first of several ranges.
        assert_eq!(parse_range(Some("bytes=10-19, 30-39"), len), Part { start: 10, end: 19 });
        // Past the end / empty file / zero suffix: 416.
        assert_eq!(parse_range(Some("bytes=10000-"), len), Unsatisfiable);
        assert_eq!(parse_range(Some("bytes=0-"), 0), Unsatisfiable);
        assert_eq!(parse_range(Some("bytes=-0"), len), Unsatisfiable);
        // Malformed: ignored (whole file).
        for bad in ["items=0-1", "bytes=abc", "bytes=5-2", "bytes=-", "bytes=1-x", "bytes=+1-2"] {
            assert_eq!(parse_range(Some(bad), len), Whole, "{bad}");
        }
    }

    #[test]
    fn open_ranges_are_capped_to_a_chunk() {
        let len = 10 * MAX_CHUNK;
        assert_eq!(
            parse_range(Some("bytes=0-"), len),
            RangeAsk::Part { start: 0, end: MAX_CHUNK - 1 }
        );
        assert_eq!(
            parse_range(Some("bytes=5-"), len),
            RangeAsk::Part { start: 5, end: 5 + MAX_CHUNK - 1 }
        );
        assert_eq!(
            parse_range(Some(&format!("bytes=0-{}", len - 1)), len),
            RangeAsk::Part { start: 0, end: MAX_CHUNK - 1 }
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

        assert_eq!(serve_file(&tmp.path().join("missing.wav"), None, false).status, 404);
    }

    #[test]
    fn a_big_file_is_never_read_whole() {
        let tmp = tempfile::tempdir().unwrap();
        let len = MAX_CHUNK as usize * 2 + 7;
        let p = file_of(tmp.path(), "audio.wav", len);
        let r = serve_file(&p, None, false);
        assert_eq!(r.status, 206);
        assert_eq!(r.body.len() as u64, MAX_CHUNK);
        assert_eq!(r.header("Content-Range"), Some(format!("bytes 0-{}/{len}", MAX_CHUNK - 1).as_str()));
        let r = serve_file(&p, Some("bytes=0-"), false);
        assert_eq!(r.body.len() as u64, MAX_CHUNK);
        // The tail, from the middle of the last chunk.
        let from = len as u64 - 3;
        let r = serve_file(&p, Some(&format!("bytes={from}-")), false);
        assert_eq!(r.body.len(), 3);
        assert_eq!(r.header("Content-Range"), Some(format!("bytes {from}-{}/{len}", len - 1).as_str()));
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

        assert_eq!(resolve(&archive, &id, "audio.wav").unwrap(), dir.join("audio.wav"));
        assert!(resolve(&archive, &id, "audio-mic.wav").is_ok());
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
        assert_eq!(handle(Ok(archive.clone()), "HEAD", &url_path, None).status, 200);
        assert_eq!(handle(Ok(archive.clone()), "POST", &url_path, None).status, 405);
        assert_eq!(handle(Err("no archive".into()), "GET", &url_path, None).status, 503);
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
