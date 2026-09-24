//! Local API tests: the route table and gating (pure), then the real
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
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
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
fn meeting_routes_exist_only_with_meetings_enabled() {
    for r in [
        Route::AppVersion,
        Route::Live,
        Route::OpenItem("x".into()),
        Route::ExportItem("x".into()),
        Route::Preflight,
    ] {
        assert!(r.is_meeting());
        assert_eq!(gate(r, false), Route::NotFound);
    }
    assert_eq!(gate(Route::Live, true), Route::Live);
    // The token-less routes don't depend on the flag.
    for r in [Route::Clean, Route::Transcribe, Route::History] {
        assert!(!r.is_meeting());
    }
    assert_eq!(gate(Route::Clean, false), Route::Clean);
    assert_eq!(gate(Route::History, true), Route::History);
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
        serde_json::json!({"cleaned": text.trim()})
    }
    fn transcribe(&self, bytes: Vec<u8>, ext: &str) -> (u16, serde_json::Value) {
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
            let req = crate::engine::session::meeting_request(id, cancel, &m.start, m.source);
            let _ = crate::engine::session::run_request_with(
                &inner.settings,
                &inner.paths,
                req,
                fake_transcribe,
                |_: &Settings, _: Option<&str>, raw: &str| format!("{raw}."),
                |_: &std::path::Path| {
                    Box::new(EnergyDetector::default()) as Box<dyn crate::engine::segmenter::SpeechDetector>
                },
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

fn start_server(meetings_enabled: bool) -> Running {
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
            meetings_enabled,
            extension_token: TOKEN.into(),
        }),
        settings: Mutex::new(settings),
        paths,
        archive,
        next_id: AtomicU64::new(1),
        meeting_running: AtomicBool::new(false),
        opened: Mutex::new(Vec::new()),
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

/// A raw HTTP/1.1 request (exact control over the headers).
fn http(port: u16, method: &str, path: &str, headers: &[(&str, &str)], body: &str) -> Reply {
    let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
    let mut req = format!("{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n");
    for (k, v) in headers {
        req.push_str(&format!("{k}: {v}\r\n"));
    }
    req.push_str(&format!("Content-Length: {}\r\n\r\n{body}", body.len()));
    s.write_all(req.as_bytes()).unwrap();
    let mut raw = String::new();
    s.read_to_string(&mut raw).unwrap();
    let (head, body) = raw.split_once("\r\n\r\n").unwrap_or((&raw, ""));
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
        body: body.to_string(),
    }
}

fn bearer() -> String {
    format!("Bearer {TOKEN}")
}

#[test]
fn token_less_routes_are_unchanged_and_meeting_routes_are_off_by_default() {
    let r = start_server(false);
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
    // Meetings off: the new routes don't exist, even with the token.
    let auth = bearer();
    let v = http(r.port, "GET", "/app/version", &[("Authorization", &auth)], "");
    assert_eq!(v.status, 404);
    assert_eq!(
        http(r.port, "OPTIONS", "/app/version", &[("Origin", EXT)], "").status,
        404
    );
    let ws = ws_connect(r.port, TOKEN, Some(EXT));
    assert_eq!(ws.err(), Some(404));
}

#[test]
fn meeting_routes_check_token_origin_and_send_cors() {
    let r = start_server(true);
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
    assert_eq!(ok.json()["app"], env!("CARGO_PKG_VERSION"));
    assert_eq!(ok.header("Access-Control-Allow-Origin"), Some(EXT));
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
    let r = start_server(true);
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
    let r = start_server(true);
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
        if n == 200 {
            ws.send(Message::text(r#"{"type":"speaker_active","name":"Anna Rossi","t":4000}"#))
                .unwrap();
        }
    }
    ws.send(Message::text(r#"{"type":"stop"}"#)).unwrap();

    let mut segments = Vec::new();
    let mut states = Vec::new();
    let mut warnings = Vec::new();
    let mut done_item = None;
    while let Some(m) = read_json(&mut ws) {
        match m["type"].as_str() {
            Some("segment") => segments.push(m["segment"].clone()),
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
    assert!(segs.iter().all(|s| s.speaker_id.is_none()), "attribution is #131");

    // The page's events are stored for #131, on the session clock.
    let events = archive::meeting::read_events(&r.host.0.archive, &item_id).unwrap();
    assert_eq!(events.len(), 2, "{events:?}");
    assert!(matches!(&events[0], archive::meeting::MeetingEvent::Participants { names, .. } if names.len() == 2));
    let archive::meeting::MeetingEvent::SpeakerActive { at_ms, t_ms, name } = &events[1] else {
        panic!("{events:?}")
    };
    assert_eq!((name.as_str(), *t_ms), ("Anna Rossi", 4_000));
    assert!((3_900..=4_100).contains(at_ms), "{at_ms}");

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
    assert!(txt.body.contains("] hello from mic."), "{}", txt.body);
    assert!(txt.body.contains("] hello from remote."), "{}", txt.body);
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
    let r = start_server(true);
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
