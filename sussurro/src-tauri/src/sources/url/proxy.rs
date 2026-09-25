//! The address guard for yt-dlp (#216): a loopback HTTP proxy, in process,
//! that yt-dlp is pointed at (`--proxy`), so **every** connection it makes
//! — the page, its API calls, redirects, the media and its fragments — goes
//! through the same rules as a direct download ([`super::resolve_checked`]):
//! the host is resolved here, every address is checked, and the connection
//! is opened to exactly those addresses (pinned, no DNS rebinding). yt-dlp
//! itself never resolves a host while it uses a proxy.
//!
//! It speaks the two forms HTTP clients use with a proxy: `CONNECT
//! host:port` for https (a byte tunnel; TLS stays end to end between yt-dlp
//! and the site, so the certificate is still checked against the host
//! name) and an absolute-form request (`GET http://host/path`) for plain
//! http, forwarded with `Connection: close`. Anything else is refused.
//!
//! It listens on `127.0.0.1` on a random port, only while one yt-dlp run
//! lasts ([`GuardProxy`] is dropped with the run: the listener closes and
//! every open connection is shut down). It needs no authentication: it only
//! relays to addresses the link rules allow for this run, which any program
//! on this computer can reach directly anyway.

use super::{resolve_checked, validate_url};
use anyhow::{anyhow, bail, Result};
use reqwest::Url;
use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

/// Longest request head (request line + headers) accepted.
const MAX_HEAD: usize = 16 * 1024;
/// How long a client may take to send its request head.
const HEAD_TIMEOUT: Duration = Duration::from_secs(30);
/// Connect timeout per checked address (the model downloads' value).
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

struct Shared {
    allow_local: bool,
    stop: AtomicBool,
    next_id: AtomicU64,
    /// Open sockets (both sides of every relayed connection), shut down
    /// when the proxy stops so no relay thread outlives the run.
    open: Mutex<HashMap<u64, Vec<TcpStream>>>,
    /// Why the last connection was refused, for the run's error message.
    refused: Mutex<Option<String>>,
}

impl Shared {
    fn lock_open(&self) -> std::sync::MutexGuard<'_, HashMap<u64, Vec<TcpStream>>> {
        self.open.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// A running guard proxy. Dropping it stops it.
pub struct GuardProxy {
    addr: SocketAddr,
    shared: Arc<Shared>,
    accept: Option<JoinHandle<()>>,
}

impl GuardProxy {
    /// Listen on a random loopback port. `allow_local` is the run's *Allow
    /// local network addresses* tick.
    pub fn start(allow_local: bool) -> Result<Self> {
        let listener = TcpListener::bind(("127.0.0.1", 0))
            .map_err(|e| anyhow!("could not start the link address guard: {e}"))?;
        let addr = listener.local_addr()?;
        let shared = Arc::new(Shared {
            allow_local,
            stop: AtomicBool::new(false),
            next_id: AtomicU64::new(0),
            open: Mutex::new(HashMap::new()),
            refused: Mutex::new(None),
        });
        let s = shared.clone();
        let accept = std::thread::Builder::new()
            .name("link-guard-proxy".into())
            .spawn(move || {
                for conn in listener.incoming() {
                    if s.stop.load(Ordering::SeqCst) {
                        break;
                    }
                    let Ok(conn) = conn else { continue };
                    let s = s.clone();
                    let _ = std::thread::Builder::new()
                        .name("link-guard-conn".into())
                        .spawn(move || handle(conn, &s));
                }
            })?;
        Ok(Self {
            addr,
            shared,
            accept: Some(accept),
        })
    }

    /// `http://127.0.0.1:<port>`, for yt-dlp's `--proxy`.
    pub fn url(&self) -> String {
        format!("http://{}", self.addr)
    }

    /// Why the last connection was refused (a local address, a bad
    /// request), if one was.
    pub fn last_refusal(&self) -> Option<String> {
        self.shared
            .refused
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }
}

impl Drop for GuardProxy {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::SeqCst);
        // Wake the accept loop.
        let _ = TcpStream::connect_timeout(&self.addr, Duration::from_secs(1));
        if let Some(t) = self.accept.take() {
            let _ = t.join();
        }
        for socks in self.shared.lock_open().values() {
            for s in socks {
                let _ = s.shutdown(Shutdown::Both);
            }
        }
    }
}

/// A parsed request head.
#[derive(Debug, PartialEq)]
pub(crate) struct Head {
    pub method: String,
    pub target: String,
    pub version: String,
    /// Header lines as sent (name, value).
    pub headers: Vec<(String, String)>,
}

