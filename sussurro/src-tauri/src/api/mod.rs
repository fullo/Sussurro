//! The local HTTP API (loopback only).
//!
//! Token-less routes for local scripts (unchanged since 0.5): `POST /clean`,
//! `POST /transcribe`, `GET /history`.
//!
//! 0.9 routes for the browser extension (#126, plan §6), answered only with
//! the extension token (E6, [`auth`]); always on since #138 removed the
//! `meetings_enabled` preview flag (E12):
//! - `GET /app/version` → `{app, protocol, subtitles}` handshake; `subtitles`
//!   is the subtitles setting (`on_request` | `always`, #133), so the side
//!   panel offers "Create .srt" only when the app doesn't write it itself
//!   (#129)
//! - `WS /live?token=` → a meeting session ([`live`], [`protocol`])
//! - `POST /items/{id}/open` → the app comes to the front on that item
//! - `GET /items/{id}/export?format=md|txt|srt|vtt` → [`export`]
//!
//! Item ids contain `/` (`2026/09/2026-09-24-weekly-sync`): they are taken
//! as-is between `/items/` and the action, `%2F` also accepted.

pub mod auth;
pub mod export;
pub mod live;
pub mod protocol;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tauri::{AppHandle, Emitter, Manager};

use crate::state::AppState;

/// What the API needs from the app. [`AppHost`] in the app; tests run the
/// real server on a fake one.
pub trait Host: Send + Sync + 'static {
    fn config(&self) -> ApiConfig;
    /// `POST /clean`: the JSON answer.
    fn clean(&self, text: &str) -> serde_json::Value;
    /// `POST /transcribe`: status and JSON answer.
    fn transcribe(&self, bytes: Vec<u8>, ext: &str) -> (u16, serde_json::Value);
    /// `GET /history`: the JSON answer.
    fn history(&self, query: &str, n: usize) -> serde_json::Value;
    fn archive_dir(&self) -> anyhow::Result<PathBuf>;
    /// Bring the app to the front on item `id`; an error if there is none.
    fn open_item(&self, id: &str) -> anyhow::Result<()>;
    /// Run the long-form engine on a browser meeting; returns the session id.
    fn start_meeting(&self, meeting: live::MeetingStart) -> anyhow::Result<u64>;
}

/// The settings the API checks on every request (so regenerating the
/// token applies at once).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ApiConfig {
    pub extension_token: String,
    /// Told to the extension by `GET /app/version` (#129).
    pub subtitles: crate::settings::SubtitlesMode,
}

/// Routes exposed by the local API. Pure mapping — unit tested.
#[derive(Debug, PartialEq, Eq)]
pub enum Route {
    Clean,
    Transcribe,
    History,
    AppVersion,
    Live,
    OpenItem(String),
    ExportItem(String),
    /// CORS preflight for a meeting route.
    Preflight,
    NotFound,
}

impl Route {
    /// A 0.9 route: behind the extension token (and extension-only origins).
    pub fn is_meeting(&self) -> bool {
        matches!(
            self,
            Route::AppVersion
                | Route::Live
                | Route::OpenItem(_)
                | Route::ExportItem(_)
                | Route::Preflight
        )
    }
}

