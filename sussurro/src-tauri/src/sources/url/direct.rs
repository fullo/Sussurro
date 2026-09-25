//! Direct media links: an http(s) GET streamed to the run's temporary file.
//! Redirects are followed by hand (at most [`MAX_REDIRECTS`]) so that every
//! hop goes through the address rules and each connection is pinned to the
//! addresses that were checked (see the module docs of [`super`]).

use super::{
    is_web_page, pick_extension, resolve_checked, too_large, validate_url, Progress, TempDownload,
    MAX_REDIRECTS,
};
use anyhow::{anyhow, bail, Context, Result};
use reqwest::header::{ACCEPT, CONTENT_TYPE, LOCATION};
use reqwest::Url;
use std::io::{Read, Write};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

/// The link answered with a web page, not media. The run falls back to
/// yt-dlp when it is installed.
#[derive(Debug)]
pub struct WebPage;

impl std::fmt::Display for WebPage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the link opens a web page, not an audio or video file")
    }
}

impl std::error::Error for WebPage {}

/// A finished direct download.
#[derive(Debug)]
pub struct Downloaded {
    pub path: PathBuf,
    pub bytes: u64,
    pub content_type: Option<String>,
    /// Where the last redirect led.
    pub final_url: Url,
}

/// Bytes read before choosing the file's extension (sniffing).
const SNIFF_BYTES: usize = 512;

/// A client for one hop: the model downloads' timeouts, no automatic
/// redirects, the host pinned to the checked addresses, and never through
/// a system proxy (`HTTP(S)_PROXY`, OS settings): a proxy would resolve and
/// connect on its own, past the pinning (#216).
fn client_for(url: &Url, addrs: &[SocketAddr]) -> Result<reqwest::blocking::Client> {
    let mut b = crate::stt::models::download_client_builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .user_agent(concat!("Sussurro/", env!("CARGO_PKG_VERSION")));
    if let Some(host) = url.host_str() {
        // IP literals need no pinning (no DNS is involved).
        if host.parse::<std::net::IpAddr>().is_err() && !host.starts_with('[') {
            b = b.resolve_to_addrs(host, addrs);
        }
    }
    Ok(b.build()?)
}

/// Download `url` into `dest` (the extension is chosen from the content).
/// Refuses local addresses unless `allow_local`, files over `max_bytes`,
/// and web pages ([`WebPage`]). `cancel` is checked between reads.
pub fn download(
    url: &Url,
    allow_local: bool,
    max_bytes: u64,
    dest: &TempDownload,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(&Progress),
) -> Result<Downloaded> {
    let mut current = url.clone();
    for _ in 0..=MAX_REDIRECTS {
        if cancel.load(Ordering::Relaxed) {
            bail!("cancelled");
        }
        let addrs = resolve_checked(&current, allow_local)?;
        let host = current.host_str().unwrap_or_default().to_string();
        let resp = client_for(&current, &addrs)?
            .get(current.clone())
            .header(ACCEPT, "audio/*, video/*;q=0.9, */*;q=0.5")
            .send()
            .with_context(|| format!("could not download from {host}"))?;
        let status = resp.status();
        if status.is_redirection() {
            let location = resp
                .headers()
                .get(LOCATION)
                .and_then(|v| v.to_str().ok())
                .ok_or_else(|| anyhow!("{host} redirected without a location"))?;
            let next = current
                .join(location)
                .with_context(|| format!("{host} redirected to an invalid link"))?;
            // The same rules as a pasted link: http(s), no credentials.
            current = validate_url(next.as_str())?;
            continue;
        }
        if !status.is_success() {
            bail!("{host} answered {status}");
        }
        return save(resp, current, max_bytes, dest, cancel, progress);
    }
    bail!("too many redirects (more than {MAX_REDIRECTS})")
}

