//! The local HTTP API (loopback only).
//!
//! Token-less routes for local scripts: `POST /clean`, `POST /transcribe`,
//! `GET /history` — answered only with `Settings.api_scripting` on (#215;
//! off on a new install, migrated from `api_enabled` for existing ones).
//! Security properties of the whole module: see [`spawn`].
//!
//! 0.9 routes for the browser extension (#126, plan §6), answered only with
//! the extension token (E6, [`auth`]); always on since #138 removed the
//! `meetings_enabled` preview flag (E12):
//! - `GET /app/version` → `{app, protocol, subtitles}` handshake; `subtitles`
//!   is the subtitles setting (`on_request` | `always`, #133), so the side
//!   panel offers "Create .srt" only when the app doesn't write it itself
//!   (#129); `live_auth: "message"` says `/live` accepts the token as its
//!   first message (#217)
//! - `WS /live` (`?token=`, or `auth {token}` first) → a meeting session
//!   ([`live`], [`protocol`])
//! - `POST /items/{id}/open` → the app comes to the front on that item
//! - `GET /items/{id}/export?format=md|txt|srt|vtt` → [`export`]
//!
//! Item ids contain `/` (`2026/09/2026-09-24-weekly-sync`): they are taken
//! as-is between `/items/` and the action, `%2F` also accepted.
//!
//! 0.11 archive routes for scripts (E14, #249): everything under
//! `/archive`, answered only with `Settings.api_archive` on and an archive
//! token with the right scope (`Authorization: Bearer sua_…`, [`tokens`]).
//! Any browser `Origin` is refused there — extensions too — and no CORS
//! header is ever sent. The read routes (#250) are in [`archive`]; the
//! note-from-text write route comes with #251.

pub mod archive;
pub mod auth;
pub mod export;
pub mod guard;
pub mod live;
pub mod protocol;
pub mod tokens;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tauri::{AppHandle, Emitter, Manager};

use crate::state::AppState;

/// What the API needs from the app. [`AppHost`] in the app; tests run the
/// real server on a fake one.
pub trait Host: Send + Sync + 'static {
    fn config(&self) -> ApiConfig;
    /// An archive token passed [`tokens::authorize`]: record its last use
    /// ([`tokens::touch`] decides when that is worth a save). Hosts without
    /// archive tokens ignore it.
    fn archive_token_used(&self, _id: &str) {}
    /// `POST /clean`: the JSON answer.
    fn clean(&self, text: &str) -> serde_json::Value;
    /// `POST /transcribe`: status and JSON answer.
    fn transcribe(&self, bytes: Vec<u8>, ext: &str) -> (u16, serde_json::Value);
    /// `GET /history`: the JSON answer.
    fn history(&self, query: &str, n: usize) -> serde_json::Value;
    fn archive_dir(&self) -> anyhow::Result<PathBuf>;
    /// The archive's search index file (in app data; never served). Hosts
    /// without one answer the archive routes with a 500.
    fn archive_index(&self) -> anyhow::Result<PathBuf> {
        anyhow::bail!("no archive index")
    }
    /// Bring the app to the front on item `id`; an error if there is none.
    fn open_item(&self, id: &str) -> anyhow::Result<()>;
    /// Run the long-form engine on a browser meeting; returns the session id.
    fn start_meeting(&self, meeting: live::MeetingStart) -> anyhow::Result<u64>;
}

/// The settings the API checks on every request (so regenerating the
/// token, or revoking an archive token, applies at once).
#[derive(Clone, Default, PartialEq)]
pub struct ApiConfig {
    pub extension_token: String,
    /// Told to the extension by `GET /app/version` (#129).
    pub subtitles: crate::settings::SubtitlesMode,
    /// The token-less scripting routes answer (`Settings.api_scripting`).
    pub scripting: bool,
    /// The archive routes answer (`Settings.api_archive`, #249).
    pub archive: bool,
    /// The archive tokens (hashes only).
    pub archive_tokens: Vec<tokens::ArchiveToken>,
}

/// Never prints the extension token (nor, through [`tokens::ArchiveToken`],
/// a hash).
impl std::fmt::Debug for ApiConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ApiConfig")
            .field("extension_token", &if self.extension_token.is_empty() { "" } else { "<redacted>" })
            .field("subtitles", &self.subtitles)
            .field("scripting", &self.scripting)
            .field("archive", &self.archive)
            .field("archive_tokens", &self.archive_tokens)
            .finish()
    }
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
    /// Anything under `/archive` (#249): the archive middleware runs first,
    /// then the archive router (#250, #251).
    Archive,
    NotFound,
}

