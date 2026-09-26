//! Local API tests: the route table (pure), then the real
//! `tiny_http` server on a loopback port with a fake host — token-less
//! routes unchanged, token/Origin/CORS on the meeting routes, and a
//! WebSocket meeting from a `tungstenite` client through the long-form
//! engine (fake STT) into an archive item in a temp dir.

use super::*;
use crate::archive::{self, Channel, ItemType};
use crate::engine::segmenter::EnergyDetector;
use crate::settings::Settings;
use crate::state::AppPaths;
use crate::stt::TimedTranscript;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::Mutex;
use tungstenite::client::IntoClientRequest;
use tungstenite::Message;

const TOKEN: &str = "5c1e0f2a5c1e0f2a5c1e0f2a5c1e0f2a5c1e0f2a5c1e0f2a5c1e0f2a5c1e0f2a";
const EXT: &str = "chrome-extension://abcdefghijklmnopabcdefghijklmnop";

#[test]
fn routes_map_method_and_path() {
    assert_eq!(route("POST", "/clean"), Route::Clean);
    assert_eq!(route("POST", "/transcribe"), Route::Transcribe);
    assert_eq!(route("GET", "/history"), Route::History);
    assert_eq!(route("GET", "/clean"), Route::NotFound); // wrong method
    assert_eq!(route("POST", "/nope"), Route::NotFound);
    assert_eq!(route("GET", "/app/version"), Route::AppVersion);
    assert_eq!(route("GET", "/app/languages"), Route::AppLanguages);
    assert_eq!(route("OPTIONS", "/app/languages"), Route::Preflight);
    assert_eq!(route("POST", "/app/languages"), Route::NotFound);
    assert_eq!(route("GET", "/live"), Route::Live);
    assert_eq!(route("POST", "/live"), Route::NotFound);
    assert_eq!(
        route("POST", "/items/2026/09/2026-09-24-sync/open"),
        Route::OpenItem("2026/09/2026-09-24-sync".into())
    );
    assert_eq!(
        route("POST", "/items/2026%2F09%2F2026-09-24-sync/open"),
        Route::OpenItem("2026/09/2026-09-24-sync".into())
    );
    assert_eq!(
        route("GET", "/items/2026/09/2026-09-24-sync/export"),
        Route::ExportItem("2026/09/2026-09-24-sync".into())
    );
    assert_eq!(route("GET", "/items/2026/09/x/open"), Route::NotFound);
    assert_eq!(route("POST", "/items/2026/09/x/export"), Route::NotFound);
    assert_eq!(route("POST", "/items//open"), Route::NotFound);
    assert_eq!(route("POST", "/items/open"), Route::NotFound);
    assert_eq!(route("POST", "/items/%zz/open"), Route::NotFound);
    assert_eq!(route("OPTIONS", "/app/version"), Route::Preflight);
    assert_eq!(route("OPTIONS", "/items/a/export"), Route::Preflight);
    assert_eq!(route("OPTIONS", "/clean"), Route::NotFound);
    // Everything under /archive goes through the archive middleware (#249).
    for (m, p) in [
        ("GET", "/archive"),
        ("GET", "/archive/items"),
        ("GET", "/archive/items/2026/09/x/export"),
        ("POST", "/archive/items"),
        ("DELETE", "/archive/items/x"),
        ("OPTIONS", "/archive/items"),
    ] {
        assert_eq!(route(m, p), Route::Archive, "{m} {p}");
        assert!(
            route(m, p).is_archive() && !route(m, p).is_meeting() && !route(m, p).is_scripting()
        );
    }
    assert_eq!(route("GET", "/archives"), Route::NotFound);
    assert_eq!(route("GET", "/archiveitems"), Route::NotFound);
}

#[test]
fn only_the_extension_routes_are_meeting_routes() {
    for r in [
        Route::AppVersion,
        Route::AppLanguages,
        Route::Live,
        Route::OpenItem("x".into()),
        Route::ExportItem("x".into()),
        Route::Preflight,
    ] {
        assert!(r.is_meeting());
    }
    // The token-less routes stay outside the token checks, behind their
    // own switch (#215).
    for r in [Route::Clean, Route::Transcribe, Route::History] {
        assert!(!r.is_meeting());
        assert!(r.is_scripting());
    }
    assert!(!Route::NotFound.is_meeting() && !Route::NotFound.is_scripting());
    assert!(!Route::AppVersion.is_scripting());
}

#[test]
fn parse_url_splits_path_and_params() {
    let (path, params) = parse_url("/history?n=5&q=ciao%20mondo");
    assert_eq!(path, "/history");
    assert_eq!(params.get("n").map(String::as_str), Some("5"));
    assert_eq!(params.get("q").map(String::as_str), Some("ciao%20mondo"));
    let (path, params) = parse_url("/clean");
    assert_eq!(path, "/clean");
    assert!(params.is_empty());
    let (_, params) = parse_url("/x?flag&k=v");
    assert_eq!(params.get("flag").map(String::as_str), Some(""));
    assert_eq!(params.get("k").map(String::as_str), Some("v"));
    assert_eq!(percent_decode("a%2Fb%20c").as_deref(), Some("a/b c"));
    assert_eq!(percent_decode("%"), None);
    assert_eq!(percent_decode("%ff"), None, "not UTF-8");
}

#[test]
fn websocket_upgrade_headers() {
    // RFC 6455's example key.
    assert_eq!(
        websocket_accept(
            Some("websocket"),
            Some("13"),
            Some("dGhlIHNhbXBsZSBub25jZQ==")
        )
        .unwrap(),
        "s3pPLMBiTxaQ9kYGzzhZRbK+xOo="
    );
    assert!(websocket_accept(Some("h2c"), Some("13"), Some("k")).is_err());
    assert!(websocket_accept(None, Some("13"), Some("k")).is_err());
    assert!(websocket_accept(Some("WebSocket"), Some("8"), Some("k")).is_err());
    assert!(websocket_accept(Some("websocket"), Some("13"), None).is_err());
}

// ---- the real server on a fake host ---------------------------------------

struct Inner {
    config: Mutex<ApiConfig>,
    settings: Mutex<Settings>,
    paths: AppPaths,
    archive: PathBuf,
    next_id: AtomicU64,
    meeting_running: AtomicBool,
    opened: Mutex<Vec<String>>,
    /// `/clean` and `/transcribe` calls that reached the host.
    calls: AtomicUsize,
    /// `/transcribe?ext=slow` runs until this is set (#215 concurrency).
    release_slow: AtomicBool,
    slow_running: AtomicUsize,
    /// Archive token ids that passed the middleware (#249).
    tokens_used: Mutex<Vec<String>>,
    /// `archive_dir` waits while this is set (#250 worker slots).
    block_archive: AtomicBool,
    archive_waiting: AtomicUsize,
    /// Notes `POST /archive/items` created (#251).
    notes_created: Mutex<Vec<String>>,
    /// The language each meeting segment was transcribed and cleaned
    /// with (#288): `stt:<lang>` / `cleanup:<lang>`.
    languages_seen: Mutex<Vec<String>>,
}

#[derive(Clone)]
struct TestHost(Arc<Inner>);

/// "Transcribes" by loudness: the synthetic `remote` voice is louder.
fn fake_transcribe(samples: &[f32], _language: &str) -> anyhow::Result<TimedTranscript> {
    let peak = samples.iter().fold(0.0f32, |m, s| m.max(s.abs()));
    let text = if peak > 0.2 {
        "hello from remote"
    } else {
        "hello from mic"
    };
    Ok(TimedTranscript {
        text: text.into(),
        language: Some("en".into()),
        ..Default::default()
    })
}

/// The real cleanup gate on the fake's settings; "cleans" by uppercasing.
impl archive_write::NoteHost for TestHost {
    fn note_cleanup_gate(&self) -> Result<(), archive_write::CleanupRefused> {
        let s = self.0.settings.lock().unwrap();
        archive_write::cleanup_gate(s.cleanup_active(), s.cleanup_llm().external)
    }
    fn clean_note(
        &self,
        paragraphs: &[String],
    ) -> Result<Vec<String>, archive_write::CleanupRefused> {
        self.note_cleanup_gate()?;
        self.0.calls.fetch_add(1, Ordering::SeqCst);
        Ok(paragraphs.iter().map(|p| p.to_uppercase()).collect())
    }
    fn note_created(&self, id: &str) {
        self.0.notes_created.lock().unwrap().push(id.to_string());
    }
}