fn save(
    mut resp: reqwest::blocking::Response,
    url: Url,
    max_bytes: u64,
    dest: &TempDownload,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(&Progress),
) -> Result<Downloaded> {
    let content_type = resp
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.trim().to_ascii_lowercase());
    let total = resp.content_length().filter(|&n| n > 0);
    if let Some(t) = total.filter(|&t| t > max_bytes) {
        return Err(too_large(Some(t), max_bytes));
    }
    if is_web_page(content_type.as_deref(), &[]) {
        return Err(WebPage.into());
    }

    let interrupted = || "the download was interrupted";
    // The first bytes pick the extension (the decoder hint).
    let mut head = Vec::with_capacity(SNIFF_BYTES);
    let mut buf = vec![0u8; 64 * 1024];
    while head.len() < SNIFF_BYTES {
        let n = resp
            .read(&mut buf[..SNIFF_BYTES - head.len()])
            .with_context(interrupted)?;
        if n == 0 {
            break;
        }
        head.extend_from_slice(&buf[..n]);
    }
    if head.is_empty() {
        bail!("the link returned an empty file");
    }
    if is_web_page(content_type.as_deref(), &head) {
        return Err(WebPage.into());
    }
    let ext = pick_extension(content_type.as_deref(), &url, &head);
    let path = dest.path(&ext);
    let mut out = std::io::BufWriter::new(
        std::fs::File::create(&path)
            .with_context(|| format!("creating the download {}", path.display()))?,
    );
    out.write_all(&head)?;
    let mut done = head.len() as u64;
    progress(&Progress {
        downloaded: done,
        total,
        title: None,
    });
    loop {
        if cancel.load(Ordering::Relaxed) {
            bail!("cancelled");
        }
        let n = resp.read(&mut buf).with_context(interrupted)?;
        if n == 0 {
            break;
        }
        done += n as u64;
        if done > max_bytes {
            return Err(too_large(None, max_bytes));
        }
        out.write_all(&buf[..n])
            .context("writing the download (is the disk full?)")?;
        progress(&Progress {
            downloaded: done,
            total,
            title: None,
        });
    }
    out.flush()
        .context("writing the download (is the disk full?)")?;
    if total.is_some_and(|t| done < t) {
        bail!(
            "{} (got {done} of {} bytes)",
            interrupted(),
            total.unwrap_or(0)
        );
    }
    Ok(Downloaded {
        path,
        bytes: done,
        content_type,
        final_url: url,
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::sync::Arc;

    /// A tiny local HTTP server for the tests: `routes` maps a path to
    /// (status, headers, body). Runs until the returned guard is dropped.
    pub(crate) struct TestServer {
        pub base: String,
        server: Arc<tiny_http::Server>,
        thread: Option<std::thread::JoinHandle<()>>,
    }

    impl Drop for TestServer {
        fn drop(&mut self) {
            self.server.unblock();
            if let Some(t) = self.thread.take() {
                let _ = t.join();
            }
        }
    }

    pub(crate) type Route = (u16, Vec<(&'static str, String)>, Vec<u8>);

    pub(crate) fn serve(routes: Vec<(&'static str, Route)>) -> TestServer {
        let server = Arc::new(tiny_http::Server::http("127.0.0.1:0").unwrap());
        let port = server.server_addr().to_ip().unwrap().port();
        let s = server.clone();
        let thread = std::thread::spawn(move || {
            for req in s.incoming_requests() {
                let path = req.url().to_string();
                let Some((_, (status, headers, body))) = routes.iter().find(|(p, _)| *p == path)
                else {
                    let _ = req.respond(tiny_http::Response::empty(404));
                    continue;
                };
                let mut resp =
                    tiny_http::Response::from_data(body.clone()).with_status_code(*status);
                for (k, v) in headers {
                    resp.add_header(
                        tiny_http::Header::from_bytes(k.as_bytes(), v.as_bytes()).unwrap(),
                    );
                }
                let _ = req.respond(resp);
            }
        });
        TestServer {
            base: format!("http://127.0.0.1:{port}"),
            server,
            thread: Some(thread),
        }
    }

    /// One second of a 440 Hz tone as a 16-bit WAV file.
    pub(crate) fn wav_bytes(secs: f32) -> Vec<u8> {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("t.wav");
        let n = (secs * 16_000.0) as usize;
        let tone: Vec<f32> = (0..n).map(|i| (i as f32 * 0.17).sin() * 0.3).collect();
        crate::audio::decode::write_wav_i16(&p, 16_000, 1, &tone);
        std::fs::read(p).unwrap()
    }

    fn get(
        url: &str,
        allow_local: bool,
        max: u64,
        dest: &TempDownload,
    ) -> (Result<Downloaded>, Vec<Progress>) {
        let mut seen = Vec::new();
        let r = download(
            &Url::parse(url).unwrap(),
            allow_local,
            max,
            dest,
            &AtomicBool::new(false),
            &mut |p| seen.push(p.clone()),
        );
        (r, seen)
    }

    #[test]
    fn downloads_a_wav_from_a_local_server_when_allowed() {
        let wav = wav_bytes(1.0);
        let srv = serve(vec![
            // No content type and no extension: sniffed as WAV.
            ("/media?id=1", (200, vec![], wav.clone())),
            (
                "/go",
                (302, vec![("Location", "/media?id=1".into())], vec![]),
            ),
        ]);
        let dir = tempfile::tempdir().unwrap();
        let dest = TempDownload::create(dir.path(), 1).unwrap();

        // Refused by default: 127.0.0.1 is this computer.
        let (r, _) = get(
            &format!("{}/go", srv.base),
            false,
            super::super::MAX_DOWNLOAD_BYTES,
            &dest,
        );
        let e = r.unwrap_err().to_string();
        assert!(e.contains("Allow local network addresses"), "{e}");
        assert!(dest.files().is_empty());

        // Allowed: the redirect is followed, the content sniffed.
        let (r, seen) = get(
            &format!("{}/go", srv.base),
            true,
            super::super::MAX_DOWNLOAD_BYTES,
            &dest,
        );
        let d = r.unwrap();
        assert_eq!(d.path, dest.path("wav"));
        assert_eq!(d.bytes, wav.len() as u64);
        assert_eq!(std::fs::read(&d.path).unwrap(), wav);
        assert_eq!(d.final_url.path(), "/media");
        let last = seen.last().unwrap();
        assert_eq!(
            (last.downloaded, last.total),
            (wav.len() as u64, Some(wav.len() as u64))
        );

        // And the file source decodes it.
        let mut src = crate::sources::file::FileSource::open(&d.path).unwrap();
        let mut n = 0;
        while let Some(f) = crate::sources::Source::next_frame(&mut src).unwrap() {
            n += f.samples.len();
        }
        assert_eq!(n, 16_000);
    }

    #[test]
    fn web_pages_too_large_files_and_errors_are_refused() {
        let wav = wav_bytes(0.5);
        let srv = serve(vec![
            (
                "/page",
                (
                    200,
                    vec![("Content-Type", "text/html; charset=utf-8".into())],
                    b"<html></html>".to_vec(),
                ),
            ),
            // Mislabelled HTML is caught by its bytes.
            (
                "/sneaky.mp3",
                (
                    200,
                    vec![("Content-Type", "application/octet-stream".into())],
                    b"<!DOCTYPE html><p>".to_vec(),
                ),
            ),
            ("/big.wav", (200, vec![], wav.clone())),
            ("/gone.mp3", (404, vec![], vec![])),
            ("/loop", (302, vec![("Location", "/loop".into())], vec![])),
            (
                "/ftp",
                (
                    302,
                    vec![("Location", "ftp://example.com/a.mp3".into())],
                    vec![],
                ),
            ),
        ]);
        let dir = tempfile::tempdir().unwrap();
        let dest = TempDownload::create(dir.path(), 2).unwrap();
        let max = super::super::MAX_DOWNLOAD_BYTES;
        let err = |path: &str, max: u64| {
            get(&format!("{}{path}", srv.base), true, max, &dest)
                .0
                .unwrap_err()
        };

        assert!(err("/page", max).is::<WebPage>());
        assert!(err("/sneaky.mp3", max).is::<WebPage>());
        let e = err("/big.wav", 100).to_string();
        assert!(e.contains("limit for links"), "{e}");
        let e = err("/gone.mp3", max).to_string();
        assert!(e.contains("404"), "{e}");
        let e = err("/loop", max).to_string();
        assert!(e.contains("too many redirects"), "{e}");
        let e = err("/ftp", max).to_string();
        assert!(e.contains("only http and https"), "{e}");
        // Nothing half-written survives the guard.
        drop(dest);
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    #[test]
    fn cancel_stops_before_downloading() {
        let srv = serve(vec![("/a.wav", (200, vec![], wav_bytes(0.2)))]);
        let dir = tempfile::tempdir().unwrap();
        let dest = TempDownload::create(dir.path(), 3).unwrap();
        let e = download(
            &Url::parse(&format!("{}/a.wav", srv.base)).unwrap(),
            true,
            super::super::MAX_DOWNLOAD_BYTES,
            &dest,
            &AtomicBool::new(true),
            &mut |_| {},
        )
        .unwrap_err();
        assert!(e.to_string().contains("cancelled"));
    }

    /// Child half of [`direct_downloads_ignore_the_system_proxy`]: runs only
    /// in the re-launched test binary, with proxy variables set.
    #[test]
    fn no_proxy_child() {
        let Ok(url) = std::env::var("SUSSURRO_TEST_NO_PROXY_URL") else {
            return; // an ordinary test run
        };
        let dir = tempfile::tempdir().unwrap();
        let dest = TempDownload::create(dir.path(), 1).unwrap();
        let (r, _) = get(&url, true, super::super::MAX_DOWNLOAD_BYTES, &dest);
        r.unwrap();
    }

    /// #216: `HTTP_PROXY` & co. must not take a direct download past the
    /// address pinning. The test binary re-runs itself with every proxy
    /// variable pointing at a trap; the download must succeed without the
    /// trap ever being connected to.
    #[test]
    fn direct_downloads_ignore_the_system_proxy() {
        let srv = serve(vec![("/a.wav", (200, vec![], wav_bytes(0.2)))]);
        let trap = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        trap.set_nonblocking(true).unwrap();
        let proxy = format!("http://{}", trap.local_addr().unwrap());
        let mut cmd = std::process::Command::new(std::env::current_exe().unwrap());
        cmd.args([
            "--exact",
            "sources::url::direct::tests::no_proxy_child",
            "--test-threads=1",
        ])
        .env("SUSSURRO_TEST_NO_PROXY_URL", format!("{}/a.wav", srv.base))
        .env_remove("NO_PROXY")
        .env_remove("no_proxy");
        for var in [
            "HTTP_PROXY",
            "HTTPS_PROXY",
            "ALL_PROXY",
            "http_proxy",
            "https_proxy",
            "all_proxy",
        ] {
            cmd.env(var, &proxy);
        }
        let out = cmd.output().unwrap();
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(
            out.status.success() && stdout.contains("1 passed"),
            "{stdout}\n{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(
            matches!(trap.accept(), Err(e) if e.kind() == std::io::ErrorKind::WouldBlock),
            "the download went through the proxy"
        );
    }

    /// A real direct media link (plan §11, 0.8: "one direct mp3 link").
    /// Needs the network:
    /// `SUSSURRO_LIVE_MEDIA=<mp3 url> cargo test live_direct_link -- --ignored`.
    #[test]
    #[ignore]
    fn live_direct_link_download() {
        let url = std::env::var("SUSSURRO_LIVE_MEDIA").unwrap_or_else(|_| {
            "https://upload.wikimedia.org/wikipedia/commons/c/c8/Example.ogg".into()
        });
        let dir = tempfile::tempdir().unwrap();
        let dest = TempDownload::create(dir.path(), 9).unwrap();
        let (r, _) = get(&url, false, super::super::MAX_DOWNLOAD_BYTES, &dest);
        let d = r.unwrap();
        assert!(
            crate::sources::file::FileSource::open(&d.path).is_ok(),
            "{:?}",
            d.path
        );
    }
}