impl Route {
    /// An archive route: archive token, scopes, no browser origin.
    pub fn is_archive(&self) -> bool {
        matches!(self, Route::Archive)
    }

    /// A token-less scripting route (behind `Settings.api_scripting`).
    pub fn is_scripting(&self) -> bool {
        matches!(self, Route::Clean | Route::Transcribe | Route::History)
    }

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

/// Pure: `/archive` or below it.
pub fn is_archive_path(path: &str) -> bool {
    path == "/archive" || path.starts_with("/archive/")
}

/// Pure: method + path → route.
pub fn route(method: &str, path: &str) -> Route {
    match (method, path) {
        (_, p) if is_archive_path(p) => Route::Archive,
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
/// - Every route, before anything else ([`guard`], #215): `Host` must be
///   `127.0.0.1:<port>` or `localhost:<port>` (defeats DNS rebinding), and a
///   request carrying an `Origin` must come from a browser extension — a
///   web page can't reach even the token-less routes with a cross-site POST.
/// - The token-less scripting routes answer only with
///   `Settings.api_scripting` on (read per request); they never send CORS.
/// - Bodies are capped (`/clean` 1 MiB, `/transcribe` 200 MiB): the declared
///   length is checked before reading, the read goes through `take()`, and
///   the whole body must arrive within [`guard::BODY_DEADLINE`].
/// - A small worker pool ([`WORKERS`]) answers requests; `/clean` and
///   `/transcribe` hold one of [`SLOW_SLOTS`] slots (503 when none is free),
///   so they can't stall the extension's `/app/version` or `/live`. A
///   panicking handler costs one 500, not a worker.
/// - Endpoints accept request bodies / query params only — no filesystem paths
///   or other caller-controlled values are ever passed to the OS (item ids
///   are validated and confined to the archive).
/// - `/items/{id}/open|export` reach only items the extension recorded
///   (`source: browser:…`, [`export::extension_may_access`]): a leaked
///   pairing code doesn't expose the rest of the archive.
/// - Request payloads, URLs (they may carry the extension token) and tokens
///   are never logged: transcripts may contain sensitive text.
/// - Meeting routes: extension token, extension-only origins and CORS
///   ([`auth`]); an unpaired app (no token) accepts nothing there.
/// - Archive routes (`/archive…`, E14, #249), after the checks above and
///   before any route code ([`tokens::authorize`]): **any** `Origin` is
///   refused (extensions too — they are for scripts), then
///   `Settings.api_archive` must be on (read per request), then an archive
///   token (`Authorization: Bearer`, never a query parameter) must match a
///   stored SHA-256 (constant-time), then its per-token rate limit (burst
///   60, 10/s: 429 + `Retry-After`) and its scope (`read` for GET/HEAD,
///   `write` otherwise; `people` for emails). No CORS header, ever. Errors
///   carry a stable `code`. Tokens are created in Settings → Scripting,
///   shown once, stored only as hashes, revocable at once, and never logged.
/// - Archive read routes (#250, [`archive`]; read-only in 0.11, P15): ids
///   go through the archive's own id check and confinement, no route takes
///   a path, and responses are built field by field — never an embedding,
///   a voice profile (P13), saved audio, a path or anything from app data,
///   and people's emails only with the `people` scope (without it the text
///   query and `participant=` can't match an email either). Bounded: pages
///   of ≤ 200 rows behind an opaque cursor, ≤ 100 values per facet, bodies
///   ≤ 16 MiB, ≤ [`ARCHIVE_SLOTS`] archive requests at once (503 +
///   `Retry-After`), so scripts can't take the workers from the extension.
///   Internal errors are a fixed message; `Cache-Control: no-store`.
/// - `settings.json` (it holds the extension token and the archive token
///   hashes) is written 0600 on Unix.
///
/// - The HTTP layer itself (#223): tiny_http 0.12.0 is vendored with
///   patches (`vendor/tiny_http`, each marked `Sussurro (#223)`), because
///   upstream trusted the client's framing. Measured on upstream against
///   this server: `Content-Length: 9223372036854775807` on `GET /history`
///   **aborted the app** (`memory allocation of 9223372036854775807 bytes
///   failed`: dropping a request with its body unread read the rest into a
///   buffer of the declared size); `18446744073709551615` panicked
///   (`capacity overflow`); a 100 TB claim aborted too. Now:
///   - a body dropped before its end is never read or allocated: the
///     connection closes instead (so a refused upload that did not wait for
///     `100 Continue` may see the connection reset rather than the answer);
///   - request line + headers ≤ 64 KiB in ≤ 100 lines (else 431 and close);
///     chunked framing lines ≤ 4 KiB, ≤ 64 trailers;
///   - ambiguous framing (a `Content-Length` that isn't digits or doesn't
///     fit, lengths that disagree, `Transfer-Encoding` not ending in
///     `chunked` or next to a length) is a 400 and close;
///   - at most 128 connections at once (more are closed at accept; each
///     holds a thread), and a 30 s socket read/write timeout while a
///     connection speaks HTTP — lifted on the `/live` upgrade, whose idle
///     waits are the extension's own ([`live`]).
///
/// Residual: a local process can hold every connection (128 idle or
/// trickling sockets, each re-arming the 30 s timeout) and so deny the API
/// to the extension until it stops; it can't grow memory or crash the app
/// that way. A local process can end the app by other means anyway.
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

/// Threads answering requests. `/live` sessions move to a thread of their
/// own after the upgrade, so they don't hold a worker.
pub const WORKERS: usize = 4;
/// `/clean` and `/transcribe` running at once: the other workers stay free
/// for the extension's routes and `/history`.
pub const SLOW_SLOTS: usize = 2;
/// Archive requests running at once (#250): a script firing requests in
/// parallel can't take every worker from the extension.
pub const ARCHIVE_SLOTS: usize = 2;
// A worker always stays free for the extension.
const _: () = assert!(ARCHIVE_SLOTS < WORKERS);

/// What every worker shares.
struct Ctx {
    host: Arc<dyn Host>,
    /// The port the API listens on: the only one `Host` may name.
    port: u16,
    slow: guard::Slots,
    /// Per archive token (#249).
    archive_limiter: tokens::RateLimiter,
    /// See [`ARCHIVE_SLOTS`].
    archive_slots: guard::Slots,
}

/// Answer requests on [`WORKERS`] threads until the server is dropped or
/// unblocked.
pub fn serve(server: tiny_http::Server, host: Arc<dyn Host>) {
    let port = server.server_addr().to_ip().map_or(0, |a| a.port());
    let server = Arc::new(server);
    let ctx = Arc::new(Ctx {
        host,
        port,
        slow: guard::Slots::new(SLOW_SLOTS),
        archive_limiter: tokens::RateLimiter::default(),
        archive_slots: guard::Slots::new(ARCHIVE_SLOTS),
    });
    let workers: Vec<_> = (0..WORKERS)
        .map(|_| {
            let (server, ctx) = (server.clone(), ctx.clone());
            std::thread::spawn(move || {
                for request in server.incoming_requests() {
                    // A panicking handler: tiny_http answers 500 when the
                    // request drops, and the worker lives on.
                    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        handle(&ctx, request)
                    }));
                }
            })
        })
        .collect();
    for w in workers {
        let _ = w.join();
    }
}