/// Pure: `%XX` decoding; `None` on a malformed escape or invalid UTF-8.
pub fn percent_decode(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = s.get(i + 1..i + 3)?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// Pure: the item id of `/items/<id>/<action>`.
fn item_path(path: &str, action: &str) -> Option<String> {
    let rest = path.strip_prefix("/items/")?;
    let id = rest.strip_suffix(action)?.strip_suffix('/')?;
    let id = percent_decode(id)?;
    (!id.is_empty()).then_some(id)
}

fn is_meeting_path(path: &str) -> bool {
    path == "/app/version"
        || path == "/live"
        || item_path(path, "open").is_some()
        || item_path(path, "export").is_some()
}

/// Pure: method + path → route.
pub fn route(method: &str, path: &str) -> Route {
    match (method, path) {
        ("POST", "/clean") => Route::Clean,
        ("POST", "/transcribe") => Route::Transcribe,
        ("GET", "/history") => Route::History,
        ("GET", "/app/version") => Route::AppVersion,
        ("GET", "/live") => Route::Live,
        ("OPTIONS", p) if is_meeting_path(p) => Route::Preflight,
        ("POST", p) => item_path(p, "open").map_or(Route::NotFound, Route::OpenItem),
        ("GET", p) => item_path(p, "export").map_or(Route::NotFound, Route::ExportItem),
        _ => Route::NotFound,
    }
}

/// Pure: split "/history?n=5&q=ciao" into (path, params).
pub fn parse_url(url: &str) -> (&str, HashMap<String, String>) {
    let mut parts = url.splitn(2, '?');
    let path = parts.next().unwrap_or("/");
    let mut params = HashMap::new();
    if let Some(query) = parts.next() {
        for pair in query.split('&') {
            let mut kv = pair.splitn(2, '=');
            if let Some(k) = kv.next() {
                if !k.is_empty() {
                    params.insert(k.to_string(), kv.next().unwrap_or("").to_string());
                }
            }
        }
    }
    (path, params)
}

/// Start the local HTTP API (loopback only). Best-effort: a bind failure is
/// logged, never fatal. Applied at startup — toggling the setting needs an
/// app restart.
///
/// Security design — keep these properties when changing this module:
/// - Loopback-only bind (`127.0.0.1`, never `0.0.0.0`): the API is meant for
///   local scripts and must not be reachable from other machines on the LAN.
/// - Endpoints accept request bodies / query params only — no filesystem paths
///   or other caller-controlled values are ever passed to the OS (item ids
///   are validated and confined to the archive).
/// - Request payloads, URLs (they may carry the extension token) and tokens
///   are never logged: transcripts may contain sensitive text.
/// - Meeting routes: extension token, extension-only origins and CORS
///   ([`auth`]); an unpaired app (no token) accepts nothing there.
pub fn spawn(app: AppHandle, port: u16) {
    std::thread::spawn(move || {
        let Some(server) = bind(port) else { return };
        serve(server, Arc::new(AppHost { app }));
    });
}

/// Whether this run of the app serves the local API (#127). Its settings
/// apply at startup: Settings → Browser extension compares them with this
/// to say when a restart is needed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum ListenState {
    /// Not started (the local API is off, or still starting).
    Off,
    Listening { port: u16 },
    /// The port was taken (or refused): the API is not running.
    Failed { port: u16 },
}

static LISTEN_STATE: std::sync::Mutex<ListenState> = std::sync::Mutex::new(ListenState::Off);

pub fn listen_state() -> ListenState {
    *LISTEN_STATE.lock().unwrap_or_else(|e| e.into_inner())
}

/// Bind `127.0.0.1:port` and record the outcome in [`listen_state`].
fn bind(port: u16) -> Option<tiny_http::Server> {
    let (server, state) = match tiny_http::Server::http(("127.0.0.1", port)) {
        Ok(s) => {
            let actual = s.server_addr().to_ip().map_or(port, |a| a.port());
            eprintln!("local API listening on http://127.0.0.1:{actual}");
            (Some(s), ListenState::Listening { port: actual })
        }
        Err(e) => {
            eprintln!("local API: cannot bind 127.0.0.1:{port}: {e}");
            (None, ListenState::Failed { port })
        }
    };
    *LISTEN_STATE.lock().unwrap_or_else(|e| e.into_inner()) = state;
    server
}

/// Answer requests until the server is dropped or unblocked. WebSocket
/// sessions get a thread each; the other routes are answered in turn.
pub fn serve(server: tiny_http::Server, host: Arc<dyn Host>) {
    for request in server.incoming_requests() {
        handle(&host, request);
    }
}

fn header(request: &tiny_http::Request, name: &'static str) -> Option<String> {
    request
        .headers()
        .iter()
        .find(|h| h.field.equiv(name))
        .map(|h| h.value.as_str().to_string())
}

fn with_headers<R: std::io::Read>(
    mut response: tiny_http::Response<R>,
    headers: &[(&str, String)],
) -> tiny_http::Response<R> {
    for (k, v) in headers {
        if let Ok(h) = tiny_http::Header::from_bytes(k.as_bytes(), v.as_bytes()) {
            response.add_header(h);
        }
    }
    response
}

fn respond_json(request: tiny_http::Request, status: u16, body: serde_json::Value) {
    respond_json_with(request, status, body, &[]);
}

fn respond_json_with(
    request: tiny_http::Request,
    status: u16,
    body: serde_json::Value,
    headers: &[(&str, String)],
) {
    let response = tiny_http::Response::from_string(body.to_string())
        .with_status_code(status)
        .with_header(
            tiny_http::Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..])
                .expect("header"),
        );
    let _ = request.respond(with_headers(response, headers));
}