/// Parse `head` (without the final blank line). Pure.
pub(crate) fn parse_head(head: &str) -> Result<Head> {
    let mut lines = head.split("\r\n");
    let request = lines.next().unwrap_or_default();
    let mut parts = request.split(' ');
    let (Some(method), Some(target), Some(version), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        bail!("malformed request line");
    };
    if method.is_empty() || target.is_empty() || !version.starts_with("HTTP/1.") {
        bail!("malformed request line");
    }
    let mut headers = Vec::new();
    for line in lines.filter(|l| !l.is_empty()) {
        let (name, value) = line
            .split_once(':')
            .ok_or_else(|| anyhow!("malformed header"))?;
        headers.push((name.trim().to_string(), value.trim().to_string()));
    }
    Ok(Head {
        method: method.to_string(),
        target: target.to_string(),
        version: version.to_string(),
        headers,
    })
}

/// Where a request goes: the URL whose host the rules check, and, for
/// plain http, the head to send upstream (origin-form, hop-by-hop and
/// proxy headers dropped, `Connection: close`). Pure (no DNS).
pub(crate) fn route(head: &Head) -> Result<(Url, Option<String>)> {
    if head.method.eq_ignore_ascii_case("CONNECT") {
        // `host:port` only — no scheme, path or credentials.
        let t = &head.target;
        if t.contains(['/', '@', '?', '#']) || !t.contains(':') {
            bail!("bad CONNECT target {t}");
        }
        let url = validate_url(&format!("https://{t}/"))?;
        if url.port().is_none() && !t.ends_with(":443") {
            bail!("bad CONNECT target {t}");
        }
        return Ok((url, None));
    }
    let url = validate_url(&head.target)?;
    if url.scheme() != "http" {
        bail!("only http requests are forwarded (https goes through CONNECT)");
    }
    let mut path = url.path().to_string();
    if let Some(q) = url.query() {
        path.push('?');
        path.push_str(q);
    }
    let mut out = format!("{} {path} {}\r\n", head.method, head.version);
    for (name, value) in &head.headers {
        let n = name.to_ascii_lowercase();
        if matches!(
            n.as_str(),
            "connection" | "proxy-connection" | "keep-alive" | "proxy-authorization"
        ) {
            continue;
        }
        out.push_str(&format!("{name}: {value}\r\n"));
    }
    out.push_str("Connection: close\r\n\r\n");
    Ok((url, Some(out)))
}

/// Read the request head; returns it and any bytes read past it.
fn read_head(conn: &mut TcpStream) -> Result<(String, Vec<u8>)> {
    let mut buf = Vec::with_capacity(1024);
    let mut chunk = [0u8; 4096];
    loop {
        if let Some(end) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            let rest = buf.split_off(end + 4);
            buf.truncate(end);
            let head = String::from_utf8(buf).map_err(|_| anyhow!("request head is not text"))?;
            return Ok((head, rest));
        }
        if buf.len() > MAX_HEAD {
            bail!("request head too long");
        }
        let n = conn.read(&mut chunk)?;
        if n == 0 {
            bail!("connection closed before the request");
        }
        buf.extend_from_slice(&chunk[..n]);
    }
}

fn respond(conn: &mut TcpStream, status: &str, msg: &str) {
    let _ = write!(
        conn,
        "HTTP/1.1 {status}\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{msg}",
        msg.len()
    );
    let _ = conn.flush();
}

/// Open a connection to the first of `addrs` that answers.
fn connect_any(addrs: &[SocketAddr]) -> std::io::Result<TcpStream> {
    let mut last = None;
    for a in addrs {
        match TcpStream::connect_timeout(a, CONNECT_TIMEOUT) {
            Ok(s) => return Ok(s),
            Err(e) => last = Some(e),
        }
    }
    Err(last.unwrap_or_else(|| std::io::Error::other("no address to connect to")))
}