fn body_error(e: guard::BodyError, cap: usize) -> (u16, serde_json::Value) {
    match e {
        guard::BodyError::TooLarge => (
            413,
            serde_json::json!({"error": format!("body too large (at most {} MiB)", cap >> 20)}),
        ),
        guard::BodyError::Empty => (400, serde_json::json!({"error": "empty body"})),
        guard::BodyError::Io => (400, serde_json::json!({"error": "could not read the body"})),
        guard::BodyError::TooSlow => (408, serde_json::json!({"error": "the body did not arrive in time"})),
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

fn handle(ctx: &Arc<Ctx>, mut request: tiny_http::Request) {
    // On every route, before anything else (#215).
    if !guard::host_allowed(header(&request, "Host").as_deref(), ctx.port) {
        return respond_json_with(request, 403, serde_json::json!({"error": "host not allowed"}), &[]);
    }
    if !guard::origin_allowed(header(&request, "Origin").as_deref()) {
        return respond_json_with(request, 403, serde_json::json!({"error": "origin not allowed"}), &[]);
    }
    let host = &ctx.host;
    let url = request.url().to_string();
    let method = request.method().as_str().to_string();
    let (path, params) = parse_url(&url);
    let config = host.config();

    let route = route(&method, path);
    if route.is_archive() {
        return handle_archive(ctx, request, &method, &url, &config);
    }
    if route.is_meeting() {
        return handle_meeting(ctx, request, route, &params, &config);
    }
    if route.is_scripting() && !config.scripting {
        return respond_json_with(
            request,
            403,
            serde_json::json!({"error": "the scripting routes are off: turn on Settings → Scripting → Scripting routes in Sussurro"}),
            &[],
        );
    }
    match route {
        Route::Clean | Route::Transcribe => {
            let Some(_slot) = ctx.slow.try_take() else {
                return respond_json_with(
                    request,
                    503,
                    serde_json::json!({"error": "busy: too many /clean or /transcribe requests at once, retry later"}),
                    &[("Retry-After", "5".to_string())],
                );
            };
            let cap = if route == Route::Clean {
                guard::CLEAN_MAX_BYTES
            } else {
                guard::TRANSCRIBE_MAX_BYTES
            };
            let declared = request.body_length();
            // Before `as_reader()`, which tells an `Expect: 100-continue`
            // client to send the body.
            if declared.is_some_and(|n| n > cap) {
                let (status, body) = body_error(guard::BodyError::TooLarge, cap);
                return respond_json_with(request, status, body, &[]);
            }
            let bytes = match guard::read_capped(
                request.as_reader(),
                declared,
                cap,
                std::time::Instant::now() + guard::BODY_DEADLINE,
            ) {
                Ok(b) => b,
                Err(e) => {
                    let (status, body) = body_error(e, cap);
                    return respond_json_with(request, status, body, &[]);
                }
            };
            if route == Route::Clean {
                let text = String::from_utf8(bytes).unwrap_or_default();
                if text.trim().is_empty() {
                    return respond_json(
                        request,
                        400,
                        serde_json::json!({"error": "empty body (or not UTF-8 text)"}),
                    );
                }
                return respond_json(request, 200, host.clean(&text));
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
            respond_json_with(
                request,
                404,
                serde_json::json!({
                    "error": "unknown endpoint",
                    "endpoints": ["POST /clean (text body)", "POST /transcribe?ext=wav (audio body)", "GET /history?n=20&q="]
                }),
                &[],
            );
        }
    }
}

/// The archive middleware (E14, #249), then the archive router.
fn handle_archive(ctx: &Arc<Ctx>, request: tiny_http::Request, method: &str, url: &str, config: &ApiConfig) {
    let authorization = header(&request, "Authorization");
    let origin = header(&request, "Origin");
    let authorized = match tokens::authorize(
        config.archive,
        &config.archive_tokens,
        authorization.as_deref(),
        origin.as_deref(),
        tokens::required_scope(method),
        &ctx.archive_limiter,
        std::time::Instant::now(),
    ) {
        Ok(a) => a,
        Err(d) => {
            return respond_json_with(
                request,
                d.status(),
                serde_json::json!({"error": d.message(), "code": d.code()}),
                &d.headers(),
            )
        }
    };
    ctx.host.archive_token_used(&authorized.id);
    archive_route(ctx, request, &authorized, method, url)
}

/// Headers on every archive answer: nothing cached, nothing sniffed —
/// and never a CORS header.
const ARCHIVE_HEADERS: [(&str, &str); 2] = [("Cache-Control", "no-store"), ("X-Content-Type-Options", "nosniff")];

/// The archive routes, for an authorized request ([`archive::handle`]; the
/// write route comes with #251 and checks `auth.has(…)` like the people
/// route does for emails).
fn archive_route(ctx: &Arc<Ctx>, request: tiny_http::Request, auth: &tokens::Authorized, method: &str, url: &str) {
    let reply = match ctx.archive_slots.try_take() {
        None => {
            let mut r = archive::Reply::error(503, "busy", "too many archive requests at once, retry later");
            r.headers.push(("Retry-After", "1".to_string()));
            r
        }
        Some(_slot) => match (ctx.host.archive_dir(), ctx.host.archive_index()) {
            (Ok(dir), Ok(index)) => archive::handle(&dir, &index, auth, method, url),
            _ => archive::Reply::internal(),
        },
    };
    let mut headers: Vec<(&str, String)> = ARCHIVE_HEADERS.iter().map(|(k, v)| (*k, v.to_string())).collect();
    headers.extend(reply.headers.iter().cloned());
    match reply.body {
        archive::Body::Json(body) => respond_json_with(request, reply.status, body, &headers),
        archive::Body::File { content_type, filename, text } => {
            headers.push(("Content-Type", content_type.to_string()));
            headers.push((
                "Content-Disposition",
                format!("attachment; filename=\"{}\"", filename.replace(['"', '\\', '\r', '\n'], "")),
            ));
            let response = tiny_http::Response::from_string(text).with_status_code(reply.status);
            let _ = request.respond(with_headers(response, &headers));
        }
    }
}

fn handle_meeting(
    ctx: &Arc<Ctx>,
    request: tiny_http::Request,
    route: Route,
    params: &HashMap<String, String>,
    config: &ApiConfig,
) {
    let host = &ctx.host;
    let origin = header(&request, "Origin");
    let cors = auth::cors_headers(origin.as_deref());
    let deny = |request: tiny_http::Request, d: auth::Denied| {
        respond_json_with(request, d.status(), serde_json::json!({"error": d.message()}), &cors)
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
        // `?token=`, or (#217) the token in the first message.
        return match live::authorize(&config.extension_token, token.as_deref(), origin.as_deref()) {
            Ok(auth) => upgrade_live(host, request, auth),
            Err(d) => deny(request, d),
        };
    }
    let authorization = header(&request, "Authorization");
    if let Err(d) = auth::check_http(
        &config.extension_token,
        authorization.as_deref(),
        origin.as_deref(),
    ) {
        return deny(request, d);
    }
    // The item routes: only items the extension recorded (#215).
    let item_error = |request: tiny_http::Request, e: export::ExportError| {
        let (status, body) = match e {
            export::ExportError::NotFound(e) => (404, serde_json::json!({"error": format!("{e:#}")})),
            export::ExportError::Refused(e) => (422, serde_json::json!({"error": format!("{e:#}")})),
            export::ExportError::NotExtensionItem => (
                403,
                serde_json::json!({
                    "error": "only meetings recorded by the browser extension can be opened or exported from it",
                    "code": export::NOT_EXTENSION_ITEM,
                }),
            ),
        };
        respond_json_with(request, status, body, &cors)
    };
    let archive = || {
        host.archive_dir()
            .map_err(|e| (500, serde_json::json!({"error": format!("{e:#}")})))
    };
    match route {
        Route::AppVersion => respond_json_with(
            request,
            200,
            serde_json::json!({
                "app": env!("CARGO_PKG_VERSION"),
                "protocol": protocol::PROTOCOL_VERSION,
                "protocol_min": protocol::MIN_PROTOCOL,
                "subtitles": config.subtitles,
                // `/live` takes `auth {token}` as its first message (#217).
                "live_auth": "message",
            }),
            &cors,
        ),
        Route::OpenItem(id) => {
            let archive = match archive() {
                Ok(a) => a,
                Err((status, body)) => return respond_json_with(request, status, body, &cors),
            };
            if let Err(e) = export::check_access(&archive, &id) {
                return item_error(request, e);
            }
            match host.open_item(&id) {
                Ok(()) => respond_json_with(request, 200, serde_json::json!({"ok": true}), &cors),
                Err(e) => item_error(request, export::ExportError::NotFound(e)),
            }
        }
        Route::ExportItem(id) => {
            let Some(format) = export::parse_format(params.get("format").map(String::as_str)) else {
                return respond_json_with(
                    request,
                    400,
                    serde_json::json!({"error": "format must be md, txt, srt or vtt"}),
                    &cors,
                );
            };
            let archive = match archive() {
                Ok(a) => a,
                Err((status, body)) => return respond_json_with(request, status, body, &cors),
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
                Err(e) => item_error(request, e),
            }
        }
        _ => respond_json_with(request, 404, serde_json::json!({"error": "unknown endpoint"}), &[]),
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

fn upgrade_live(host: &Arc<dyn Host>, request: tiny_http::Request, auth: live::LiveAuth) {
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
        live::serve(ws, &*host, auth);
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
            scripting: s.api_scripting,
            archive: s.api_archive,
            archive_tokens: if s.api_archive { s.archive_tokens.clone() } else { Vec::new() },
        }
    }

    fn archive_token_used(&self, id: &str) {
        let state = self.app.state::<AppState>();
        let mut s = state.settings.lock().unwrap();
        if tokens::touch(&mut s.archive_tokens, id, chrono::Utc::now()) {
            // Best effort: a failed save loses only the time shown.
            let _ = s.save(&state.paths.settings_file);
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

    fn archive_index(&self) -> anyhow::Result<PathBuf> {
        Ok(self.app.state::<AppState>().paths.archive_index.clone())
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