fn handle(host: &Arc<dyn Host>, mut request: tiny_http::Request) {
    let url = request.url().to_string();
    let method = request.method().as_str().to_string();
    let (path, params) = parse_url(&url);
    let config = host.config();

    let route = route(&method, path);
    if route.is_meeting() {
        return handle_meeting(host, request, route, &params, &config);
    }
    match route {
        Route::Clean => {
            let mut text = String::new();
            if request.as_reader().read_to_string(&mut text).is_err() || text.trim().is_empty() {
                return respond_json(request, 400, serde_json::json!({"error": "empty body"}));
            }
            respond_json(request, 200, host.clean(&text));
        }
        Route::Transcribe => {
            let mut bytes = Vec::new();
            if request.as_reader().read_to_end(&mut bytes).is_err() || bytes.is_empty() {
                return respond_json(request, 400, serde_json::json!({"error": "empty body"}));
            }
            let ext = params.get("ext").cloned().unwrap_or_default();
            let (status, body) = host.transcribe(bytes, &ext);
            respond_json(request, status, body);
        }
        Route::History => {
            let n = params
                .get("n")
                .and_then(|v| v.parse::<usize>().ok())
                .unwrap_or(20);
            let query = params.get("q").cloned().unwrap_or_default();
            respond_json(request, 200, host.history(&query, n));
        }
        _ => {
            respond_json(
                request,
                404,
                serde_json::json!({
                    "error": "unknown endpoint",
                    "endpoints": ["POST /clean (text body)", "POST /transcribe?ext=wav (audio body)", "GET /history?n=20&q="]
                }),
            );
        }
    }
}

fn handle_meeting(
    host: &Arc<dyn Host>,
    request: tiny_http::Request,
    route: Route,
    params: &HashMap<String, String>,
    config: &ApiConfig,
) {
    let origin = header(&request, "Origin");
    let cors = auth::cors_headers(origin.as_deref());
    let deny = |request: tiny_http::Request, d: auth::Denied| {
        respond_json_with(
            request,
            d.status(),
            serde_json::json!({"error": d.message()}),
            &cors,
        )
    };

    if route == Route::Preflight {
        return match auth::preflight_headers(origin.as_deref()) {
            Some(h) => {
                let _ = request.respond(with_headers(tiny_http::Response::empty(204), &h));
            }
            None => deny(request, auth::Denied::Forbidden),
        };
    }
    if route == Route::Live {
        let token = params.get("token").and_then(|t| percent_decode(t));
        if let Err(d) = auth::check_ws(&config.extension_token, token.as_deref(), origin.as_deref()) {
            return deny(request, d);
        }
        return upgrade_live(host, request);
    }
    let authorization = header(&request, "Authorization");
    if let Err(d) = auth::check_http(
        &config.extension_token,
        authorization.as_deref(),
        origin.as_deref(),
    ) {
        return deny(request, d);
    }
    match route {
        Route::AppVersion => respond_json_with(
            request,
            200,
            serde_json::json!({
                "app": env!("CARGO_PKG_VERSION"),
                "protocol": protocol::PROTOCOL_VERSION,
                "protocol_min": protocol::MIN_PROTOCOL,
                "subtitles": config.subtitles,
            }),
            &cors,
        ),
        Route::OpenItem(id) => match host.open_item(&id) {
            Ok(()) => respond_json_with(request, 200, serde_json::json!({"ok": true}), &cors),
            Err(e) => respond_json_with(
                request,
                404,
                serde_json::json!({"error": format!("{e:#}")}),
                &cors,
            ),
        },
        Route::ExportItem(id) => {
            let Some(format) = export::parse_format(params.get("format").map(String::as_str)) else {
                return respond_json_with(
                    request,
                    400,
                    serde_json::json!({"error": "format must be md, txt, srt or vtt"}),
                    &cors,
                );
            };
            let archive = match host.archive_dir() {
                Ok(a) => a,
                Err(e) => {
                    return respond_json_with(
                        request,
                        500,
                        serde_json::json!({"error": format!("{e:#}")}),
                        &cors,
                    )
                }
            };
            match export::render(&archive, &id, format) {
                Ok(x) => {
                    let mut headers = cors.clone();
                    headers.push(("Content-Type", x.content_type.to_string()));
                    headers.push((
                        "Content-Disposition",
                        format!("attachment; filename=\"{}\"", x.filename.replace('"', "")),
                    ));
                    let response = tiny_http::Response::from_string(x.body);
                    let _ = request.respond(with_headers(response, &headers));
                }
                Err(export::ExportError::NotFound(e)) => respond_json_with(
                    request,
                    404,
                    serde_json::json!({"error": format!("{e:#}")}),
                    &cors,
                ),
                Err(export::ExportError::Refused(e)) => respond_json_with(
                    request,
                    422,
                    serde_json::json!({"error": format!("{e:#}")}),
                    &cors,
                ),
            }
        }
        _ => respond_json(request, 404, serde_json::json!({"error": "unknown endpoint"})),
    }
}