fn handle(mut conn: TcpStream, s: &Shared) {
    let _ = conn.set_read_timeout(Some(HEAD_TIMEOUT));
    let Ok((head, rest)) = read_head(&mut conn) else {
        return;
    };
    let refuse = |conn: &mut TcpStream, status: &str, e: &anyhow::Error| {
        let msg = format!("{e:#}");
        *s.refused.lock().unwrap_or_else(|e| e.into_inner()) = Some(msg.clone());
        respond(conn, status, &msg);
    };
    let (url, upstream_head) = match parse_head(&head).and_then(|h| route(&h)) {
        Ok(r) => r,
        Err(e) => return refuse(&mut conn, "400 Bad Request", &e),
    };
    // The link rules: resolve here, check every address, pin to them.
    let addrs = match resolve_checked(&url, s.allow_local) {
        Ok(a) => a,
        Err(e) => return refuse(&mut conn, "403 Forbidden", &e),
    };
    let mut upstream = match connect_any(&addrs) {
        Ok(u) => u,
        Err(e) => {
            let host = url.host_str().unwrap_or_default();
            respond(
                &mut conn,
                "502 Bad Gateway",
                &format!("could not connect to {host}: {e}"),
            );
            return;
        }
    };
    let _ = conn.set_read_timeout(None);
    let sent = match &upstream_head {
        Some(h) => upstream.write_all(h.as_bytes()),
        None => conn.write_all(b"HTTP/1.1 200 Connection established\r\n\r\n"),
    };
    if sent.is_err() || upstream.write_all(&rest).is_err() {
        return;
    }
    relay(conn, upstream, s);
}

