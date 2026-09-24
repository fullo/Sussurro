//! Extension auth for the 0.9 routes (E6, #126). Pure — unit tested.
//!
//! - A random **extension token** (32 bytes, hex) is created for pairing
//!   and stored in the settings. HTTP routes take it as
//!   `Authorization: Bearer <token>`; the `/live` WebSocket, which browsers
//!   can't open with custom headers, as `?token=<token>`.
//! - **Origin**: browsers always send one on a WebSocket and on
//!   cross-origin fetches. Only extension origins (`chrome-extension://`,
//!   `moz-extension://`) are accepted; a web page's origin is refused even
//!   with the right token (a token pasted into the wrong place must not turn
//!   every site into a client). The WebSocket requires an extension origin;
//!   plain HTTP also accepts no origin at all (a local script with the
//!   token — non-browser clients can set any header anyway, the token is
//!   what protects them).
//! - **CORS** headers go only to extension origins; the token-less routes
//!   (`/clean`, `/transcribe`, `/history`) never send any, as before.
//! - Tokens are compared in constant time and never logged.

/// Bytes of randomness in a token (hex-encoded: 64 characters).
pub const TOKEN_BYTES: usize = 32;

/// A fresh random token from the OS generator.
pub fn generate_token() -> anyhow::Result<String> {
    let mut bytes = [0u8; TOKEN_BYTES];
    getrandom::fill(&mut bytes).map_err(|e| anyhow::anyhow!("no OS randomness: {e}"))?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

/// Constant-time equality (for equal lengths; the length itself is not
/// secret — every token is 64 characters).
pub fn tokens_match(expected: &str, given: &str) -> bool {
    let (a, b) = (expected.as_bytes(), given.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// The token of an `Authorization: Bearer <token>` header value.
pub fn bearer_token(header: &str) -> Option<&str> {
    let (scheme, token) = header.trim().split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("bearer") {
        return None;
    }
    let token = token.trim();
    (!token.is_empty()).then_some(token)
}

/// `chrome-extension://<id>` (Chrome, Edge, Brave…) or
/// `moz-extension://<uuid>` (Firefox), with a non-empty id and nothing
/// after it.
pub fn is_extension_origin(origin: &str) -> bool {
    let rest = origin
        .strip_prefix("chrome-extension://")
        .or_else(|| origin.strip_prefix("moz-extension://"));
    match rest {
        Some(id) => {
            !id.is_empty()
                && id
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-')
        }
        None => false,
    }
}

/// Why a request was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Denied {
    /// No token, a wrong one, or none configured yet (not paired): 401.
    Unauthorized,
    /// A web page's origin, or (WebSocket) no extension origin: 403.
    Forbidden,
}

impl Denied {
    pub fn status(self) -> u16 {
        match self {
            Denied::Unauthorized => 401,
            Denied::Forbidden => 403,
        }
    }

    pub fn message(self) -> &'static str {
        match self {
            Denied::Unauthorized => "missing or wrong extension token — pair the extension in Sussurro's settings",
            Denied::Forbidden => "origin not allowed",
        }
    }
}

fn check_token(expected: &str, given: Option<&str>) -> Result<(), Denied> {
    let expected = expected.trim();
    match given {
        // Not paired yet: nothing can match.
        _ if expected.is_empty() => Err(Denied::Unauthorized),
        Some(t) if tokens_match(expected, t) => Ok(()),
        _ => Err(Denied::Unauthorized),
    }
}

/// An HTTP request to a token route: the origin first (a web page is
/// refused whatever it sends), then the bearer token.
pub fn check_http(
    expected: &str,
    authorization: Option<&str>,
    origin: Option<&str>,
) -> Result<(), Denied> {
    if let Some(o) = origin {
        if !is_extension_origin(o) {
            return Err(Denied::Forbidden);
        }
    }
    check_token(expected, authorization.and_then(bearer_token))
}

/// A `/live` WebSocket upgrade: an extension origin is required, then the
/// `?token=`.
pub fn check_ws(expected: &str, query_token: Option<&str>, origin: Option<&str>) -> Result<(), Denied> {
    if !origin.is_some_and(is_extension_origin) {
        return Err(Denied::Forbidden);
    }
    check_token(expected, query_token)
}

/// CORS response headers for `origin`: only extension origins get any.
pub fn cors_headers(origin: Option<&str>) -> Vec<(&'static str, String)> {
    match origin {
        Some(o) if is_extension_origin(o) => vec![
            ("Access-Control-Allow-Origin", o.to_string()),
            ("Vary", "Origin".to_string()),
        ],
        _ => Vec::new(),
    }
}

/// Extra headers of a CORS preflight answer for an extension origin.
pub fn preflight_headers(origin: Option<&str>) -> Option<Vec<(&'static str, String)>> {
    let mut h = cors_headers(origin);
    if h.is_empty() {
        return None;
    }
    h.push(("Access-Control-Allow-Methods", "GET, POST, OPTIONS".to_string()));
    h.push((
        "Access-Control-Allow-Headers",
        "Authorization, Content-Type".to_string(),
    ));
    h.push(("Access-Control-Max-Age", "600".to_string()));
    Some(h)
}

#[cfg(test)]
mod tests {
    use super::*;

    const T: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
    const CHROME: &str = "chrome-extension://abcdefghijklmnopabcdefghijklmnop";
    const FIREFOX: &str = "moz-extension://2d6b6c1e-3f7a-4b1e-9d0a-1c2b3d4e5f60";

    #[test]
    fn tokens_are_random_hex_of_fixed_length() {
        let a = generate_token().unwrap();
        let b = generate_token().unwrap();
        assert_eq!(a.len(), 2 * TOKEN_BYTES);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, b);
    }

    #[test]
    fn token_comparison() {
        assert!(tokens_match(T, T));
        assert!(!tokens_match(T, &T.replace('f', "e")));
        assert!(!tokens_match(T, &T[1..]));
        assert!(!tokens_match(T, ""));
    }

    #[test]
    fn bearer_header_parsing() {
        assert_eq!(bearer_token("Bearer abc"), Some("abc"));
        assert_eq!(bearer_token("bearer  abc "), Some("abc"));
        assert_eq!(bearer_token("Basic abc"), None);
        assert_eq!(bearer_token("Bearer "), None);
        assert_eq!(bearer_token("abc"), None);
    }

    #[test]
    fn only_extension_origins_are_extension_origins() {
        assert!(is_extension_origin(CHROME));
        assert!(is_extension_origin(FIREFOX));
        for o in [
            "https://meet.google.com",
            "http://127.0.0.1:4525",
            "null",
            "chrome-extension://",
            "chrome-extension://abc/path",
            "chrome-extension://abc.evil.com",
            "safari-web-extension://abc",
            "",
        ] {
            assert!(!is_extension_origin(o), "{o}");
        }
    }

    #[test]
    fn http_checks_origin_then_bearer_token() {
        let bearer = format!("Bearer {T}");
        assert_eq!(check_http(T, Some(&bearer), Some(CHROME)), Ok(()));
        assert_eq!(check_http(T, Some(&bearer), Some(FIREFOX)), Ok(()));
        // A local script: no origin, right token.
        assert_eq!(check_http(T, Some(&bearer), None), Ok(()));
        // A web page is refused even with the token.
        assert_eq!(
            check_http(T, Some(&bearer), Some("https://evil.example")),
            Err(Denied::Forbidden)
        );
        assert_eq!(check_http(T, None, Some(CHROME)), Err(Denied::Unauthorized));
        assert_eq!(
            check_http(T, Some("Bearer nope"), Some(CHROME)),
            Err(Denied::Unauthorized)
        );
        // Not paired yet: even an empty bearer never matches.
        assert_eq!(check_http("", Some("Bearer "), None), Err(Denied::Unauthorized));
        assert_eq!(check_http(" ", Some("Bearer x"), None), Err(Denied::Unauthorized));
    }

    #[test]
    fn websocket_requires_an_extension_origin_and_the_query_token() {
        assert_eq!(check_ws(T, Some(T), Some(CHROME)), Ok(()));
        assert_eq!(check_ws(T, Some(T), None), Err(Denied::Forbidden));
        assert_eq!(
            check_ws(T, Some(T), Some("https://meet.google.com")),
            Err(Denied::Forbidden)
        );
        assert_eq!(check_ws(T, None, Some(FIREFOX)), Err(Denied::Unauthorized));
        assert_eq!(check_ws(T, Some("x"), Some(FIREFOX)), Err(Denied::Unauthorized));
        assert_eq!(check_ws("", Some(""), Some(FIREFOX)), Err(Denied::Unauthorized));
        assert_eq!(Denied::Unauthorized.status(), 401);
        assert_eq!(Denied::Forbidden.status(), 403);
    }

    #[test]
    fn cors_only_for_extension_origins() {
        let h = cors_headers(Some(CHROME));
        assert!(h.contains(&("Access-Control-Allow-Origin", CHROME.to_string())));
        assert!(h.contains(&("Vary", "Origin".to_string())));
        assert!(cors_headers(Some("https://meet.google.com")).is_empty());
        assert!(cors_headers(None).is_empty());
        let pre = preflight_headers(Some(FIREFOX)).unwrap();
        assert!(pre
            .iter()
            .any(|(k, v)| *k == "Access-Control-Allow-Headers" && v.contains("Authorization")));
        assert!(preflight_headers(Some("https://x.example")).is_none());
        assert!(preflight_headers(None).is_none());
    }
}
