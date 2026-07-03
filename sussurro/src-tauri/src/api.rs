use std::collections::HashMap;
use tauri::{AppHandle, Manager};

use crate::state::AppState;

/// Routes exposed by the local API. Pure mapping — unit tested.
#[derive(Debug, PartialEq, Eq)]
pub enum Route {
    Clean,
    Transcribe,
    History,
    NotFound,
}

/// Pure: method + path → route.
pub fn route(method: &str, path: &str) -> Route {
    match (method, path) {
        ("POST", "/clean") => Route::Clean,
        ("POST", "/transcribe") => Route::Transcribe,
        ("GET", "/history") => Route::History,
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
pub fn spawn(app: AppHandle, port: u16) {
    std::thread::spawn(move || {
        let server = match tiny_http::Server::http(("127.0.0.1", port)) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("local API: cannot bind 127.0.0.1:{port}: {e}");
                return;
            }
        };
        eprintln!("local API listening on http://127.0.0.1:{port}");
        for request in server.incoming_requests() {
            handle(&app, request);
        }
    });
}

fn respond_json(request: tiny_http::Request, status: u16, body: serde_json::Value) {
    let data = body.to_string();
    let response = tiny_http::Response::from_string(data)
        .with_status_code(status)
        .with_header(
            tiny_http::Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..])
                .expect("header"),
        );
    let _ = request.respond(response);
}

fn handle(app: &AppHandle, mut request: tiny_http::Request) {
    let url = request.url().to_string();
    let method = request.method().as_str().to_string();
    let (path, params) = parse_url(&url);

    match route(&method, path) {
        Route::Clean => {
            let mut text = String::new();
            if request.as_reader().read_to_string(&mut text).is_err() || text.trim().is_empty() {
                return respond_json(request, 400, serde_json::json!({"error": "empty body"}));
            }
            let state = app.state::<AppState>();
            let settings = state.settings.lock().unwrap().clone();
            let cleaned = crate::cleanup::ollama::cleanup(&settings, None, &text);
            respond_json(request, 200, serde_json::json!({"cleaned": cleaned}));
        }
        Route::Transcribe => {
            let mut bytes = Vec::new();
            if request.as_reader().read_to_end(&mut bytes).is_err() || bytes.is_empty() {
                return respond_json(request, 400, serde_json::json!({"error": "empty body"}));
            }
            let ext = params.get("ext").cloned().unwrap_or_default();
            let samples = match crate::audio::decode::decode_bytes_16k_mono(bytes, &ext) {
                Ok(s) => s,
                Err(e) => {
                    return respond_json(
                        request,
                        400,
                        serde_json::json!({"error": format!("{e:#}")}),
                    )
                }
            };
            let state = app.state::<AppState>();
            match crate::pipeline::transcribe_batch(&state, &samples) {
                Ok((raw, cleaned)) => respond_json(
                    request,
                    200,
                    serde_json::json!({"raw": raw, "cleaned": cleaned}),
                ),
                Err(e) => respond_json(request, 500, serde_json::json!({"error": format!("{e:#}")})),
            }
        }
        Route::History => {
            let n = params
                .get("n")
                .and_then(|v| v.parse::<usize>().ok())
                .unwrap_or(20);
            let query = params.get("q").cloned().unwrap_or_default();
            let state = app.state::<AppState>();
            let entries = crate::history::search(&state.paths.history_file, &query, n);
            respond_json(
                request,
                200,
                serde_json::to_value(entries).unwrap_or(serde_json::json!([])),
            );
        }
        Route::NotFound => {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routes_map_method_and_path() {
        assert_eq!(route("POST", "/clean"), Route::Clean);
        assert_eq!(route("POST", "/transcribe"), Route::Transcribe);
        assert_eq!(route("GET", "/history"), Route::History);
        assert_eq!(route("GET", "/clean"), Route::NotFound); // wrong method
        assert_eq!(route("POST", "/nope"), Route::NotFound);
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
    }
}