/// Copy bytes both ways until either side closes.
fn relay(client: TcpStream, upstream: TcpStream, s: &Shared) {
    let id = s.next_id.fetch_add(1, Ordering::Relaxed);
    let clones = [client.try_clone(), upstream.try_clone()];
    let (Ok(c2), Ok(u2)) = (client.try_clone(), upstream.try_clone()) else {
        return;
    };
    s.lock_open()
        .insert(id, clones.into_iter().flatten().collect());
    if s.stop.load(Ordering::SeqCst) {
        let _ = client.shutdown(Shutdown::Both);
        let _ = upstream.shutdown(Shutdown::Both);
    }
    let up = std::thread::spawn(move || {
        let (mut from, mut to) = (c2, u2);
        let _ = std::io::copy(&mut from, &mut to);
        let _ = to.shutdown(Shutdown::Write);
    });
    let (mut from, mut to) = (upstream, client);
    let _ = std::io::copy(&mut from, &mut to);
    // The server is done (or gone): close the client side too, which also
    // ends the other direction.
    let _ = to.shutdown(Shutdown::Both);
    let _ = from.shutdown(Shutdown::Both);
    let _ = up.join();
    s.lock_open().remove(&id);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sources::url::direct::tests::serve;

    fn head(s: &str) -> Head {
        parse_head(s).unwrap()
    }

    #[test]
    fn request_heads_are_parsed_and_routed() {
        let h = head("CONNECT www.youtube.com:443 HTTP/1.1\r\nHost: www.youtube.com:443");
        assert_eq!(h.method, "CONNECT");
        let (url, fwd) = route(&h).unwrap();
        assert_eq!(url.host_str(), Some("www.youtube.com"));
        assert_eq!(url.port_or_known_default(), Some(443));
        assert!(fwd.is_none());
        let (url, _) = route(&head("CONNECT [2001:db8::1]:8443 HTTP/1.1")).unwrap();
        assert_eq!(url.port(), Some(8443));

        let h = head(
            "GET http://cdn.example.com/a/b.m4a?x=1 HTTP/1.1\r\nHost: cdn.example.com\r\n\
             Proxy-Connection: keep-alive\r\nConnection: keep-alive\r\nKeep-Alive: 5\r\n\
             Proxy-Authorization: Basic eDp5\r\nRange: bytes=0-",
        );
        let (url, fwd) = route(&h).unwrap();
        assert_eq!(url.host_str(), Some("cdn.example.com"));
        let fwd = fwd.unwrap();
        assert_eq!(
            fwd,
            "GET /a/b.m4a?x=1 HTTP/1.1\r\nHost: cdn.example.com\r\nRange: bytes=0-\r\n\
             Connection: close\r\n\r\n"
        );

        for bad in [
            "CONNECT www.youtube.com HTTP/1.1",
            "CONNECT user@host:443 HTTP/1.1",
            "CONNECT host:443/path HTTP/1.1",
            "GET /relative HTTP/1.1",
            "GET https://example.com/ HTTP/1.1",
            "GET ftp://example.com/ HTTP/1.1",
            "GET http://u:p@example.com/ HTTP/1.1",
        ] {
            assert!(route(&head(bad)).is_err(), "{bad}");
        }
        for bad in [
            "",
            "GET",
            "GET / HTTP/1.1 x",
            "GET / SPDY/3",
            "GET / HTTP/1.1\r\nno colon",
        ] {
            assert!(parse_head(bad).is_err(), "{bad:?}");
        }
    }

    /// Send `req` to the proxy and read everything it answers.
    fn exchange(proxy: &GuardProxy, req: &[u8]) -> String {
        let mut c = TcpStream::connect(proxy.addr).unwrap();
        c.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
        c.write_all(req).unwrap();
        let mut out = Vec::new();
        let _ = c.read_to_end(&mut out);
        String::from_utf8_lossy(&out).into_owned()
    }

    #[test]
    fn local_addresses_are_refused_unless_allowed() {
        let srv = serve(vec![("/a", (200, vec![], b"hello".to_vec()))]);
        let authority = srv.base.trim_start_matches("http://").to_string();

        let guard = GuardProxy::start(false).unwrap();
        assert!(guard.url().starts_with("http://127.0.0.1:"));
        for req in [
            format!("CONNECT {authority} HTTP/1.1\r\nHost: {authority}\r\n\r\n"),
            format!("GET {}/a HTTP/1.1\r\nHost: {authority}\r\n\r\n", srv.base),
            "CONNECT localhost:443 HTTP/1.1\r\n\r\n".to_string(),
            "CONNECT 169.254.169.254:80 HTTP/1.1\r\n\r\n".to_string(),
            "CONNECT [::1]:443 HTTP/1.1\r\n\r\n".to_string(),
        ] {
            let out = exchange(&guard, req.as_bytes());
            assert!(out.starts_with("HTTP/1.1 403"), "{req}: {out}");
            assert!(out.contains("Allow local network addresses"), "{out}");
        }
        assert!(guard
            .last_refusal()
            .is_some_and(|r| r.contains("local network")));
        let out = exchange(&guard, b"BREW /pot HTTP/1.1\r\n\r\n");
        assert!(out.starts_with("HTTP/1.1 400"), "{out}");
    }

    #[test]
    fn allowed_connections_are_tunnelled_and_forwarded() {
        let srv = serve(vec![("/a?x=1", (200, vec![], b"hello".to_vec()))]);
        let authority = srv.base.trim_start_matches("http://").to_string();
        let guard = GuardProxy::start(true).unwrap();

        // CONNECT: a byte tunnel; the request inside goes as is (sent
        // together with the CONNECT, as a client may).
        let out = exchange(
            &guard,
            format!(
                "CONNECT {authority} HTTP/1.1\r\nHost: {authority}\r\n\r\n\
                 GET /a?x=1 HTTP/1.1\r\nHost: {authority}\r\nConnection: close\r\n\r\n"
            )
            .as_bytes(),
        );
        assert!(
            out.starts_with("HTTP/1.1 200 Connection established\r\n\r\nHTTP/1.1 200"),
            "{out}"
        );
        assert!(out.ends_with("hello"), "{out}");

        // Absolute form: rewritten to origin form and forwarded.
        let out = exchange(
            &guard,
            format!(
                "GET {}/a?x=1 HTTP/1.1\r\nHost: {authority}\r\nProxy-Connection: keep-alive\r\n\r\n",
                srv.base
            )
            .as_bytes(),
        );
        assert!(out.starts_with("HTTP/1.1 200"), "{out}");
        assert!(out.ends_with("hello"), "{out}");
        assert_eq!(guard.last_refusal(), None);

        // A real client through it (reqwest uses the absolute form for http).
        let client = reqwest::blocking::Client::builder()
            .proxy(reqwest::Proxy::all(guard.url()).unwrap())
            .build()
            .unwrap();
        let body = client
            .get(format!("{}/a?x=1", srv.base))
            .send()
            .unwrap()
            .text()
            .unwrap();
        assert_eq!(body, "hello");
    }

    #[test]
    fn stopping_closes_open_tunnels() {
        // A tunnel to a server that never answers stays open until the
        // proxy is dropped.
        let silent = TcpListener::bind("127.0.0.1:0").unwrap();
        let authority = silent.local_addr().unwrap().to_string();
        let guard = GuardProxy::start(true).unwrap();
        let mut c = TcpStream::connect(guard.addr).unwrap();
        c.write_all(format!("CONNECT {authority} HTTP/1.1\r\n\r\n").as_bytes())
            .unwrap();
        let mut buf = [0u8; 64];
        c.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
        let n = c.read(&mut buf).unwrap();
        assert!(String::from_utf8_lossy(&buf[..n]).starts_with("HTTP/1.1 200"));
        let _held = silent.accept().unwrap();
        drop(guard);
        // The proxy side is shut down: the client sees the end.
        let mut rest = Vec::new();
        assert!(matches!(c.read_to_end(&mut rest), Ok(0)));
    }
}
