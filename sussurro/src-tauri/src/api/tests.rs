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
}

#[test]
fn only_the_extension_routes_are_meeting_routes() {
    for r in [
        Route::AppVersion,
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
        websocket_accept(Some("websocket"), Some("13"), Some("dGhlIHNhbXBsZSBub25jZQ==")).unwrap(),
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

impl Host for TestHost {
    fn config(&self) -> ApiConfig {
        self.0.config.lock().unwrap().clone()
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
            while !self.0.release_slow.load(Ordering::SeqCst) && std::time::Instant::now() < deadline {
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
        Ok(self.0.archive.clone())
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
            let req = crate::engine::session::meeting_request(id, cancel, &m.start, m.source, m.names);
            // No speaker model in tests: voices are off, names and "You" work.
            let no_model = |_: PathBuf| -> crate::speakers::tracker::EmbedderLoader {
                Box::new(|| Err(anyhow::anyhow!("no speaker model in tests")))
            };
            let _ = crate::engine::session::run_request_with_speakers(
                &inner.settings,
                &inner.paths,
                req,
                fake_transcribe,
                |_: &Settings, _: Option<&str>, raw: &str| format!("{raw}."),
                |_: &std::path::Path| {
                    Box::new(EnergyDetector::default()) as Box<dyn crate::engine::segmenter::SpeechDetector>
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
    }));
    let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
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
    s.set_read_timeout(Some(std::time::Duration::from_secs(20))).unwrap();
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
    Reply { status, headers, body }
}

fn bearer() -> String {
    format!("Bearer {TOKEN}")
}

#[test]
fn token_less_routes_are_unchanged() {
    let r = start_server();
    let clean = http(r.port, "POST", "/clean", &[], "  ciao  ");
    assert_eq!((clean.status, clean.json()["cleaned"].as_str()), (200, Some("ciao")));
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
    let script = http(r.port, "GET", "/app/version", &[("Authorization", &auth)], "");
    assert_eq!(script.status, 200);
    assert!(script.header("Access-Control-Allow-Origin").is_none());
    // No token, a wrong one, a web page.
    assert_eq!(http(r.port, "GET", "/app/version", &[("Origin", EXT)], "").status, 401);
    assert_eq!(
        http(r.port, "GET", "/app/version", &[("Authorization", "Bearer 00")], "").status,
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
    let pre = http(r.port, "OPTIONS", "/items/a/b/export", &[("Origin", EXT)], "");
    assert_eq!(pre.status, 204);
    assert!(pre
        .header("Access-Control-Allow-Headers")
        .unwrap()
        .contains("Authorization"));
    assert_eq!(
        http(r.port, "OPTIONS", "/app/version", &[("Origin", "https://x.example")], "").status,
        403
    );
    // Items that don't exist, bad formats.
    assert_eq!(
        http(r.port, "POST", "/items/2026/09/nope/open", &[("Authorization", &auth)], "").status,
        404
    );
    assert_eq!(
        http(r.port, "POST", "/items/../../etc/open", &[("Authorization", &auth)], "").status,
        404
    );
    assert_eq!(
        http(r.port, "GET", "/items/2026/09/nope/export?format=docx", &[("Authorization", &auth)], "")
            .status,
        400
    );
    // WebSocket: an extension origin and the query token are required.
    assert_eq!(ws_connect(r.port, TOKEN, None).err(), Some(403));
    assert_eq!(ws_connect(r.port, TOKEN, Some("https://meet.google.com")).err(), Some(403));
    assert_eq!(ws_connect(r.port, "wrong", Some(EXT)).err(), Some(401));
    // Unpaired (no token configured): nothing is accepted.
    r.host.0.config.lock().unwrap().extension_token.clear();
    assert_eq!(ws_connect(r.port, "", Some(EXT)).err(), Some(401));
    assert_eq!(
        http(r.port, "GET", "/app/version", &[("Authorization", "Bearer ")], "").status,
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
        http(r.port, "GET", "/app/version", &[("Authorization", &old), ("Origin", EXT)], "").status,
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
        http(r.port, "GET", "/app/version", &[("Authorization", &old), ("Origin", EXT)], "").status,
        401
    );
    assert_eq!(ws_connect(r.port, TOKEN, Some(EXT)).err(), Some(401));
    let new = format!("Bearer {fresh}");
    let ok = http(r.port, "GET", "/app/version", &[("Authorization", &new), ("Origin", EXT)], "");
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
    assert_eq!(serde_json::to_value(ListenState::Off).unwrap(), serde_json::json!({"state": "off"}));
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
    assert_eq!((ready["type"].as_str(), ready["state"].as_str()), (Some("status"), Some("ready")));
    assert_eq!(ready["protocol"], protocol::PROTOCOL_VERSION);

    ws.send(Message::text(
        r#"{"type":"start","title":"Weekly sync","url":"https://meet.google.com/abc-defg-hij","platform":"meet","rate":48000,"channels":2}"#,
    ))
    .unwrap();
    ws.send(Message::text(r#"{"type":"participants","names":["Anna Rossi","Bo"]}"#))
        .unwrap();
    // 9 s: the user (mic) speaks 0.5–2.5 s softly, the remote side
    // 3.5–6 s loudly; 20 ms frames, both channels interleaved.
    let frames = 450;
    for n in 0..frames {
        let t = n as f32 * 0.02;
        let mic_amp = if (0.5..2.5).contains(&t) { 0.1 } else { 0.0 };
        let remote_amp = if (3.5..6.0).contains(&t) { 0.4 } else { 0.0 };
        let seq = n as u32;
        ws.send(Message::binary(protocol::encode_audio_frame(0, seq, &frame_48k(n * 960, mic_amp))))
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
            ws.send(Message::text(r#"{"type":"speaker_active","name":"Anna Rossi","t":4000}"#))
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
        warnings.iter().any(|w| w.contains("1 remote audio frame(s) lost")),
        "{warnings:?}"
    );
    let item_id = done_item.expect("done with an item id");

    // Both channels, each with its own text, in time order.
    let channels: Vec<&str> = segments.iter().filter_map(|s| s["channel"].as_str()).collect();
    assert!(channels.contains(&"mic") && channels.contains(&"remote"), "{segments:?}");
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
    assert!((3_000..4_000).contains(&remote.start_ms), "{}", remote.start_ms);
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
    let names: Vec<&str> = item.meta.participants.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, ["Anna Rossi", "Bo"]);

    // The page's events are stored, on the session clock.
    let events = archive::meeting::read_events(&r.host.0.archive, &item_id).unwrap();
    assert_eq!(events.len(), 3, "{events:?}");
    assert!(matches!(&events[0], archive::meeting::MeetingEvent::Participants { names, .. } if names.len() == 2));
    let archive::meeting::MeetingEvent::SpeakerActive {
        at_ms, t_ms, name, id, ..
    } = &events[1]
    else {
        panic!("{events:?}")
    };
    assert_eq!((name.as_deref(), *t_ms, id.as_deref()), (Some("Anna Rossi"), 4_000, None));
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
    assert!(txt.header("Content-Type").unwrap().starts_with("text/plain"));
    assert_eq!(txt.header("Access-Control-Allow-Origin"), Some(EXT));
    assert!(txt.body.contains("] You: hello from mic."), "{}", txt.body);
    assert!(txt.body.contains("] Anna Rossi: hello from remote."), "{}", txt.body);
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
    assert!(srt.header("Content-Type").unwrap().starts_with("application/x-subrip"));
    assert!(srt.header("Content-Disposition").unwrap().contains(".srt"));
    assert!(srt.body.contains(" --> ") && srt.body.contains("hello from remote."), "{}", srt.body);
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
    let anon = http(r.port, "GET", &format!("/items/{item_id}/export?format=srt"), &[], "");
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
        assert!(std::time::Instant::now() < deadline, "the meeting was not written");
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
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
            ("GET", "/items/2026/09/x/export"),
            ("POST", "/items/2026/09/x/open"),
            ("OPTIONS", "/app/version"),
            ("GET", "/nope"),
        ] {
            let reply = http(r.port, method, path, &[("Host", host), ("Authorization", &auth)], "text");
            assert_eq!(reply.status, 403, "{host} {method} {path}");
            assert_eq!(reply.json()["error"], "host not allowed");
            assert!(reply.header("Access-Control-Allow-Origin").is_none());
        }
    }
    assert_eq!(r.host.0.calls.load(Ordering::SeqCst), 0, "no route ran");
    // Both loopback names on the right port work.
    for host in [format!("127.0.0.1:{}", r.port), format!("localhost:{}", r.port)] {
        assert_eq!(http(r.port, "GET", "/history", &[("Host", &host)], "").status, 200);
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
        for (method, path) in [("POST", "/clean"), ("POST", "/transcribe?ext=wav"), ("GET", "/history")] {
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
    assert_eq!(r.host.0.calls.load(Ordering::SeqCst), 0, "nothing was cleaned or transcribed");
    // No origin (curl) or an extension's: allowed.
    assert_eq!(http(r.port, "POST", "/clean", &[], "ciao").status, 200);
    assert_eq!(http(r.port, "POST", "/clean", &[("Origin", EXT)], "ciao").status, 200);
}

/// `Settings.api_scripting` off (a new install): the token-less routes are
/// refused with an explanation; the extension's routes still work. Read
/// per request, so switching applies at once.
#[test]
fn the_scripting_routes_answer_only_when_switched_on() {
    let r = start_server();
    r.host.0.config.lock().unwrap().scripting = false;
    for (method, path) in [("POST", "/clean"), ("POST", "/transcribe?ext=wav"), ("GET", "/history")] {
        let reply = http(r.port, method, path, &[], "RIFF");
        assert_eq!(reply.status, 403, "{method} {path}");
        assert!(reply.json()["error"].as_str().unwrap().contains("Scripting routes"));
    }
    assert_eq!(r.host.0.calls.load(Ordering::SeqCst), 0);
    let auth = bearer();
    assert_eq!(http(r.port, "GET", "/app/version", &[("Authorization", &auth)], "").status, 200);
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
    assert_eq!(r.host.0.calls.load(Ordering::SeqCst), 0, "no host call for a refused body");

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
        .map(|_| std::thread::spawn(move || http(port, "POST", "/transcribe?ext=slow", &[], "RIFF").status))
        .collect();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while r.host.0.slow_running.load(Ordering::SeqCst) < SLOW_SLOTS {
        assert!(std::time::Instant::now() < deadline, "the slow requests never started");
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let auth = bearer();
    let t0 = std::time::Instant::now();
    let v = http(port, "GET", "/app/version", &[("Authorization", &auth), ("Origin", EXT)], "");
    assert_eq!(v.status, 200);
    assert_eq!(http(port, "GET", "/history", &[], "").status, 200);
    assert!(t0.elapsed() < std::time::Duration::from_secs(2), "{:?}", t0.elapsed());
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
    let export = http(r.port, "GET", &format!("/items/{note}/export?format=md"), &h, "");
    assert_eq!(export.status, 403);
    assert_eq!(export.json()["code"], export::NOT_EXTENSION_ITEM);
    assert!(!export.body.contains("Private note"));
    assert_eq!(export.header("Access-Control-Allow-Origin"), Some(EXT), "readable by the extension");
    let open = http(r.port, "POST", &format!("/items/{note}/open"), &h, "");
    assert_eq!(open.status, 403);
    assert_eq!(open.json()["code"], export::NOT_EXTENSION_ITEM);
    assert!(r.host.0.opened.lock().unwrap().is_empty());
    // The extension's own item still works.
    assert_eq!(
        http(r.port, "GET", &format!("/items/{meeting}/export?format=md"), &h, "").status,
        200
    );
    assert_eq!(http(r.port, "POST", &format!("/items/{meeting}/open"), &h, "").status, 200);
}