/// Pure: is this a WebSocket upgrade request we can accept? Returns the
/// `Sec-WebSocket-Accept` value.
pub fn websocket_accept(
    upgrade: Option<&str>,
    version: Option<&str>,
    key: Option<&str>,
) -> Result<String, &'static str> {
    if !upgrade.is_some_and(|u| u.split(',').any(|p| p.trim().eq_ignore_ascii_case("websocket"))) {
        return Err("expected a WebSocket upgrade");
    }
    if version.map(str::trim) != Some("13") {
        return Err("unsupported WebSocket version (13 required)");
    }
    let key = key.map(str::trim).filter(|k| !k.is_empty()).ok_or("missing Sec-WebSocket-Key")?;
    Ok(tungstenite::handshake::derive_accept_key(key.as_bytes()))
}

fn upgrade_live(host: &Arc<dyn Host>, request: tiny_http::Request) {
    let accept = match websocket_accept(
        header(&request, "Upgrade").as_deref(),
        header(&request, "Sec-WebSocket-Version").as_deref(),
        header(&request, "Sec-WebSocket-Key").as_deref(),
    ) {
        Ok(a) => a,
        Err(e) => {
            let mut r = tiny_http::Response::from_string(e).with_status_code(426);
            if let Ok(h) = tiny_http::Header::from_bytes(&b"Sec-WebSocket-Version"[..], &b"13"[..]) {
                r.add_header(h);
            }
            let _ = request.respond(r);
            return;
        }
    };
    let response = tiny_http::Response::empty(101).with_header(
        tiny_http::Header::from_bytes(&b"Sec-WebSocket-Accept"[..], accept.as_bytes())
            .expect("header"),
    );
    let stream = request.upgrade("websocket", response);
    let host = host.clone();
    std::thread::spawn(move || {
        let ws = tungstenite::WebSocket::from_raw_socket(
            stream,
            tungstenite::protocol::Role::Server,
            Some(live::ws_config()),
        );
        live::serve(ws, &*host);
    });
}

// ---- the app's host ----------------------------------------------------------

/// [`Host`] backed by the running app.
pub struct AppHost {
    pub app: AppHandle,
}

impl Host for AppHost {
    fn config(&self) -> ApiConfig {
        let state = self.app.state::<AppState>();
        let s = state.settings.lock().unwrap();
        ApiConfig {
            extension_token: s.extension_token.clone(),
            subtitles: s.subtitles,
        }
    }

    fn clean(&self, text: &str) -> serde_json::Value {
        let state = self.app.state::<AppState>();
        let settings = state.settings.lock().unwrap().clone();
        let cleaned = crate::cleanup::ollama::cleanup(&settings, None, text);
        // #122: an external cleanup profile without the opt-in returns
        // the text unchanged — say why instead of looking broken.
        if settings.cleanup_blocked() {
            serde_json::json!({
                "cleaned": cleaned,
                "warning": "cleanup profile is external and not enabled for cleanup in Settings: text returned unchanged, nothing sent"
            })
        } else {
            serde_json::json!({"cleaned": cleaned})
        }
    }

    fn transcribe(&self, bytes: Vec<u8>, ext: &str) -> (u16, serde_json::Value) {
        let samples = match crate::audio::decode::decode_bytes_16k_mono(bytes, ext) {
            Ok(s) => s,
            Err(e) => return (400, serde_json::json!({"error": format!("{e:#}")})),
        };
        let state = self.app.state::<AppState>();
        match crate::pipeline::transcribe_batch(&state, &samples) {
            Ok((raw, cleaned)) => (200, serde_json::json!({"raw": raw, "cleaned": cleaned})),
            Err(e) => (500, serde_json::json!({"error": format!("{e:#}")})),
        }
    }

    fn history(&self, query: &str, n: usize) -> serde_json::Value {
        let state = self.app.state::<AppState>();
        let entries = crate::history::search(&state.paths.history_file, query, n);
        serde_json::to_value(entries).unwrap_or(serde_json::json!([]))
    }

    fn archive_dir(&self) -> anyhow::Result<PathBuf> {
        let state = self.app.state::<AppState>();
        let settings = state.settings.lock().unwrap().clone();
        crate::state::resolve_archive_dir(&state.paths, &settings)
    }

    fn open_item(&self, id: &str) -> anyhow::Result<()> {
        let archive = self.archive_dir()?;
        crate::archive::read_item(&archive, id)?;
        if let Some(w) = self.app.get_webview_window("main") {
            let _ = w.unminimize();
            let _ = w.show();
            let _ = w.set_focus();
        }
        // The workspace selects the item in the Library.
        let _ = self.app.emit_to("main", "open-item", id.to_string());
        Ok(())
    }

    fn start_meeting(&self, meeting: live::MeetingStart) -> anyhow::Result<u64> {
        crate::engine::session::start_meeting(&self.app, meeting)
    }
}

#[cfg(test)]
mod tests;