impl Host for TestHost {
    fn config(&self) -> ApiConfig {
        self.0.config.lock().unwrap().clone()
    }
    fn archive_token_used(&self, id: &str) {
        self.0.tokens_used.lock().unwrap().push(id.to_string());
    }
    fn clean(&self, text: &str) -> serde_json::Value {
        self.0.calls.fetch_add(1, Ordering::SeqCst);
        serde_json::json!({"cleaned": text.trim()})
    }
    fn transcribe(&self, bytes: Vec<u8>, ext: &str) -> (u16, serde_json::Value) {
        self.0.calls.fetch_add(1, Ordering::SeqCst);
        if ext == "slow" {
            self.0.slow_running.fetch_add(1, Ordering::SeqCst);
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
            while !self.0.release_slow.load(Ordering::SeqCst)
                && std::time::Instant::now() < deadline
            {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            self.0.slow_running.fetch_sub(1, Ordering::SeqCst);
        }
        (200, serde_json::json!({"bytes": bytes.len(), "ext": ext}))
    }
    fn history(&self, query: &str, n: usize) -> serde_json::Value {
        serde_json::json!([{"q": query, "n": n}])
    }
    fn archive_dir(&self) -> anyhow::Result<PathBuf> {
        if self.0.block_archive.load(Ordering::SeqCst) {
            self.0.archive_waiting.fetch_add(1, Ordering::SeqCst);
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
            while self.0.block_archive.load(Ordering::SeqCst)
                && std::time::Instant::now() < deadline
            {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            self.0.archive_waiting.fetch_sub(1, Ordering::SeqCst);
        }
        Ok(self.0.archive.clone())
    }
    fn archive_index(&self) -> anyhow::Result<PathBuf> {
        Ok(self.0.paths.archive_index.clone())
    }
    fn open_item(&self, id: &str) -> anyhow::Result<()> {
        archive::read_item(&self.0.archive, id)?;
        self.0.opened.lock().unwrap().push(id.to_string());
        Ok(())
    }
    fn start_meeting(&self, m: live::MeetingStart) -> anyhow::Result<u64> {
        if self.0.meeting_running.swap(true, Ordering::SeqCst) {
            anyhow::bail!("a meeting is already being recorded");
        }
        let id = self.0.next_id.fetch_add(1, Ordering::SeqCst);
        let inner = self.0.clone();
        std::thread::spawn(move || {
            let cancel = Arc::new(AtomicBool::new(false));
            let req =
                crate::engine::session::meeting_request(id, cancel, &m.start, m.source, m.names);
            // No speaker model in tests: voices are off, names and "You" work.
            let no_model = |_: PathBuf| -> crate::speakers::tracker::EmbedderLoader {
                Box::new(|| Err(anyhow::anyhow!("no speaker model in tests")))
            };
            let (stt_seen, clean_seen) = (inner.clone(), inner.clone());
            let _ = crate::engine::session::run_request_with_speakers(
                &inner.settings,
                &inner.paths,
                req,
                move |samples: &[f32], language: &str| {
                    stt_seen
                        .languages_seen
                        .lock()
                        .unwrap()
                        .push(format!("stt:{language}"));
                    fake_transcribe(samples, language)
                },
                move |s: &Settings, _: Option<&str>, raw: &str| {
                    clean_seen
                        .languages_seen
                        .lock()
                        .unwrap()
                        .push(format!("cleanup:{}", s.language));
                    format!("{raw}.")
                },
                |_: &std::path::Path| {
                    Box::new(EnergyDetector::default())
                        as Box<dyn crate::engine::segmenter::SpeechDetector>
                },
                no_model,
                m.sink,
            );
            inner.meeting_running.store(false, Ordering::SeqCst);
        });
        Ok(id)
    }
}

struct Running {
    host: TestHost,
    port: u16,
    _dir: tempfile::TempDir,
}

fn start_server() -> Running {
    start_server_with(None)
}

/// The real server; `limits` replaces tiny_http's default [`tiny_http::Limits`].
fn start_server_with(limits: Option<tiny_http::Limits>) -> Running {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("data");
    std::fs::create_dir_all(&data).unwrap();
    let archive = dir.path().join("archive");
    let settings = Settings {
        archive_dir: archive.to_string_lossy().into_owned(),
        language: "en".into(),
        ..Default::default()
    };
    let paths = AppPaths {
        settings_file: data.join("settings.json"),
        models_dir: data.join("models"),
        history_file: data.join("history.jsonl"),
        stats_file: data.join("stats.json"),
        archive_index: data.join(archive::INDEX_FILE),
        documents_dir: None,
        home_dir: Some(dir.path().to_path_buf()),
    };
    let host = TestHost(Arc::new(Inner {
        config: Mutex::new(ApiConfig {
            extension_token: TOKEN.into(),
            scripting: true,
            dictation_language: "en".into(),
            ..Default::default()
        }),
        settings: Mutex::new(settings),
        paths,
        archive,
        next_id: AtomicU64::new(1),
        meeting_running: AtomicBool::new(false),
        opened: Mutex::new(Vec::new()),
        calls: AtomicUsize::new(0),
        release_slow: AtomicBool::new(false),
        slow_running: AtomicUsize::new(0),
        tokens_used: Mutex::new(Vec::new()),
        block_archive: AtomicBool::new(false),
        archive_waiting: AtomicUsize::new(0),
        notes_created: Mutex::new(Vec::new()),
        languages_seen: Mutex::new(Vec::new()),
    }));
    let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
    if let Some(limits) = limits {
        server.set_limits(limits);
    }
    let port = server.server_addr().to_ip().unwrap().port();
    let h: Arc<dyn Host> = Arc::new(host.clone());
    std::thread::spawn(move || serve(server, h));
    Running {
        host,
        port,
        _dir: dir,
    }
}

struct Reply {
    status: u16,
    headers: Vec<(String, String)>,
    body: String,
}

impl Reply {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
    fn json(&self) -> serde_json::Value {
        serde_json::from_str(&self.body).unwrap_or(serde_json::Value::Null)
    }
}

/// A raw HTTP/1.1 request (exact control over the headers). `Host` is
/// `127.0.0.1:<port>` unless `headers` name one.
fn http(port: u16, method: &str, path: &str, headers: &[(&str, &str)], body: &str) -> Reply {
    let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
    let mut req = format!("{method} {path} HTTP/1.1\r\nConnection: close\r\n");
    if !headers.iter().any(|(k, _)| k.eq_ignore_ascii_case("Host")) {
        req.push_str(&format!("Host: 127.0.0.1:{port}\r\n"));
    }
    for (k, v) in headers {
        req.push_str(&format!("{k}: {v}\r\n"));
    }
    req.push_str(&format!("Content-Length: {}\r\n\r\n{body}", body.len()));
    s.write_all(req.as_bytes()).unwrap();
    read_reply(&mut s)
}

/// One response: the head, then `Content-Length` bytes of body (never
/// waits for the server to close — a refused upload may keep it open).
fn read_reply(s: &mut TcpStream) -> Reply {
    s.set_read_timeout(Some(std::time::Duration::from_secs(20)))
        .unwrap();
    let mut raw = Vec::new();
    let mut buf = [0u8; 4096];
    let head_end = loop {
        if let Some(i) = raw.windows(4).position(|w| w == b"\r\n\r\n") {
            break i;
        }
        let n = s.read(&mut buf).unwrap_or(0);
        if n == 0 {
            break raw.len();
        }
        raw.extend_from_slice(&buf[..n]);
    };
    let head = String::from_utf8_lossy(&raw[..head_end]).into_owned();
    let length: usize = head
        .lines()
        .filter_map(|l| l.split_once(':'))
        .find(|(k, _)| k.trim().eq_ignore_ascii_case("Content-Length"))
        .and_then(|(_, v)| v.trim().parse().ok())
        .unwrap_or(0);
    let mut body = raw.get(head_end + 4..).unwrap_or_default().to_vec();
    while body.len() < length {
        let n = s.read(&mut buf).unwrap_or(0);
        if n == 0 {
            break;
        }
        body.extend_from_slice(&buf[..n]);
    }
    let body = String::from_utf8_lossy(&body).into_owned();
    let mut lines = head.lines();
    let status = lines
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|c| c.parse().ok())
        .unwrap_or(0);
    let headers = lines
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
        .collect();
    Reply {
        status,
        headers,
        body,
    }
}

fn bearer() -> String {
    format!("Bearer {TOKEN}")
}

#[test]
fn token_less_routes_are_unchanged() {
    let r = start_server();
    let clean = http(r.port, "POST", "/clean", &[], "  ciao  ");
    assert_eq!(
        (clean.status, clean.json()["cleaned"].as_str()),
        (200, Some("ciao"))
    );
    // No token needed, and no CORS for anyone — even an extension.
    let hist = http(r.port, "GET", "/history?n=3&q=x", &[("Origin", EXT)], "");
    assert_eq!(hist.status, 200);
    assert_eq!(hist.json()[0]["n"], 3);
    assert!(hist.header("Access-Control-Allow-Origin").is_none());
    let tr = http(r.port, "POST", "/transcribe?ext=wav", &[], "RIFF");
    assert_eq!((tr.status, tr.json()["ext"].as_str()), (200, Some("wav")));
    assert_eq!(http(r.port, "POST", "/clean", &[], " ").status, 400);
    // The token-less routes never answer a preflight (no CORS).
    assert_eq!(
        http(r.port, "OPTIONS", "/clean", &[("Origin", EXT)], "").status,
        404
    );
}

#[test]
fn meeting_routes_check_token_origin_and_send_cors() {
    let r = start_server();
    let auth = bearer();
    let ok = http(
        r.port,
        "GET",
        "/app/version",
        &[("Authorization", &auth), ("Origin", EXT)],
        "",
    );
    assert_eq!(ok.status, 200);
    assert_eq!(ok.json()["protocol"], protocol::PROTOCOL_VERSION);
    assert_eq!(ok.json()["protocol_min"], protocol::MIN_PROTOCOL);
    assert_eq!(ok.json()["app"], env!("CARGO_PKG_VERSION"));
    assert_eq!(ok.header("Access-Control-Allow-Origin"), Some(EXT));
    // The subtitles setting, for the side panel's "Create .srt" (#129).
    assert_eq!(ok.json()["subtitles"], "on_request");
    r.host.0.config.lock().unwrap().subtitles = crate::settings::SubtitlesMode::Always;
    let always = http(
        r.port,
        "GET",
        "/app/version",
        &[("Authorization", &auth), ("Origin", EXT)],
        "",
    );
    assert_eq!(always.json()["subtitles"], "always");
    // A local script: token, no origin.
    let script = http(
        r.port,
        "GET",
        "/app/version",
        &[("Authorization", &auth)],
        "",
    );
    assert_eq!(script.status, 200);
    assert!(script.header("Access-Control-Allow-Origin").is_none());
    // No token, a wrong one, a web page.
    assert_eq!(
        http(r.port, "GET", "/app/version", &[("Origin", EXT)], "").status,
        401
    );
    assert_eq!(
        http(
            r.port,
            "GET",
            "/app/version",
            &[("Authorization", "Bearer 00")],
            ""
        )
        .status,
        401
    );
    let web = http(
        r.port,
        "GET",
        "/app/version",
        &[("Authorization", &auth), ("Origin", "https://evil.example")],
        "",
    );
    assert_eq!(web.status, 403);
    assert!(web.header("Access-Control-Allow-Origin").is_none());
    // Preflight: extension origins only.
    let pre = http(
        r.port,
        "OPTIONS",
        "/items/a/b/export",
        &[("Origin", EXT)],
        "",
    );
    assert_eq!(pre.status, 204);
    assert!(pre
        .header("Access-Control-Allow-Headers")
        .unwrap()
        .contains("Authorization"));
    assert_eq!(
        http(
            r.port,
            "OPTIONS",
            "/app/version",
            &[("Origin", "https://x.example")],
            ""
        )
        .status,
        403
    );
    // Items that don't exist, bad formats.
    assert_eq!(
        http(
            r.port,
            "POST",
            "/items/2026/09/nope/open",
            &[("Authorization", &auth)],
            ""
        )
        .status,
        404
    );
    assert_eq!(
        http(
            r.port,
            "POST",
            "/items/../../etc/open",
            &[("Authorization", &auth)],
            ""
        )
        .status,
        404
    );
    assert_eq!(
        http(
            r.port,
            "GET",
            "/items/2026/09/nope/export?format=docx",
            &[("Authorization", &auth)],
            ""
        )
        .status,
        400
    );
    // WebSocket: an extension origin and the query token are required.
    assert_eq!(ws_connect(r.port, TOKEN, None).err(), Some(403));
    assert_eq!(
        ws_connect(r.port, TOKEN, Some("https://meet.google.com")).err(),
        Some(403)
    );
    assert_eq!(ws_connect(r.port, "wrong", Some(EXT)).err(), Some(401));
    // Unpaired (no token configured): nothing is accepted.
    r.host.0.config.lock().unwrap().extension_token.clear();
    assert_eq!(ws_connect(r.port, "", Some(EXT)).err(), Some(401));
    assert_eq!(
        http(
            r.port,
            "GET",
            "/app/version",
            &[("Authorization", "Bearer ")],
            ""
        )
        .status,
        401
    );
}

/// #127: Settings → Browser extension → Regenerate token. The API reads the
/// token on every request, so the old one is refused at once.
#[test]
fn a_regenerated_token_replaces_the_old_one_at_once() {
    let r = start_server();
    let old = bearer();
    assert_eq!(
        http(
            r.port,
            "GET",
            "/app/version",
            &[("Authorization", &old), ("Origin", EXT)],
            ""
        )
        .status,
        200
    );
    let mut settings = Settings {
        extension_token: TOKEN.into(),
        ..Default::default()
    };
    let fresh = settings.regenerate_extension_token().unwrap();
    assert_ne!(fresh, TOKEN);
    r.host.0.config.lock().unwrap().extension_token = fresh.clone();
    assert_eq!(
        http(
            r.port,
            "GET",
            "/app/version",
            &[("Authorization", &old), ("Origin", EXT)],
            ""
        )
        .status,
        401
    );
    assert_eq!(ws_connect(r.port, TOKEN, Some(EXT)).err(), Some(401));
    let new = format!("Bearer {fresh}");
    let ok = http(
        r.port,
        "GET",
        "/app/version",
        &[("Authorization", &new), ("Origin", EXT)],
        "",
    );
    assert_eq!(ok.status, 200);
    assert_eq!(ok.json()["protocol"], protocol::PROTOCOL_VERSION);
}

/// #127: the pairing UI learns whether (and where) the API listens.
#[test]
fn bind_records_the_listen_state() {
    // Taken port: recorded as failed. (The one test touching the global.)
    let taken = tiny_http::Server::http("127.0.0.1:0").unwrap();
    let port = taken.server_addr().to_ip().unwrap().port();
    assert!(bind(port).is_none());
    assert_eq!(listen_state(), ListenState::Failed { port });
    drop(taken);
    let server = bind(0).expect("an ephemeral port binds");
    let actual = server.server_addr().to_ip().unwrap().port();
    assert_eq!(listen_state(), ListenState::Listening { port: actual });
    assert_eq!(
        serde_json::to_value(ListenState::Listening { port: 4525 }).unwrap(),
        serde_json::json!({"state": "listening", "port": 4525})
    );
    assert_eq!(
        serde_json::to_value(ListenState::Off).unwrap(),
        serde_json::json!({"state": "off"})
    );
    assert_eq!(
        serde_json::to_value(ListenState::Failed { port: 1 }).unwrap(),
        serde_json::json!({"state": "failed", "port": 1})
    );
}

type Ws = tungstenite::WebSocket<tungstenite::stream::MaybeTlsStream<TcpStream>>;

/// Connect to `/live`; `Err(status)` when the upgrade is refused.
fn ws_connect(port: u16, token: &str, origin: Option<&str>) -> Result<Ws, u16> {
    let mut req = format!("ws://127.0.0.1:{port}/live?token={token}")
        .into_client_request()
        .unwrap();
    if let Some(o) = origin {
        req.headers_mut().insert("Origin", o.parse().unwrap());
    }
    match tungstenite::connect(req) {
        Ok((ws, _)) => Ok(ws),
        Err(tungstenite::Error::Http(resp)) => Err(resp.status().as_u16()),
        Err(e) => panic!("unexpected WebSocket error: {e}"),
    }
}

fn read_json(ws: &mut Ws) -> Option<serde_json::Value> {
    loop {
        match ws.read() {
            Ok(Message::Text(t)) => return serde_json::from_str(t.as_str()).ok(),
            Ok(Message::Close(_)) | Err(_) => return None,
            Ok(_) => continue,
        }
    }
}

/// 20 ms of 48 kHz audio: a tone of `amp` or silence.
fn frame_48k(t0: usize, amp: f32) -> Vec<i16> {
    (0..960)
        .map(|i| {
            let x = amp * (((t0 + i) as f32) * 0.05).sin();
            (x * 32_767.0) as i16
        })
        .collect()
}

#[test]
fn a_websocket_meeting_becomes_an_archive_item() {
    let r = start_server();
    let mut ws = ws_connect(r.port, TOKEN, Some(EXT)).expect("upgrade");
    let ready = read_json(&mut ws).unwrap();
    assert_eq!(
        (ready["type"].as_str(), ready["state"].as_str()),
        (Some("status"), Some("ready"))
    );
    assert_eq!(ready["protocol"], protocol::PROTOCOL_VERSION);

    ws.send(Message::text(
        r#"{"type":"start","title":"Weekly sync","url":"https://meet.google.com/abc-defg-hij","platform":"meet","rate":48000,"channels":2}"#,
    ))
    .unwrap();
    ws.send(Message::text(
        r#"{"type":"participants","names":["Anna Rossi","Bo"]}"#,
    ))
    .unwrap();
    // 9 s: the user (mic) speaks 0.5–2.5 s softly, the remote side
    // 3.5–6 s loudly; 20 ms frames, both channels interleaved.
    let frames = 450;
    for n in 0..frames {
        let t = n as f32 * 0.02;
        let mic_amp = if (0.5..2.5).contains(&t) { 0.1 } else { 0.0 };
        let remote_amp = if (3.5..6.0).contains(&t) { 0.4 } else { 0.0 };
        let seq = n as u32;
        ws.send(Message::binary(protocol::encode_audio_frame(
            0,
            seq,
            &frame_48k(n * 960, mic_amp),
        )))
        .unwrap();
        // One remote frame lost at 7 s: filled with silence, reported.
        if n != 350 {
            ws.send(Message::binary(protocol::encode_audio_frame(
                1,
                seq,
                &frame_48k(n * 960, remote_amp),
            )))
            .unwrap();
        }
        // A protocol 1 client: a bare name when the indicator changes.
        if n == 200 {
            ws.send(Message::text(
                r#"{"type":"speaker_active","name":"Anna Rossi","t":4000}"#,
            ))
            .unwrap();
        }
        if n == 400 {
            ws.send(Message::text(
                r#"{"type":"observer_health","state":"names_unavailable","hooks":{"speaking":"broken","x y":"ok"}}"#,
            ))
            .unwrap();
        }
    }
    ws.send(Message::text(r#"{"type":"stop"}"#)).unwrap();

    let mut segments = Vec::new();
    let mut states = Vec::new();
    let mut warnings = Vec::new();
    let mut done_item = None;
    let mut speakers = Vec::new();
    while let Some(m) = read_json(&mut ws) {
        match m["type"].as_str() {
            Some("segment") => segments.push(m["segment"].clone()),
            Some("speaker") => speakers.push((
                m["id"].as_str().unwrap_or_default().to_string(),
                m["label"].as_str().unwrap_or_default().to_string(),
                m["color"].as_str().unwrap_or_default().to_string(),
            )),
            Some("status") => {
                let state = m["state"].as_str().unwrap_or_default().to_string();
                if state == "warning" {
                    warnings.push(m["message"].as_str().unwrap_or_default().to_string());
                }
                if state == "done" {
                    done_item = m["item_id"].as_str().map(str::to_string);
                }
                assert_ne!(state, "error", "{m}");
                states.push(state);
            }
            _ => {}
        }
    }
    assert!(states.contains(&"started".to_string()), "{states:?}");
    assert_eq!(states.last().map(String::as_str), Some("done"));
    assert!(
        warnings
            .iter()
            .any(|w| w.contains("1 remote audio frame(s) lost")),
        "{warnings:?}"
    );
    let item_id = done_item.expect("done with an item id");

    // Both channels, each with its own text, in time order.
    let channels: Vec<&str> = segments
        .iter()
        .filter_map(|s| s["channel"].as_str())
        .collect();
    assert!(
        channels.contains(&"mic") && channels.contains(&"remote"),
        "{segments:?}"
    );
    let item = archive::read_item(&r.host.0.archive, &item_id).unwrap();
    assert_eq!(item.meta.item_type, ItemType::Meeting);
    assert_eq!(item.meta.source, "browser:meet.google.com");
    assert_eq!(item.meta.title, "Weekly sync");
    assert!(!item.recording);
    let segs = &item.segments.segments;
    let mic = segs.iter().find(|s| s.channel == Channel::Mic).unwrap();
    let remote = segs.iter().find(|s| s.channel == Channel::Remote).unwrap();
    assert_eq!(mic.text, "hello from mic.");
    assert_eq!(remote.text, "hello from remote.");
    // On the shared clock: the mic spoke first, the remote side at ~3.5 s.
    assert!(mic.start_ms < 1_000, "{}", mic.start_ms);
    assert!(
        (3_000..4_000).contains(&remote.start_ms),
        "{}",
        remote.start_ms
    );
    assert!(segs.windows(2).all(|w| w[0].start_ms <= w[1].start_ms));
    // Speakers (#131): the mic is You, the remote line takes the name the
    // page showed during it (no speaker model here, so no voices); both
    // announced to the client once, before their first segment.
    assert_eq!(mic.speaker_id.as_deref(), Some("you"));
    assert_eq!(remote.speaker_id.as_deref(), Some("meet:Anna Rossi"));
    let listed: Vec<(&str, &str)> = item
        .segments
        .speakers
        .iter()
        .map(|s| (s.id.as_str(), s.label.as_str()))
        .collect();
    assert_eq!(listed, [("you", "You"), ("meet:Anna Rossi", "Anna Rossi")]);
    // Announced with the item's own labels and colours.
    let item_speakers: Vec<(String, String, String)> = item
        .segments
        .speakers
        .iter()
        .map(|s| (s.id.clone(), s.label.clone(), s.color.clone()))
        .collect();
    assert_eq!(speakers, item_speakers);
    // The page's participants are the item's (emails would come from People).
    let names: Vec<&str> = item
        .meta
        .participants
        .iter()
        .map(|p| p.name.as_str())
        .collect();
    assert_eq!(names, ["Anna Rossi", "Bo"]);

    // The page's events are stored, on the session clock.
    let events = archive::meeting::read_events(&r.host.0.archive, &item_id).unwrap();
    assert_eq!(events.len(), 3, "{events:?}");
    assert!(
        matches!(&events[0], archive::meeting::MeetingEvent::Participants { names, .. } if names.len() == 2)
    );
    let archive::meeting::MeetingEvent::SpeakerActive {
        at_ms,
        t_ms,
        name,
        id,
        ..
    } = &events[1]
    else {
        panic!("{events:?}")
    };
    assert_eq!(
        (name.as_deref(), *t_ms, id.as_deref()),
        (Some("Anna Rossi"), 4_000, None)
    );
    assert!((3_900..=4_100).contains(at_ms), "{at_ms}");
    let archive::meeting::MeetingEvent::ObserverHealth { state, hooks, .. } = &events[2] else {
        panic!("{events:?}")
    };
    assert_eq!(state, "names_unavailable");
    assert_eq!(hooks.len(), 1, "a bad hook name is dropped: {hooks:?}");

    // Export and open over HTTP, with the token.
    let auth = bearer();
    let txt = http(
        r.port,
        "GET",
        &format!("/items/{item_id}/export?format=txt"),
        &[("Authorization", &auth), ("Origin", EXT)],
        "",
    );
    assert_eq!(txt.status, 200);
    assert!(txt
        .header("Content-Type")
        .unwrap()
        .starts_with("text/plain"));
    assert_eq!(txt.header("Access-Control-Allow-Origin"), Some(EXT));
    assert!(txt.body.contains("] You: hello from mic."), "{}", txt.body);
    assert!(
        txt.body.contains("] Anna Rossi: hello from remote."),
        "{}",
        txt.body
    );
    let md = http(
        r.port,
        "GET",
        &format!("/items/{item_id}/export"),
        &[("Authorization", &auth)],
        "",
    );
    assert_eq!(md.status, 200);
    assert!(md.body.starts_with("---\n"), "{}", md.body);
    assert!(md.body.contains("type: meeting"));
    let srt = http(
        r.port,
        "GET",
        &format!("/items/{item_id}/export?format=srt"),
        &[("Authorization", &auth)],
        "",
    );
    assert_eq!(srt.status, 200);
    assert!(srt
        .header("Content-Type")
        .unwrap()
        .starts_with("application/x-subrip"));
    assert!(srt.header("Content-Disposition").unwrap().contains(".srt"));
    assert!(
        srt.body.contains(" --> ") && srt.body.contains("hello from remote."),
        "{}",
        srt.body
    );
    let vtt = http(
        r.port,
        "GET",
        &format!("/items/{item_id}/export?format=vtt"),
        &[("Authorization", &auth)],
        "",
    );
    assert_eq!(vtt.status, 200);
    assert!(vtt.body.starts_with("WEBVTT"), "{}", vtt.body);
    // Without the token, nothing.
    let anon = http(
        r.port,
        "GET",
        &format!("/items/{item_id}/export?format=srt"),
        &[],
        "",
    );
    assert_eq!(anon.status, 401);
    let open = http(
        r.port,
        "POST",
        &format!("/items/{}/open", item_id.replace('/', "%2F")),
        &[("Authorization", &auth), ("Origin", EXT)],
        "",
    );
    assert_eq!(open.status, 200);
    assert_eq!(*r.host.0.opened.lock().unwrap(), vec![item_id.clone()]);
}

/// Teams (#245) and Zoom (#246) pages send the same protocol 2 events as
/// Meet; their names get the platform's own prefix.
#[test]
fn teams_and_zoom_meetings_name_remote_lines_with_their_platform_prefix() {
    for (platform, url, source, id, expected) in [
        (
            "teams",
            "https://teams.cloud.microsoft/v2/",
            "rtp",
            "csrc:3001",
            "teams:Anna Rossi",
        ),
        (
            "zoom",
            "https://app.zoom.us/wc/123/join",
            "rtp",
            "ssrc:4001",
            "zoom:Anna Rossi",
        ),
    ] {
        let r = start_server();
        let mut ws = ws_connect(r.port, TOKEN, Some(EXT)).expect("upgrade");
        read_json(&mut ws).unwrap();
        ws.send(Message::text(format!(
            r#"{{"type":"start","title":"Sync","url":"{url}","platform":"{platform}","rate":48000,"channels":2}}"#
        )))
        .unwrap();
        // The remote side speaks 3.5–6 s; the page's source (Teams' CSRC,
        // Zoom's SSRC) is bound to a name and active over it.
        for n in 0..450usize {
            let t = n as f32 * 0.02;
            let remote_amp = if (3.5..6.0).contains(&t) { 0.4 } else { 0.0 };
            ws.send(Message::binary(protocol::encode_audio_frame(
                0,
                n as u32,
                &frame_48k(n * 960, 0.0),
            )))
            .unwrap();
            ws.send(Message::binary(protocol::encode_audio_frame(
                1,
                n as u32,
                &frame_48k(n * 960, remote_amp),
            )))
            .unwrap();
            if n == 170 {
                ws.send(Message::text(format!(
                    r#"{{"type":"speaker_name","id":"{id}","name":"Anna Rossi"}}"#
                )))
                .unwrap();
                ws.send(Message::text(format!(
                    r#"{{"type":"speaker_active","id":"{id}","source":"{source}","t":3400}}"#
                )))
                .unwrap();
            }
            if n == 310 {
                ws.send(Message::text(format!(
                    r#"{{"type":"speaker_idle","id":"{id}","t":6200}}"#
                )))
                .unwrap();
            }
        }
        ws.send(Message::text(r#"{"type":"stop"}"#)).unwrap();
        let mut done_item = None;
        let mut announced = Vec::new();
        while let Some(m) = read_json(&mut ws) {
            if m["type"] == "speaker" {
                announced.push((
                    m["id"].as_str().unwrap_or_default().to_string(),
                    m["label"].as_str().unwrap_or_default().to_string(),
                ));
            }
            if m["type"] == "status" && m["state"] == "done" {
                done_item = m["item_id"].as_str().map(str::to_string);
            }
        }
        let item_id = done_item.expect("done with an item id");
        let item = archive::read_item(&r.host.0.archive, &item_id).unwrap();
        let remote = item
            .segments
            .segments
            .iter()
            .find(|s| s.channel == Channel::Remote)
            .unwrap();
        assert_eq!(remote.speaker_id.as_deref(), Some(expected), "{platform}");
        let listed: Vec<(&str, &str)> = item
            .segments
            .speakers
            .iter()
            .map(|s| (s.id.as_str(), s.label.as_str()))
            .collect();
        assert!(listed.contains(&(expected, "Anna Rossi")), "{listed:?}");
        assert!(
            announced.contains(&(expected.to_string(), "Anna Rossi".to_string())),
            "{announced:?}"
        );
        let names: Vec<&str> = item
            .meta
            .participants
            .iter()
            .map(|p| p.name.as_str())
            .collect();
        assert_eq!(names, ["Anna Rossi"], "{platform}");
    }
}

#[test]
fn app_languages_lists_the_engine_languages_for_the_extension_only() {
    use crate::stt::languages::LanguageSet;
    let r = start_server();
    let auth = bearer();
    let get = |headers: &[(&str, &str)]| http(r.port, "GET", "/app/languages", headers, "");
    let ok = get(&[("Authorization", &auth), ("Origin", EXT)]);
    assert_eq!(ok.status, 200);
    assert_eq!(ok.header("Access-Control-Allow-Origin"), Some(EXT));
    let v = ok.json();
    assert_eq!(v["engine"], "whisper");
    assert_eq!(v["default"], "en", "the dictation's language");
    let langs = v["languages"].as_array().unwrap();
    assert_eq!(langs.len(), 99);
    assert!(
        langs
            .iter()
            .any(|l| l["code"] == "it" && l["name"] == "Italiano"),
        "{v}"
    );
    assert!(
        !langs.iter().any(|l| l["code"] == "auto"),
        "auto is implied"
    );
    // It follows the engine and the dictation setting.
    {
        let mut c = r.host.0.config.lock().unwrap();
        c.languages = LanguageSet::Parakeet;
        c.dictation_language = "IT-it".into();
    }
    let v = get(&[("Authorization", &auth), ("Origin", EXT)]).json();
    assert_eq!(
        (v["engine"].as_str(), v["default"].as_str()),
        (Some("parakeet"), Some("it"))
    );
    assert_eq!(v["languages"].as_array().unwrap().len(), 25);
    r.host.0.config.lock().unwrap().dictation_language = String::new();
    assert_eq!(get(&[("Authorization", &auth)]).json()["default"], "auto");
    // Behind the extension token, extension origins only (#126, #215).
    assert_eq!(get(&[("Origin", EXT)]).status, 401);
    assert_eq!(get(&[("Authorization", "Bearer 00")]).status, 401);
    assert_eq!(
        get(&[("Authorization", &auth), ("Origin", "https://evil.example")]).status,
        403
    );
    let pre = http(r.port, "OPTIONS", "/app/languages", &[("Origin", EXT)], "");
    assert_eq!(pre.status, 204);
}

/// One short meeting over `/live` whose `start` carries `extra` JSON
/// fields: the item id and the warnings the app sent.
fn short_meeting(port: u16, extra: &str) -> (String, Vec<String>) {
    let mut ws = ws_connect(port, TOKEN, Some(EXT)).expect("upgrade");
    read_json(&mut ws).unwrap();
    ws.send(Message::text(format!(
        r#"{{"type":"start","title":"Sync","url":"https://meet.google.com/abc-defg-hij","platform":"meet","rate":48000,"channels":2{extra}}}"#
    )))
    .unwrap();
    // 4 s: the remote side speaks 0.5–2.5 s.
    for n in 0..200usize {
        let t = n as f32 * 0.02;
        let amp = if (0.5..2.5).contains(&t) { 0.4 } else { 0.0 };
        ws.send(Message::binary(protocol::encode_audio_frame(
            0,
            n as u32,
            &frame_48k(n * 960, 0.0),
        )))
        .unwrap();
        ws.send(Message::binary(protocol::encode_audio_frame(
            1,
            n as u32,
            &frame_48k(n * 960, amp),
        )))
        .unwrap();
    }
    ws.send(Message::text(r#"{"type":"stop"}"#)).unwrap();
    let mut warnings = Vec::new();
    let mut done = None;
    while let Some(m) = read_json(&mut ws) {
        if m["type"] != "status" {
            continue;
        }
        assert_ne!(m["state"], "error", "{m}");
        if m["state"] == "warning" {
            warnings.push(m["message"].as_str().unwrap_or_default().to_string());
        }
        if m["state"] == "done" {
            done = m["item_id"].as_str().map(str::to_string);
        }
    }
    (done.expect("done with an item"), warnings)
}

#[test]
fn a_meeting_runs_in_the_language_the_side_panel_chose() {
    let r = start_server();
    let seen = || std::mem::take(&mut *r.host.0.languages_seen.lock().unwrap());
    // The dictation setting is English; the panel chose Italian (#288):
    // the STT hint, the cleanup and the frontmatter all take it.
    let (id, warnings) = short_meeting(r.port, r#","language":"it""#);
    assert!(warnings.is_empty(), "{warnings:?}");
    let item = archive::read_item(&r.host.0.archive, &id).unwrap();
    assert_eq!(item.meta.language, "it");
    let langs = seen();
    assert!(
        langs.contains(&"stt:it".to_string()) && langs.contains(&"cleanup:it".to_string()),
        "{langs:?}"
    );
    assert!(langs.iter().all(|l| l.ends_with(":it")), "{langs:?}");
    assert_eq!(
        r.host.0.settings.lock().unwrap().language,
        "en",
        "the global setting is untouched"
    );

    // An older extension sends none: the dictation's language.
    let (id, warnings) = short_meeting(r.port, "");
    assert!(warnings.is_empty(), "{warnings:?}");
    assert_eq!(
        archive::read_item(&r.host.0.archive, &id)
            .unwrap()
            .meta
            .language,
        "en"
    );
    assert!(seen().iter().all(|l| l.ends_with(":en")));

    // A language the engine doesn't offer: warned, dictation language.
    r.host.0.config.lock().unwrap().languages = crate::stt::languages::LanguageSet::Parakeet;
    let (id, warnings) = short_meeting(r.port, r#","language":"ja""#);
    assert!(
        warnings.iter().any(|w| w.contains("\"ja\"")),
        "{warnings:?}"
    );
    assert_eq!(
        archive::read_item(&r.host.0.archive, &id)
            .unwrap()
            .meta
            .language,
        "en"
    );
    seen();

    // Auto-detect: the cleanup and the frontmatter take the language the
    // STT detected (#218; the fake detects English).
    let (id, _) = short_meeting(r.port, r#","language":"auto""#);
    assert_eq!(
        archive::read_item(&r.host.0.archive, &id)
            .unwrap()
            .meta
            .language,
        "en"
    );
    let langs = seen();
    assert!(langs.contains(&"stt:auto".to_string()), "{langs:?}");
    assert!(
        langs.contains(&"cleanup:en".to_string()),
        "fake STT detects en: {langs:?}"
    );
}

#[test]
fn a_dropped_connection_still_keeps_the_meeting() {
    let r = start_server();
    let mut ws = ws_connect(r.port, TOKEN, Some(EXT)).expect("upgrade");
    read_json(&mut ws).unwrap();
    ws.send(Message::text(
        r#"{"type":"start","url":"https://teams.microsoft.com/l/meetup","platform":"teams","rate":16000,"channels":1}"#,
    ))
    .unwrap();
    for n in 0..150u32 {
        // 3 s of remote speech at 16 kHz, 20 ms frames.
        let pcm: Vec<i16> = (0..320)
            .map(|i| ((0.4 * ((n * 320 + i) as f32 * 0.05).sin()) * 32_767.0) as i16)
            .collect();
        ws.send(Message::binary(protocol::encode_audio_frame(1, n, &pcm)))
            .unwrap();
    }
    // A second start on the same connection is refused politely.
    ws.send(Message::text(
        r#"{"type":"start","url":"https://teams.microsoft.com/","rate":16000,"channels":1}"#,
    ))
    .unwrap();
    // Gone without `stop`.
    drop(ws);
    // The run finishes on its own; the item is kept, titled from its text.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    loop {
        let items = archive::list_items(&r.host.0.archive);
        if let Some(it) = items.iter().find(|i| !i.recording) {
            assert_eq!(it.meta.item_type, ItemType::Meeting);
            assert_eq!(it.meta.source, "browser:teams.microsoft.com");
            assert_eq!(it.meta.title, "hello from remote");
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the meeting was not written"
        );
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}

/// Connect to `/live` with no token in the URL (#217: first-message auth).
fn ws_connect_bare(port: u16, origin: Option<&str>) -> Result<Ws, u16> {
    let mut req = format!("ws://127.0.0.1:{port}/live")
        .into_client_request()
        .unwrap();
    if let Some(o) = origin {
        req.headers_mut().insert("Origin", o.parse().unwrap());
    }
    match tungstenite::connect(req) {
        Ok((ws, _)) => Ok(ws),
        Err(tungstenite::Error::Http(resp)) => Err(resp.status().as_u16()),
        Err(e) => panic!("unexpected WebSocket error: {e}"),
    }
}

fn auth_message(token: &str) -> Message {
    Message::text(format!(r#"{{"type":"auth","token":"{token}"}}"#))
}

/// The first status after a failed first-message auth, then the close.
fn refused(ws: &mut Ws) -> String {
    let m = read_json(ws).expect("an error status");
    assert_eq!(m["state"], "error", "{m}");
    assert!(read_json(ws).is_none(), "closed after the error");
    m["message"].as_str().unwrap_or_default().to_string()
}

#[test]
fn live_takes_the_token_as_its_first_message() {
    let r = start_server();
    // The handshake says so.
    let v = http(
        r.port,
        "GET",
        "/app/version",
        &[("Authorization", &bearer()), ("Origin", EXT)],
        "",
    );
    assert_eq!(v.json()["live_auth"], "message");
    // Without a token the origin rules still apply before the upgrade.
    assert_eq!(ws_connect_bare(r.port, None).err(), Some(403));
    assert_eq!(
        ws_connect_bare(r.port, Some("https://meet.google.com")).err(),
        Some(403)
    );
    // …and so does the #215 guard: a rebinding `Host` is refused first.
    let mut req = format!("ws://127.0.0.1:{}/live", r.port)
        .into_client_request()
        .unwrap();
    req.headers_mut().insert("Origin", EXT.parse().unwrap());
    req.headers_mut().insert(
        "Host",
        format!("rebind.attacker:{}", r.port).parse().unwrap(),
    );
    match tungstenite::connect(req) {
        Err(tungstenite::Error::Http(resp)) => assert_eq!(resp.status().as_u16(), 403),
        other => panic!("expected a refusal, got {:?}", other.map(|_| ())),
    }
    // Nothing is sent before the auth; the right token gets `ready`.
    let mut ws = ws_connect_bare(r.port, Some(EXT)).expect("upgrade");
    ws.send(auth_message(TOKEN)).unwrap();
    let ready = read_json(&mut ws).unwrap();
    assert_eq!(
        (ready["type"].as_str(), ready["state"].as_str()),
        (Some("status"), Some("ready"))
    );
    ws.send(Message::text(r#"{"type":"ping"}"#)).unwrap();
    assert_eq!(read_json(&mut ws).unwrap()["state"], "ready");
    drop(ws);
    // A wrong token, or anything else first, is refused and closed.
    let mut ws = ws_connect_bare(r.port, Some(EXT)).expect("upgrade");
    ws.send(auth_message("0000")).unwrap();
    assert_eq!(refused(&mut ws), "wrong extension token");
    let mut ws = ws_connect_bare(r.port, Some(EXT)).expect("upgrade");
    ws.send(Message::text(
        r#"{"type":"start","url":"https://meet.google.com/a","rate":16000,"channels":1}"#,
    ))
    .unwrap();
    assert_eq!(refused(&mut ws), "authentication required");
    assert!(
        archive::list_items(&r.host.0.archive).is_empty(),
        "no meeting started"
    );
    // Too late, even with the right token.
    let mut ws = ws_connect_bare(r.port, Some(EXT)).expect("upgrade");
    std::thread::sleep(live::AUTH_TIMEOUT + std::time::Duration::from_millis(200));
    ws.send(auth_message(TOKEN)).unwrap();
    assert_eq!(refused(&mut ws), "authentication timed out");
}

#[test]
fn an_unpaired_app_refuses_live_without_a_token_too() {
    let r = start_server();
    r.host.0.config.lock().unwrap().extension_token = String::new();
    assert_eq!(ws_connect_bare(r.port, Some(EXT)).err(), Some(401));
}

// ---- #215: Host / Origin / body caps / worker pool / item filter ----------

/// DNS rebinding: a page at `rebind.attacker:<port>` re-resolved to
/// 127.0.0.1 sends its own name as `Host` — refused on every route, before
/// the route runs, token or not.
#[test]
fn a_foreign_host_is_refused_on_every_route() {
    let r = start_server();
    let auth = bearer();
    let rebind = format!("rebind.attacker:{}", r.port);
    let other_port = format!("127.0.0.1:{}", r.port.wrapping_add(1));
    for host in [rebind.as_str(), other_port.as_str(), "127.0.0.1", "[::1]"] {
        for (method, path) in [
            ("GET", "/history?n=100000"),
            ("POST", "/clean"),
            ("POST", "/transcribe?ext=wav"),
            ("GET", "/app/version"),
            ("GET", "/app/languages"),
            ("GET", "/items/2026/09/x/export"),
            ("POST", "/items/2026/09/x/open"),
            ("OPTIONS", "/app/version"),
            ("GET", "/nope"),
            ("GET", "/archive/items"),
            ("POST", "/archive/items"),
        ] {
            let reply = http(
                r.port,
                method,
                path,
                &[("Host", host), ("Authorization", &auth)],
                "text",
            );
            assert_eq!(reply.status, 403, "{host} {method} {path}");
            assert_eq!(reply.json()["error"], "host not allowed");
            assert!(reply.header("Access-Control-Allow-Origin").is_none());
        }
    }
    assert_eq!(r.host.0.calls.load(Ordering::SeqCst), 0, "no route ran");
    // Both loopback names on the right port work.
    for host in [
        format!("127.0.0.1:{}", r.port),
        format!("localhost:{}", r.port),
    ] {
        assert_eq!(
            http(r.port, "GET", "/history", &[("Host", &host)], "").status,
            200
        );
    }
    // The WebSocket too, even with the token and an extension origin.
    let mut req = format!("ws://127.0.0.1:{}/live?token={TOKEN}", r.port)
        .into_client_request()
        .unwrap();
    req.headers_mut().insert("Origin", EXT.parse().unwrap());
    req.headers_mut().insert("Host", rebind.parse().unwrap());
    match tungstenite::connect(req) {
        Err(tungstenite::Error::Http(resp)) => assert_eq!(resp.status().as_u16(), 403),
        other => panic!("expected a refusal, got {:?}", other.map(|_| ())),
    }
}

/// A web page's cross-site "simple" POST (or any fetch) carries its
/// `Origin`: refused on the token-less routes too, so it can't write
/// history or spend the cleanup LLM. Scripts send no `Origin`.
#[test]
fn a_web_origin_is_refused_on_the_token_less_routes() {
    let r = start_server();
    let own = format!("http://127.0.0.1:{}", r.port);
    for origin in ["https://evil.example", "null", own.as_str()] {
        for (method, path) in [
            ("POST", "/clean"),
            ("POST", "/transcribe?ext=wav"),
            ("GET", "/history"),
        ] {
            let reply = http(
                r.port,
                method,
                path,
                &[("Origin", origin), ("Content-Type", "text/plain")],
                "RIFF text",
            );
            assert_eq!(reply.status, 403, "{origin} {method} {path}");
            assert_eq!(reply.json()["error"], "origin not allowed");
            assert!(reply.header("Access-Control-Allow-Origin").is_none());
        }
    }
    assert_eq!(
        r.host.0.calls.load(Ordering::SeqCst),
        0,
        "nothing was cleaned or transcribed"
    );
    // No origin (curl) or an extension's: allowed.
    assert_eq!(http(r.port, "POST", "/clean", &[], "ciao").status, 200);
    assert_eq!(
        http(r.port, "POST", "/clean", &[("Origin", EXT)], "ciao").status,
        200
    );
}

/// `Settings.api_scripting` off (a new install): the token-less routes are
/// refused with an explanation; the extension's routes still work. Read
/// per request, so switching applies at once.
#[test]
fn the_scripting_routes_answer_only_when_switched_on() {
    let r = start_server();
    r.host.0.config.lock().unwrap().scripting = false;
    for (method, path) in [
        ("POST", "/clean"),
        ("POST", "/transcribe?ext=wav"),
        ("GET", "/history"),
    ] {
        let reply = http(r.port, method, path, &[], "RIFF");
        assert_eq!(reply.status, 403, "{method} {path}");
        assert!(reply.json()["error"]
            .as_str()
            .unwrap()
            .contains("Scripting routes"));
    }
    assert_eq!(r.host.0.calls.load(Ordering::SeqCst), 0);
    let auth = bearer();
    assert_eq!(
        http(
            r.port,
            "GET",
            "/app/version",
            &[("Authorization", &auth)],
            ""
        )
        .status,
        200
    );
    r.host.0.config.lock().unwrap().scripting = true;
    assert_eq!(http(r.port, "POST", "/clean", &[], "ciao").status, 200);
}

/// Bodies over the cap: a declared length is refused before a byte is read
/// (with or without `Expect: 100-continue` — the client is never told to
/// send), and a chunked body is cut one byte past the cap.
#[test]
fn bodies_over_the_cap_are_refused_without_reading_them() {
    let r = start_server();
    let port = r.port;
    for expect in ["", "Expect: 100-continue\r\n"] {
        let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
        let big = guard::TRANSCRIBE_MAX_BYTES + 1;
        write!(
            s,
            "POST /transcribe?ext=wav HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n{expect}Content-Length: {big}\r\n\r\n"
        )
        .unwrap();
        // Nothing of the body was sent: the answer comes anyway.
        let reply = read_reply(&mut s);
        assert_eq!(reply.status, 413, "{expect:?}");
        assert!(reply.json()["error"].as_str().unwrap().contains("200 MiB"));
    }
    // An absurd length is answered at once; the handler never allocates it.
    let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
    write!(
        s,
        "POST /clean HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Length: 18446744073709551615\r\n\r\n"
    )
    .unwrap();
    assert_eq!(read_reply(&mut s).status, 413);
    drop(s);

    // Chunked: no declared length; 1 MiB + 64 KiB of text to /clean.
    let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
    write!(
        s,
        "POST /clean HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nTransfer-Encoding: chunked\r\n\r\n"
    )
    .unwrap();
    let mut writer = s.try_clone().unwrap();
    let sender = std::thread::spawn(move || {
        let chunk = vec![b'a'; 64 * 1024];
        for _ in 0..17 {
            if write!(writer, "{:x}\r\n", chunk.len()).is_err()
                || writer.write_all(&chunk).is_err()
                || writer.write_all(b"\r\n").is_err()
            {
                return;
            }
        }
        let _ = writer.write_all(b"0\r\n\r\n");
    });
    let reply = read_reply(&mut s);
    assert_eq!(reply.status, 413);
    assert!(reply.json()["error"].as_str().unwrap().contains("1 MiB"));
    let _ = s.shutdown(std::net::Shutdown::Both);
    drop(s);
    let _ = sender.join();
    assert_eq!(
        r.host.0.calls.load(Ordering::SeqCst),
        0,
        "no host call for a refused body"
    );

    // Right at the cap, it goes through.
    let text = "a".repeat(guard::CLEAN_MAX_BYTES);
    assert_eq!(http(port, "POST", "/clean", &[], &text).status, 200);
}

/// A slow `/transcribe` runs on its own worker: the extension's
/// `/app/version` and `/history` answer meanwhile; with every slow slot
/// taken, one more `/clean` gets 503 instead of waiting.
#[test]
fn a_slow_transcribe_does_not_block_the_other_routes() {
    let r = start_server();
    let port = r.port;
    let slow: Vec<_> = (0..SLOW_SLOTS)
        .map(|_| {
            std::thread::spawn(move || {
                http(port, "POST", "/transcribe?ext=slow", &[], "RIFF").status
            })
        })
        .collect();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while r.host.0.slow_running.load(Ordering::SeqCst) < SLOW_SLOTS {
        assert!(
            std::time::Instant::now() < deadline,
            "the slow requests never started"
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let auth = bearer();
    let t0 = std::time::Instant::now();
    let v = http(
        port,
        "GET",
        "/app/version",
        &[("Authorization", &auth), ("Origin", EXT)],
        "",
    );
    assert_eq!(v.status, 200);
    assert_eq!(http(port, "GET", "/history", &[], "").status, 200);
    assert!(
        t0.elapsed() < std::time::Duration::from_secs(2),
        "{:?}",
        t0.elapsed()
    );
    let busy = http(port, "POST", "/clean", &[], "ciao");
    assert_eq!(busy.status, 503);
    assert_eq!(busy.header("Retry-After"), Some("5"));
    // The WebSocket upgrade too.
    let ws = ws_connect(port, TOKEN, Some(EXT));
    assert!(ws.is_ok());
    drop(ws);
    r.host.0.release_slow.store(true, Ordering::SeqCst);
    for t in slow {
        assert_eq!(t.join().unwrap(), 200);
    }
    // The slots are free again.
    assert_eq!(http(port, "POST", "/clean", &[], "ciao").status, 200);
}

/// `/items/*/open|export` reach only what the extension recorded; another
/// item is refused with a code the extension explains.
#[test]
fn the_item_routes_reach_only_extension_items() {
    let r = start_server();
    let archive = &r.host.0.archive;
    let meta = |source: &str| archive::ItemMeta {
        item_type: ItemType::Note,
        title: "Private note".into(),
        date: "2026-09-24T10:00:00+02:00".into(),
        source: source.into(),
        ..Default::default()
    };
    let segs = archive::SegmentsFile::default();
    let note = archive::create_item(archive, &meta("mic"), &segs).unwrap();
    let meeting = archive::create_item(archive, &meta("browser:meet.google.com"), &segs).unwrap();
    let auth = bearer();
    let h = [("Authorization", auth.as_str()), ("Origin", EXT)];
    let export = http(
        r.port,
        "GET",
        &format!("/items/{note}/export?format=md"),
        &h,
        "",
    );
    assert_eq!(export.status, 403);
    assert_eq!(export.json()["code"], export::NOT_EXTENSION_ITEM);
    assert!(!export.body.contains("Private note"));
    assert_eq!(
        export.header("Access-Control-Allow-Origin"),
        Some(EXT),
        "readable by the extension"
    );
    let open = http(r.port, "POST", &format!("/items/{note}/open"), &h, "");
    assert_eq!(open.status, 403);
    assert_eq!(open.json()["code"], export::NOT_EXTENSION_ITEM);
    assert!(r.host.0.opened.lock().unwrap().is_empty());
    // The extension's own item still works.
    assert_eq!(
        http(
            r.port,
            "GET",
            &format!("/items/{meeting}/export?format=md"),
            &h,
            ""
        )
        .status,
        200
    );
    assert_eq!(
        http(r.port, "POST", &format!("/items/{meeting}/open"), &h, "").status,
        200
    );
}

// ---- #223: hostile framing against the (vendored, patched) tiny_http -------

/// Resident memory of this process, KiB.
#[cfg(unix)]
fn rss_kib() -> u64 {
    let out = std::process::Command::new("ps")
        .args(["-o", "rss=", "-p", &std::process::id().to_string()])
        .output()
        .expect("ps");
    String::from_utf8_lossy(&out.stdout)
        .trim()
        .parse()
        .expect("rss")
}

/// Send a raw request head (CRLFs included), then read one reply.
fn raw(port: u16, head: &str) -> (TcpStream, Reply) {
    let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
    s.write_all(head.as_bytes()).unwrap();
    let reply = read_reply(&mut s);
    (s, reply)
}

/// After a reply, does the server end the connection (EOF or reset within
/// 5 s) instead of waiting to read the rest of a body?
fn server_closes(s: &mut TcpStream) -> bool {
    s.set_read_timeout(Some(std::time::Duration::from_secs(5)))
        .unwrap();
    let mut buf = [0u8; 1024];
    loop {
        match s.read(&mut buf) {
            Ok(0) => return true,
            Ok(_) => continue,
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                return false
            }
            Err(_) => return true,
        }
    }
}

/// Send `prefix`, then `filler` over and over (at most `max` bytes in all)
/// from another thread while this one reads the reply. Returns the reply
/// (status 0 if the connection was reset first) and the bytes the server
/// let the client write before it ended the connection.
fn flood(port: u16, prefix: String, filler: &'static [u8], max: usize) -> (Reply, usize) {
    let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
    let mut w = s.try_clone().unwrap();
    let writer = std::thread::spawn(move || {
        let mut sent = 0;
        if w.write_all(prefix.as_bytes()).is_err() {
            return sent;
        }
        sent += prefix.len();
        let block: Vec<u8> = filler.iter().copied().cycle().take(64 * 1024).collect();
        while sent < max {
            let n = block.len().min(max - sent);
            if w.write_all(&block[..n]).is_err() {
                break;
            }
            sent += n;
        }
        sent
    });
    let reply = read_reply(&mut s);
    let _ = s.shutdown(std::net::Shutdown::Both);
    (reply, writer.join().unwrap())
}

/// The API still answers normally.
fn healthy(port: u16) -> bool {
    http(port, "GET", "/history", &[], "").status == 200
}

/// #223: upstream tiny_http 0.12.0 read an unfinished body into a buffer of
/// the size the client declared when the request dropped: this very test
/// aborted the whole process with `memory allocation of
/// 9223372036854775807 bytes failed` (and `100000000000000`), and
/// `18446744073709551615` panicked. Now every route answers without
/// reading or allocating the body, and closes the connection.
#[test]
fn an_absurd_content_length_is_answered_without_reading_it() {
    let r = start_server();
    let port = r.port;
    #[cfg(unix)]
    let before = rss_kib();
    let local = format!("127.0.0.1:{port}");
    for len in [
        "9223372036854775807",
        "18446744073709551615",
        "100000000000000",
        "10000000000",
    ] {
        for (method, path, host, extra, status) in [
            // Answered without reading the body (a GET with one).
            ("GET", "/history", local.as_str(), "", 200),
            // Refused: over the cap, with and without `Expect`.
            ("POST", "/clean", &local, "", 413),
            (
                "POST",
                "/transcribe?ext=wav",
                &local,
                "Expect: 100-continue\r\n",
                413,
            ),
            ("GET", "/nope", &local, "", 404),
            ("POST", "/clean", "rebind.attacker", "", 403),
            (
                "POST",
                "/clean",
                &local,
                "Origin: https://evil.example\r\n",
                403,
            ),
        ] {
            let (mut s, reply) = raw(
                port,
                &format!("{method} {path} HTTP/1.1\r\nHost: {host}\r\n{extra}Content-Length: {len}\r\n\r\n"),
            );
            assert_eq!(
                reply.status, status,
                "{method} {path} {extra:?} Content-Length: {len}"
            );
            assert!(
                server_closes(&mut s),
                "closed, not drained: {method} {path} {len}"
            );
        }
    }
    // A length that does not fit, or that is not a number, is a 400 — not
    // "no body" with the body then read as the next request.
    for len in ["18446744073709551616", "-1", "+5", "0x10", "5, 6"] {
        let (mut s, reply) = raw(
            port,
            &format!("POST /clean HTTP/1.1\r\nHost: {local}\r\nContent-Length: {len}\r\n\r\nGET /history HTTP/1.1\r\n\r\n"),
        );
        assert_eq!(reply.status, 400, "Content-Length: {len}");
        assert!(server_closes(&mut s));
    }
    assert!(healthy(port));
    assert_eq!(r.host.0.calls.load(Ordering::SeqCst), 0, "no host call");
    #[cfg(unix)]
    {
        let grown = rss_kib().saturating_sub(before);
        assert!(grown < 256 * 1024, "RSS grew {grown} KiB");
    }
}

/// #223: a body that never finishes arriving holds neither a worker nor a
/// slot. Routes that don't read a body answer at once and close; an upload
/// that stops mid-way gets 408 after the socket timeout.
#[test]
fn a_body_that_never_arrives_holds_no_worker() {
    let r = start_server_with(Some(tiny_http::Limits {
        io_timeout: Some(std::time::Duration::from_secs(1)),
        ..Default::default()
    }));
    let port = r.port;
    // Twice as many silent 10 GB "uploads" as there are workers.
    let held: Vec<TcpStream> = (0..WORKERS * 2)
        .map(|_| {
            let (s, reply) = raw(
                port,
                &format!("GET /history HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Length: 10000000000\r\n\r\npartial"),
            );
            assert_eq!(reply.status, 200);
            s
        })
        .collect();
    let t = std::time::Instant::now();
    let v = http(
        port,
        "GET",
        "/app/version",
        &[("Authorization", &bearer()), ("Origin", EXT)],
        "",
    );
    assert_eq!(v.status, 200);
    assert!(
        t.elapsed() < std::time::Duration::from_secs(2),
        "{:?}",
        t.elapsed()
    );
    // An upload under the cap that stops arriving: 408.
    let t = std::time::Instant::now();
    let (_s, reply) = raw(
        port,
        &format!("POST /transcribe?ext=wav HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Length: 104857600\r\n\r\nRIFF"),
    );
    assert_eq!(reply.status, 408, "{}", reply.body);
    assert!(
        t.elapsed() < std::time::Duration::from_secs(10),
        "{:?}",
        t.elapsed()
    );
    // Chunked: a chunk announced, never sent.
    let (_s2, reply) = raw(
        port,
        &format!("POST /clean HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nTransfer-Encoding: chunked\r\n\r\nfffff\r\nab"),
    );
    assert_eq!(reply.status, 408, "{}", reply.body);
    assert_eq!(r.host.0.calls.load(Ordering::SeqCst), 0);
    // Both slow slots are free again.
    for _ in 0..SLOW_SLOTS + 1 {
        assert_eq!(http(port, "POST", "/clean", &[], "hello").status, 200);
    }
    drop(held);
}

/// #223: headers, header lines and chunk-size lines are bounded — upstream
/// buffered a line until its CRLF and any number of headers, so a local
/// process could grow the app's memory without limit. The floods offer up
/// to 1 GiB; the server ends the connection after its limit.
#[test]
fn oversized_heads_and_framing_lines_are_cut_short() {
    let r = start_server();
    let port = r.port;
    #[cfg(unix)]
    let before = rss_kib();
    const GIB: usize = 1 << 30;
    // What the server reads before it stops, plus socket buffers.
    const STOPPED: usize = 64 << 20;
    let host = format!("Host: 127.0.0.1:{port}\r\n");

    // 101 short headers, all sent: 431. 100 in all still pass.
    for (count, status) in [(101, 431), (100, 200)] {
        let mut head = format!("GET /history HTTP/1.1\r\n{host}");
        for i in 1..count {
            head.push_str(&format!("X-{i}: v\r\n"));
        }
        head.push_str("\r\n");
        assert_eq!(raw(port, &head).1.status, status, "{count} headers");
    }

    // 10k headers; a 1 MB header line; an endless header line; an endless
    // request line; an endless chunk-size line.
    let cases: [(String, &'static [u8], usize, u16); 5] = [
        (
            format!("GET /history HTTP/1.1\r\n{host}"),
            b"X-H: v\r\n",
            10_000 * 8,
            431,
        ),
        (
            format!("GET /history HTTP/1.1\r\n{host}X-Big: "),
            b"a",
            1 << 20,
            431,
        ),
        (
            format!("GET /history HTTP/1.1\r\n{host}X-Big: "),
            b"a",
            GIB,
            431,
        ),
        ("GET /".to_string(), b"a", GIB, 431),
        (
            format!("POST /clean HTTP/1.1\r\n{host}Transfer-Encoding: chunked\r\n\r\n"),
            b"1",
            GIB,
            400,
        ),
    ];
    for (prefix, filler, max, status) in cases {
        let what = format!("{prefix:?} + {:?} x {max}", String::from_utf8_lossy(filler));
        let (reply, sent) = flood(port, prefix, filler, max);
        // The answer, unless the reset for the unread rest overtook it.
        assert!(
            reply.status == status || reply.status == 0,
            "{what}: {}",
            reply.status
        );
        if max > STOPPED {
            assert!(sent < STOPPED, "{what}: the server read on ({sent} bytes)");
        }
        assert!(healthy(port), "{what}");
    }
    assert_eq!(r.host.0.calls.load(Ordering::SeqCst), 0);
    #[cfg(unix)]
    {
        let grown = rss_kib().saturating_sub(before);
        assert!(grown < 256 * 1024, "RSS grew {grown} KiB");
    }
}

/// #223: connections over the limit are closed as soon as they are
/// accepted (each open one holds a thread); closing some frees room.
#[test]
fn connections_over_the_limit_are_closed_at_accept() {
    let r = start_server_with(Some(tiny_http::Limits {
        max_connections: 4,
        ..Default::default()
    }));
    let port = r.port;
    let idle: Vec<TcpStream> = (0..4)
        .map(|_| TcpStream::connect(("127.0.0.1", port)).unwrap())
        .collect();
    // Let the accept thread count them.
    std::thread::sleep(std::time::Duration::from_millis(300));
    let mut extra = TcpStream::connect(("127.0.0.1", port)).unwrap();
    assert!(server_closes(&mut extra), "the fifth connection is closed");
    drop(idle);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
        let ok = write!(
            s,
            "GET /history HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n"
        )
        .is_ok()
            && read_reply(&mut s).status == 200;
        if ok {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "room again once the idle ones close"
        );
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}

/// #223: the socket timeout is for HTTP only — an upgraded `/live` stream
/// waits for the extension's messages as long as it likes, while an idle
/// keep-alive connection is closed.
#[test]
fn live_outlasts_the_http_timeout() {
    let r = start_server_with(Some(tiny_http::Limits {
        io_timeout: Some(std::time::Duration::from_millis(500)),
        ..Default::default()
    }));
    let mut ws = ws_connect_bare(r.port, Some(EXT)).expect("upgrade");
    ws.send(auth_message(TOKEN)).unwrap();
    assert_eq!(read_json(&mut ws).unwrap()["state"], "ready");
    std::thread::sleep(std::time::Duration::from_millis(1500));
    ws.send(Message::text(r#"{"type":"ping"}"#)).unwrap();
    assert_eq!(read_json(&mut ws).unwrap()["state"], "ready");
    // Plain HTTP: an idle keep-alive connection ends after the timeout.
    let (mut s, reply) = raw(
        r.port,
        &format!(
            "GET /history HTTP/1.1\r\nHost: 127.0.0.1:{}\r\n\r\n",
            r.port
        ),
    );
    assert_eq!(reply.status, 200);
    assert!(server_closes(&mut s));
    // A handler slower than the timeout still delivers its answer (the
    // timeout only ends the wait for a *next* request), and nothing else
    // follows it.
    let mut s = TcpStream::connect(("127.0.0.1", r.port)).unwrap();
    write!(
        s,
        "POST /transcribe?ext=slow HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nContent-Length: 4\r\n\r\nRIFF",
        r.port
    )
    .unwrap();
    while r.host.0.slow_running.load(Ordering::SeqCst) == 0 {
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    std::thread::sleep(std::time::Duration::from_millis(1500));
    r.host.0.release_slow.store(true, Ordering::SeqCst);
    let reply = read_reply(&mut s);
    assert_eq!(reply.status, 200);
    assert_eq!(reply.json()["bytes"], 4);
    let mut rest = Vec::new();
    s.set_read_timeout(Some(std::time::Duration::from_secs(5)))
        .unwrap();
    let _ = s.read_to_end(&mut rest);
    assert!(
        rest.is_empty(),
        "nothing after the answer: {:?}",
        String::from_utf8_lossy(&rest)
    );
}

// ---- archive API tokens (#249) ----------------------------------------------

use crate::api::tokens::{self as archive_tokens, Scope};

/// A token with `scopes`, added to the running server's config; returns the
/// `Authorization` header value and the token id.
fn add_archive_token(r: &Running, name: &str, scopes: &[Scope]) -> (String, String) {
    let mut config = r.host.0.config.lock().unwrap();
    let (stored, new) =
        archive_tokens::create(&config.archive_tokens, name, scopes, chrono::Utc::now()).unwrap();
    config.archive_tokens.push(stored);
    (format!("Bearer {}", new.token), new.info.id)
}

fn assert_no_cors(reply: &Reply, what: &str) {
    for (k, _) in &reply.headers {
        assert!(
            !k.to_ascii_lowercase().starts_with("access-control-"),
            "{what}: {k}"
        );
    }
}

#[test]
fn archive_routes_need_the_switch_a_token_and_no_origin() {
    let r = start_server();
    let (read, read_id) = add_archive_token(&r, "reader", &[Scope::Read]);
    // Off by default: refused, with a code scripts can branch on.
    let off = http(
        r.port,
        "GET",
        "/archive/items",
        &[("Authorization", &read)],
        "",
    );
    assert_eq!(
        (off.status, off.json()["code"].as_str()),
        (403, Some("archive_api_off"))
    );
    assert_no_cors(&off, "off");
    r.host.0.config.lock().unwrap().archive = true;

    // No token, a wrong one, the extension token, the token as a query.
    let ext = bearer();
    let q = format!("/archive/items?token={}", &read["Bearer ".len()..]);
    for (path, auth) in [
        ("/archive/items", None),
        ("/archive/items", Some("Bearer sua_wrong")),
        ("/archive/items", Some(ext.as_str())),
        (q.as_str(), None),
    ] {
        let headers: Vec<(&str, &str)> =
            auth.map(|a| vec![("Authorization", a)]).unwrap_or_default();
        let reply = http(r.port, "GET", path, &headers, "");
        assert_eq!(reply.status, 401, "{path} {auth:?}");
        assert_eq!(reply.json()["code"], "unauthorized");
        assert!(reply
            .header("WWW-Authenticate")
            .unwrap()
            .starts_with("Bearer"));
        assert_no_cors(&reply, "401");
    }
    // The right token with any browser Origin: an extension's is refused
    // by the archive middleware, a web page's already by the #215 guard.
    let from_ext = http(
        r.port,
        "GET",
        "/archive/items",
        &[("Authorization", &read), ("Origin", EXT)],
        "",
    );
    assert_eq!(
        (from_ext.status, from_ext.json()["code"].as_str()),
        (403, Some("origin_refused"))
    );
    assert_no_cors(&from_ext, "extension origin");
    let pre = http(r.port, "OPTIONS", "/archive/items", &[("Origin", EXT)], "");
    assert_eq!(pre.status, 403, "no preflight answer");
    assert_no_cors(&pre, "preflight");
    for origin in ["https://evil.example", "null"] {
        let web = http(
            r.port,
            "GET",
            "/archive/items",
            &[("Authorization", &read), ("Origin", origin)],
            "",
        );
        assert_eq!(
            (web.status, web.json()["error"].as_str()),
            (403, Some("origin not allowed"))
        );
        assert_no_cors(&web, origin);
    }
    assert!(
        r.host.0.tokens_used.lock().unwrap().is_empty(),
        "nothing got through yet"
    );

    // Authorized: past the middleware, into the read routes (#250).
    let ok = http(
        r.port,
        "GET",
        "/archive/items",
        &[("Authorization", &read)],
        "",
    );
    assert_eq!((ok.status, ok.json()["total"].as_u64()), (200, Some(0)));
    assert_no_cors(&ok, "authorized");
    assert_eq!(*r.host.0.tokens_used.lock().unwrap(), vec![read_id.clone()]);

    // The archive token opens nothing else: not the extension's routes.
    assert_eq!(
        http(
            r.port,
            "GET",
            "/app/version",
            &[("Authorization", &read)],
            ""
        )
        .status,
        401
    );
}

#[test]
fn archive_scopes_are_enforced_per_method() {
    let r = start_server();
    r.host.0.config.lock().unwrap().archive = true;
    let (read, _) = add_archive_token(&r, "reader", &[Scope::Read]);
    let (write, _) = add_archive_token(&r, "clipper", &[Scope::Write]);
    let (all, _) = add_archive_token(&r, "all", &[Scope::Read, Scope::People, Scope::Write]);
    let post = |auth: &str| {
        http(
            r.port,
            "POST",
            "/archive/items",
            &[("Authorization", auth)],
            r#"{"title":"x","text":"y"}"#,
        )
    };
    let get = |auth: &str| {
        http(
            r.port,
            "GET",
            "/archive/people",
            &[("Authorization", auth)],
            "",
        )
    };

    let denied = post(&read);
    assert_eq!(
        (denied.status, denied.json()["code"].as_str()),
        (403, Some("insufficient_scope"))
    );
    assert!(denied
        .header("WWW-Authenticate")
        .unwrap()
        .contains("scope=\"write\""));
    let denied = get(&write);
    assert_eq!(
        (denied.status, denied.json()["code"].as_str()),
        (403, Some("insufficient_scope"))
    );
    // Authorized: reads answer (#250), writes create a note (#251).
    for reply in [get(&read), get(&all)] {
        assert_eq!(reply.status, 200, "authorized");
    }
    for reply in [post(&write), post(&all)] {
        assert_eq!(reply.status, 201, "authorized: {}", reply.body);
    }
    let del = http(
        r.port,
        "DELETE",
        "/archive/items/x",
        &[("Authorization", &read)],
        "",
    );
    assert_eq!(del.status, 403, "anything but GET/HEAD needs write");
}

#[test]
fn a_revoked_archive_token_is_refused_at_once() {
    let r = start_server();
    r.host.0.config.lock().unwrap().archive = true;
    let (read, id) = add_archive_token(&r, "reader", &[Scope::Read]);
    assert_eq!(
        http(
            r.port,
            "GET",
            "/archive/items",
            &[("Authorization", &read)],
            ""
        )
        .status,
        200
    );
    assert!(archive_tokens::revoke(
        &mut r.host.0.config.lock().unwrap().archive_tokens,
        &id
    ));
    assert_eq!(
        http(
            r.port,
            "GET",
            "/archive/items",
            &[("Authorization", &read)],
            ""
        )
        .status,
        401
    );
    // Switching the API off refuses every token too.
    let (again, _) = add_archive_token(&r, "again", &[Scope::Read]);
    r.host.0.config.lock().unwrap().archive = false;
    assert_eq!(
        http(
            r.port,
            "GET",
            "/archive/items",
            &[("Authorization", &again)],
            ""
        )
        .status,
        403
    );
}

#[test]
fn archive_requests_are_rate_limited_per_token() {
    let r = start_server();
    r.host.0.config.lock().unwrap().archive = true;
    let (busy, _) = add_archive_token(&r, "busy", &[Scope::Read]);
    let (calm, _) = add_archive_token(&r, "calm", &[Scope::Read]);
    let mut passed = 0;
    let limited = loop {
        let reply = http(
            r.port,
            "GET",
            "/archive/items",
            &[("Authorization", &busy)],
            "",
        );
        if reply.status == 429 {
            break reply;
        }
        assert_eq!(reply.status, 200);
        passed += 1;
        assert!(passed < 1000, "never limited");
    };
    assert!(
        passed >= archive_tokens::RATE_BURST as usize,
        "the burst passes: {passed}"
    );
    assert_eq!(limited.json()["code"], "rate_limited");
    assert!(
        limited
            .header("Retry-After")
            .unwrap()
            .parse::<u64>()
            .unwrap()
            >= 1
    );
    assert_no_cors(&limited, "429");
    // Another token is unaffected.
    assert_eq!(
        http(
            r.port,
            "GET",
            "/archive/items",
            &[("Authorization", &calm)],
            ""
        )
        .status,
        200
    );
}

// ---- archive read routes (#250) ---------------------------------------------

/// A meeting with a participant email and a speaker embedding, a companion
/// document and a People registry, in the running server's archive.
fn seed_archive(r: &Running) -> String {
    let archive = &r.host.0.archive;
    let meta = archive::ItemMeta {
        item_type: ItemType::Meeting,
        title: "Weekly sync".into(),
        date: "2026-09-24T10:00:00+02:00".into(),
        source: "browser:meet.google.com".into(),
        participants: vec![archive::Participant {
            name: "Anna Rossi".into(),
            email: Some("anna@example.com".into()),
        }],
        ..Default::default()
    };
    let segs = archive::SegmentsFile {
        speakers: vec![archive::DocSpeaker {
            id: "voice:1".into(),
            label: "Anna Rossi".into(),
            person_id: Some("p-anna".into()),
            ..Default::default()
        }],
        segments: vec![archive::Segment {
            id: 0,
            channel: Channel::Remote,
            start_ms: 1_000,
            end_ms: 2_000,
            speaker_id: Some("voice:1".into()),
            text: "Parliamo della roadmap.".into(),
            embedding: Some(vec![0.987_654_3; 4]),
            ..Default::default()
        }],
        ..Default::default()
    };
    let id = archive::create_item(archive, &meta, &segs).unwrap();
    std::fs::write(
        archive.join(&id).join("document.md"),
        "---\ntitle: Minutes\n---\n\nAgreed.\n",
    )
    .unwrap();
    std::fs::create_dir_all(archive.join(".sussurro")).unwrap();
    std::fs::write(
        archive.join(".sussurro").join("people.json"),
        r#"{"version":1,"people":[{"id":"p-anna","name":"Anna Rossi","email":"anna@example.com","aliases":["Annina"]}]}"#,
    )
    .unwrap();
    id
}

/// Every read route: no token, a wrong token, a token without `read`, a
/// browser Origin and a foreign Host are refused before the route runs;
/// the right token gets an answer with no CORS and `no-store`.
#[test]
fn every_archive_read_route_is_guarded() {
    let r = start_server();
    r.host.0.config.lock().unwrap().archive = true;
    let id = seed_archive(&r);
    let (read, _) = add_archive_token(&r, "reader", &[Scope::Read]);
    let (write, _) = add_archive_token(&r, "clipper", &[Scope::Write]);
    let (people, _) = add_archive_token(&r, "people only", &[Scope::People]);
    let foreign = format!("rebind.attacker:{}", r.port);
    let routes = [
        "/archive/items".to_string(),
        "/archive/items?q=roadmap&limit=1".to_string(),
        format!("/archive/items/{id}"),
        format!("/archive/items/{id}/export?format=txt"),
        format!("/archive/items/{id}/documents"),
        format!("/archive/items/{id}/documents/document.md"),
        "/archive/people".to_string(),
    ];
    for path in &routes {
        let get = |headers: &[(&str, &str)]| http(r.port, "GET", path, headers, "");
        let none = get(&[]);
        assert_eq!(
            (none.status, none.json()["code"].as_str()),
            (401, Some("unauthorized")),
            "{path}"
        );
        let wrong = get(&[("Authorization", "Bearer sua_0000")]);
        assert_eq!(wrong.status, 401, "{path}");
        let ext_token = bearer();
        assert_eq!(
            get(&[("Authorization", &ext_token)]).status,
            401,
            "the extension token: {path}"
        );
        for scoped in [&write, &people] {
            let denied = get(&[("Authorization", scoped)]);
            assert_eq!(
                (denied.status, denied.json()["code"].as_str()),
                (403, Some("insufficient_scope")),
                "{path}"
            );
        }
        let from_ext = get(&[("Authorization", &read), ("Origin", EXT)]);
        assert_eq!(
            (from_ext.status, from_ext.json()["code"].as_str()),
            (403, Some("origin_refused")),
            "{path}"
        );
        let from_web = get(&[("Authorization", &read), ("Origin", "https://evil.example")]);
        assert_eq!(from_web.status, 403, "{path}");
        let rebind = get(&[("Authorization", &read), ("Host", &foreign)]);
        assert_eq!(
            (rebind.status, rebind.json()["error"].as_str()),
            (403, Some("host not allowed")),
            "{path}"
        );
        for reply in [&none, &wrong, &from_ext, &from_web, &rebind] {
            assert_no_cors(reply, path);
            assert!(
                !reply.body.contains("Parliamo") && !reply.body.contains("Anna"),
                "{path}: nothing served"
            );
        }

        let ok = get(&[("Authorization", &read)]);
        assert_eq!(ok.status, 200, "{path}: {}", ok.body);
        assert_no_cors(&ok, path);
        assert_eq!(ok.header("Cache-Control"), Some("no-store"), "{path}");
        assert_eq!(
            ok.header("X-Content-Type-Options"),
            Some("nosniff"),
            "{path}"
        );
        for forbidden in [
            "anna@example.com",
            "embedding",
            "0.98765",
            "person_id",
            "archive-index",
        ] {
            assert!(
                !ok.body.contains(forbidden),
                "{forbidden} in {path}: {}",
                ok.body
            );
        }
    }
    // Unknown ids and document names; unknown archive paths.
    for (path, status, code) in [
        ("/archive/items/2026/09/nope".to_string(), 404, "not_found"),
        (
            "/archive/items/2026/09/nope/export".to_string(),
            404,
            "not_found",
        ),
        (
            format!("/archive/items/{id}/documents/nope.md"),
            404,
            "not_found",
        ),
        (
            "/archive/items/..%2F..%2Fetc".to_string(),
            400,
            "invalid_id",
        ),
        (
            format!("/archive/items/{id}/documents/transcript.md"),
            400,
            "invalid_name",
        ),
        ("/archive/voices".to_string(), 404, "not_found"),
    ] {
        let reply = http(r.port, "GET", &path, &[("Authorization", &read)], "");
        assert_eq!(
            (reply.status, reply.json()["code"].as_str()),
            (status, Some(code)),
            "{path}"
        );
        assert_no_cors(&reply, &path);
    }
}

#[test]
fn archive_reads_answer_json_pages_and_files() {
    let r = start_server();
    r.host.0.config.lock().unwrap().archive = true;
    let id = seed_archive(&r);
    for i in 0..3 {
        let meta = archive::ItemMeta {
            title: format!("Note {i}"),
            date: format!("2026-09-0{}T08:00:00+02:00", i + 1),
            ..Default::default()
        };
        archive::create_item(&r.host.0.archive, &meta, &archive::SegmentsFile::default()).unwrap();
    }
    let (read, _) = add_archive_token(&r, "reader", &[Scope::Read]);
    let (full, _) = add_archive_token(&r, "full", &[Scope::Read, Scope::People]);
    let get = |auth: &str, path: &str| http(r.port, "GET", path, &[("Authorization", auth)], "");

    // Pages with an opaque cursor, bounded.
    let first = get(&read, "/archive/items?limit=2");
    assert_eq!(first.status, 200);
    assert_eq!(first.header("Content-Type"), Some("application/json"));
    assert_eq!(first.json()["total"], 4);
    assert_eq!(first.json()["items"].as_array().unwrap().len(), 2);
    assert_eq!(first.json()["items"][0]["id"], id.as_str());
    let cursor = first.json()["next_cursor"].as_str().unwrap().to_string();
    let second = get(&read, &format!("/archive/items?limit=2&cursor={cursor}"));
    assert_eq!(second.json()["items"].as_array().unwrap().len(), 2);
    assert!(second.json()["next_cursor"].is_null());
    for bad in [
        "/archive/items?limit=1000",
        "/archive/items?limit=0",
        "/archive/items?cursor=zzz",
    ] {
        assert_eq!(get(&read, bad).status, 400, "{bad}");
    }

    // An item; emails with the people scope only.
    let item = get(&read, &format!("/archive/items/{id}"));
    assert_eq!(
        item.json()["meta"]["participants"],
        serde_json::json!([{"name": "Anna Rossi"}])
    );
    assert_eq!(
        item.json()["speakers"],
        serde_json::json!([{"id": "voice:1", "label": "Anna Rossi"}])
    );
    let item = get(&full, &format!("/archive/items/{}", id.replace('/', "%2F")));
    assert_eq!(
        item.json()["meta"]["participants"][0]["email"],
        "anna@example.com"
    );
    assert!(!item.body.contains("embedding"));
    let people = get(&read, "/archive/people");
    assert_eq!(
        people.json()["people"],
        serde_json::json!([{"id": "p-anna", "name": "Anna Rossi", "aliases": ["Annina"]}])
    );
    assert_eq!(
        get(&full, "/archive/people").json()["people"][0]["email"],
        "anna@example.com"
    );

    // An export is a file download.
    let vtt = get(&read, &format!("/archive/items/{id}/export?format=vtt"));
    assert_eq!(vtt.status, 200);
    assert!(vtt.body.starts_with("WEBVTT"));
    assert_eq!(vtt.header("Content-Type"), Some("text/vtt; charset=utf-8"));
    assert!(vtt
        .header("Content-Disposition")
        .unwrap()
        .ends_with("weekly-sync.vtt\""));
    assert_eq!(vtt.header("Cache-Control"), Some("no-store"));
    let md = get(&read, &format!("/archive/items/{id}/export?format=md"));
    assert!(md.body.contains("Anna Rossi") && !md.body.contains("anna@example.com"));
    assert!(get(&full, &format!("/archive/items/{id}/export?format=md"))
        .body
        .contains("anna@example.com"));

    // HEAD is a read too: headers, no body.
    let head = http(
        r.port,
        "HEAD",
        "/archive/items",
        &[("Authorization", &read)],
        "",
    );
    assert_eq!(head.status, 200);
    assert!(head.body.is_empty());
}

/// Archive requests hold one of a few slots, so a script can't take every
/// worker from the extension; the rest get 503 + Retry-After.
#[test]
fn archive_requests_share_a_bounded_number_of_workers() {
    let r = start_server();
    r.host.0.config.lock().unwrap().archive = true;
    let (read, _) = add_archive_token(&r, "reader", &[Scope::Read]);
    r.host.0.block_archive.store(true, Ordering::SeqCst);
    let port = r.port;
    let held: Vec<_> = (0..ARCHIVE_SLOTS)
        .map(|_| {
            let read = read.clone();
            std::thread::spawn(move || {
                http(
                    port,
                    "GET",
                    "/archive/items",
                    &[("Authorization", &read)],
                    "",
                )
                .status
            })
        })
        .collect();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while r.host.0.archive_waiting.load(Ordering::SeqCst) < ARCHIVE_SLOTS {
        assert!(
            std::time::Instant::now() < deadline,
            "the slow requests never started"
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let busy = http(
        r.port,
        "GET",
        "/archive/people",
        &[("Authorization", &read)],
        "",
    );
    assert_eq!(
        (busy.status, busy.json()["code"].as_str()),
        (503, Some("busy"))
    );
    assert_eq!(busy.header("Retry-After"), Some("1"));
    assert_no_cors(&busy, "503");
    // The extension still gets an answer meanwhile.
    let auth = bearer();
    assert_eq!(
        http(
            r.port,
            "GET",
            "/app/version",
            &[("Authorization", &auth)],
            ""
        )
        .status,
        200
    );
    r.host.0.block_archive.store(false, Ordering::SeqCst);
    for h in held {
        assert_eq!(h.join().unwrap(), 200);
    }
    assert_eq!(
        http(
            r.port,
            "GET",
            "/archive/people",
            &[("Authorization", &read)],
            ""
        )
        .status,
        200
    );
}

// ---- archive write route (#251) ---------------------------------------------

fn post_note(r: &Running, headers: &[(&str, &str)], body: &str) -> Reply {
    http(r.port, "POST", "/archive/items", headers, body)
}

/// `POST /archive/items`: refused without the switch, a token, the `write`
/// scope, or with any browser Origin or a foreign Host — before the body
/// is looked at; with them, a note in the archive, readable and indexed.
#[test]
fn creating_a_note_is_guarded_like_every_archive_route() {
    let r = start_server();
    let (write, _) = add_archive_token(&r, "Shortcuts", &[Scope::Write]);
    let (read, _) = add_archive_token(&r, "reader", &[Scope::Read, Scope::People]);
    let body = r#"{"title": "Da comprare", "text": "Latte e caffè.", "tags": ["spesa"]}"#;
    let json = ("Content-Type", "application/json");

    let off = post_note(&r, &[("Authorization", &write), json], body);
    assert_eq!(
        (off.status, off.json()["code"].as_str()),
        (403, Some("archive_api_off"))
    );
    r.host.0.config.lock().unwrap().archive = true;
    let foreign = format!("rebind.attacker:{}", r.port);
    let ext_token = bearer();
    for (headers, status, code) in [
        (vec![json], 401, "unauthorized"),
        (
            vec![("Authorization", "Bearer sua_nope"), json],
            401,
            "unauthorized",
        ),
        (
            vec![("Authorization", ext_token.as_str()), json],
            401,
            "unauthorized",
        ),
        (
            vec![("Authorization", read.as_str()), json],
            403,
            "insufficient_scope",
        ),
        (
            vec![("Authorization", write.as_str()), ("Origin", EXT), json],
            403,
            "origin_refused",
        ),
    ] {
        let reply = post_note(&r, &headers, body);
        assert_eq!(
            (reply.status, reply.json()["code"].as_str()),
            (status, Some(code)),
            "{headers:?}"
        );
        assert_no_cors(&reply, code);
    }
    for (k, v) in [
        ("Origin", "https://evil.example"),
        ("Host", foreign.as_str()),
    ] {
        let reply = post_note(&r, &[("Authorization", &write), (k, v), json], body);
        assert_eq!(reply.status, 403, "{k}");
        assert_no_cors(&reply, k);
    }
    assert!(
        archive::list_items(&r.host.0.archive).is_empty(),
        "nothing created by a refused request"
    );

    let created = post_note(&r, &[("Authorization", &write), json], body);
    assert_eq!(created.status, 201, "{}", created.body);
    assert_no_cors(&created, "201");
    assert_eq!(created.header("Cache-Control"), Some("no-store"));
    let id = created.json()["id"].as_str().unwrap().to_string();
    assert_eq!(
        created.header("Location"),
        Some(format!("/archive/items/{id}").as_str())
    );
    assert_eq!(created.json()["source"], "api:Shortcuts");
    assert_eq!(created.json()["type"], "note");
    assert!(
        !created.body.contains(r.host.0.archive.to_str().unwrap()),
        "no paths"
    );
    assert_eq!(*r.host.0.notes_created.lock().unwrap(), vec![id.clone()]);
    // Readable (and found) through the read routes.
    let location = created.header("Location").unwrap().to_string();
    let item = http(r.port, "GET", &location, &[("Authorization", &read)], "");
    assert_eq!(item.status, 200);
    assert_eq!(item.json()["meta"]["title"], "Da comprare");
    assert_eq!(item.json()["meta"]["tags"], serde_json::json!(["spesa"]));
    assert_eq!(item.json()["text"], "\n# Da comprare\n\nLatte e caffè.\n");
    let found = http(
        r.port,
        "GET",
        "/archive/items?q=caff%C3%A8&type=note",
        &[("Authorization", &read)],
        "",
    );
    assert_eq!(found.json()["items"][0]["id"], id.as_str());
    // Other writes don't exist.
    for (m, p) in [
        ("POST", format!("/archive/items/{id}")),
        ("PUT", "/archive/items".to_string()),
        ("DELETE", location.clone()),
    ] {
        let reply = http(r.port, m, &p, &[("Authorization", &write), json], body);
        assert_eq!(
            (reply.status, reply.json()["code"].as_str()),
            (404, Some("not_found")),
            "{m} {p}"
        );
    }
    assert_eq!(
        post_note(&r, &[("Authorization", &write), json], "{").json()["code"],
        "invalid_json"
    );
    let with_query = http(
        r.port,
        "POST",
        "/archive/items?title=x",
        &[("Authorization", &write), json],
        body,
    );
    assert_eq!(with_query.status, 400);
    assert_eq!(archive::list_items(&r.host.0.archive).len(), 1);
}

#[test]
fn a_note_body_over_1_mib_is_refused_without_reading_it() {
    let r = start_server();
    r.host.0.config.lock().unwrap().archive = true;
    let (write, _) = add_archive_token(&r, "clipper", &[Scope::Write]);
    // Declared lengths over the cap (just over, and absurd): answered
    // without the body.
    for len in [archive_write::MAX_BODY_BYTES + 1, 9_223_372_036_854_775_807] {
        let (_s, reply) = raw(
            r.port,
            &format!(
                "POST /archive/items HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nAuthorization: {write}\r\nContent-Length: {len}\r\n\r\n",
                r.port
            ),
        );
        assert_eq!(
            (reply.status, reply.json()["code"].as_str()),
            (413, Some("too_large")),
            "{len}"
        );
        assert!(healthy(r.port));
    }
    let fits = format!(
        r#"{{"text": "{}"}}"#,
        "a".repeat(archive_write::MAX_BODY_BYTES - 64)
    );
    assert_eq!(
        post_note(&r, &[("Authorization", &write)], &fits).status,
        201
    );
}

#[test]
fn note_cleanup_follows_the_settings_and_never_uses_an_external_profile() {
    let r = start_server();
    r.host.0.config.lock().unwrap().archive = true;
    let (write, _) = add_archive_token(&r, "clipper", &[Scope::Write]);
    let auth = [("Authorization", write.as_str())];
    let body = r#"{"text": "ciao mondo", "cleanup": true}"#;
    // The default local profile: cleaned.
    let local = post_note(&r, &auth, body);
    assert_eq!(
        (local.status, local.json()["cleaned"].as_bool()),
        (201, Some(true))
    );
    assert_eq!(local.json()["title"], "CIAO MONDO");
    // An external profile the user opted in to for dictation: refused.
    {
        let mut s = r.host.0.settings.lock().unwrap();
        let mut p = crate::llm::LlmProfile::new(
            "work",
            "Work",
            crate::settings::CleanupApi::Openai,
            "https://llm.example.com/v1",
            "",
            "m",
        );
        p.cleanup_opt_in = p.host();
        assert!(p.external && p.cleanup_allowed());
        s.llm_profiles.push(p);
        s.cleanup_profile = "work".into();
    }
    let calls = r.host.0.calls.load(Ordering::SeqCst);
    let external = post_note(&r, &auth, body);
    assert_eq!(
        (external.status, external.json()["code"].as_str()),
        (409, Some("cleanup_external"))
    );
    assert_eq!(
        r.host.0.calls.load(Ordering::SeqCst),
        calls,
        "nothing cleaned"
    );
    assert_eq!(
        post_note(&r, &auth, r#"{"text": "ciao mondo"}"#).status,
        201,
        "without cleanup it works"
    );
    // Cleanup off in Settings: refused too.
    {
        let mut s = r.host.0.settings.lock().unwrap();
        s.cleanup_profile = crate::llm::LOCAL_PROFILE_ID.into();
        s.cleanup_level = crate::settings::CleanupLevel::None;
    }
    let off = post_note(&r, &auth, body);
    assert_eq!(
        (off.status, off.json()["code"].as_str()),
        (409, Some("cleanup_off"))
    );
    assert_eq!(archive::list_items(&r.host.0.archive).len(), 2);
}

#[test]
fn notes_are_idempotent_by_key_and_rate_limited() {
    let r = start_server();
    r.host.0.config.lock().unwrap().archive = true;
    let (write, _) = add_archive_token(&r, "clipper", &[Scope::Write]);
    let body = r#"{"text": "clipboard"}"#;
    let key = "6f1c2b1e-0d7a-4b8e-9c55-1f0a2b3c4d5e";
    let with_key = |body: &str| {
        post_note(
            &r,
            &[("Authorization", &write), ("Idempotency-Key", key)],
            body,
        )
    };
    let first = with_key(body);
    assert_eq!(first.status, 201);
    let again = with_key(body);
    assert_eq!(again.status, 201);
    assert_eq!(again.json()["id"], first.json()["id"]);
    assert_eq!(again.json()["replayed"], true);
    assert_eq!(again.header("Idempotent-Replayed"), Some("true"));
    assert_eq!(archive::list_items(&r.host.0.archive).len(), 1);
    let changed = with_key(r#"{"text": "other"}"#);
    assert_eq!(
        (changed.status, changed.json()["code"].as_str()),
        (422, Some("idempotency_mismatch"))
    );

    // Creations have their own, tighter limit (the first note used one).
    let mut created = 1;
    let limited = loop {
        let reply = post_note(&r, &[("Authorization", &write)], body);
        if reply.status != 201 {
            break reply;
        }
        created += 1;
        assert!(created < 100, "never limited");
    };
    assert_eq!(created, archive_write::CREATE_BURST as usize);
    assert_eq!(
        (limited.status, limited.json()["code"].as_str()),
        (429, Some("rate_limited"))
    );
    assert!(
        limited
            .header("Retry-After")
            .unwrap()
            .parse::<u64>()
            .unwrap()
            >= 1
    );
    assert_no_cors(&limited, "429");
    // A replay creates nothing, so it still answers.
    assert_eq!(with_key(body).status, 201);
    assert_eq!(archive::list_items(&r.host.0.archive).len(), created);
}
