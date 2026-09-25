//! Archive API tokens (E14, #249). Pure — unit tested.
//!
//! Scripts reach the archive through `/archive/…` routes (the routes
//! themselves: #250, #251) with a token of their own, separate from the
//! extension token (E6) and from the token-less scripting routes.
//!
//! - **Creation**: 32 random bytes from the OS generator, hex, with the
//!   [`TOKEN_PREFIX`] so a leaked one is recognisable. The plaintext is
//!   returned once ([`NewToken`]) for Settings → Scripting to show and copy;
//!   it is never stored, logged or sent again.
//! - **Storage**: only the SHA-256 of the token ([`ArchiveToken::sha256`]),
//!   with a name, scopes, the creation time and the last-used time, in
//!   `settings.json` (written 0600, #215). The hash is not a secret worth
//!   the OS keychain (#159): 256 random bits can't be recovered from it and
//!   the API never accepts it in place of the token — whereas the keychain
//!   would be read on every request (slow, and a prompt on some systems).
//!   The UI never receives the hashes ([`TokenInfo`]).
//! - **Lookup**: the given token is hashed and compared in constant time
//!   with every stored hash (no early exit).
//! - **Scopes**: `read` (items, search, exports, documents, people's names),
//!   `people` (people's emails, on top of `read`), `write` (create a note).
//!   Independent: a `write`-only token can add notes but read nothing.
//! - **Order of the checks** ([`authorize`]), after the #215 guard (Host,
//!   web Origins): **any** `Origin` is refused, extensions included — the
//!   archive routes are for scripts and send no CORS headers; then the
//!   `api_archive` switch; then the bearer token; then the per-token rate
//!   limit ([`RateLimiter`]); then the scope.
//! - **Revocation** removes the entry: the next request with that token is
//!   a 401 (the API reads the tokens on every request).
//! - Error messages and `Debug` output never carry a token or a hash.

use crate::api::auth;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Every archive token starts with this, then 64 hex characters.
pub const TOKEN_PREFIX: &str = "sua_";
/// Bytes of randomness in a token.
pub const TOKEN_BYTES: usize = 32;
/// Tokens kept at most: enough for every script, small enough to scan.
pub const MAX_TOKENS: usize = 32;
/// A token's name, in characters.
pub const MAX_NAME_CHARS: usize = 64;
/// Rate limit per token: a burst of this many requests…
pub const RATE_BURST: f64 = 60.0;
/// …refilled at this many per second.
pub const RATE_PER_SEC: f64 = 10.0;
/// `last_used` is saved at most this often per token, so a busy script
/// doesn't rewrite `settings.json` on every request.
pub const LAST_USED_EVERY: chrono::TimeDelta = chrono::TimeDelta::seconds(60);

/// What an archive token may do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Scope {
    /// List, search, read and export items and documents; people's names.
    Read,
    /// People's emails (with `read`).
    People,
    /// Create a note from text.
    Write,
}

impl Scope {
    pub fn as_str(self) -> &'static str {
        match self {
            Scope::Read => "read",
            Scope::People => "people",
            Scope::Write => "write",
        }
    }
}

/// A stored archive token: the hash, never the token.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct ArchiveToken {
    /// Public id (16 hex characters): revoke, rate limit, last-used.
    pub id: String,
    pub name: String,
    /// Lowercase hex SHA-256 of the whole token (prefix included).
    pub sha256: String,
    pub scopes: Vec<Scope>,
    /// RFC 3339, UTC.
    pub created: String,
    #[serde(default)]
    pub last_used: Option<String>,
}

impl std::fmt::Debug for ArchiveToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ArchiveToken")
            .field("id", &self.id)
            .field("name", &self.name)
            .field("sha256", &"<redacted>")
            .field("scopes", &self.scopes)
            .field("created", &self.created)
            .field("last_used", &self.last_used)
            .finish()
    }
}

/// A token as the UI lists it: no hash.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TokenInfo {
    pub id: String,
    pub name: String,
    pub scopes: Vec<Scope>,
    pub created: String,
    pub last_used: Option<String>,
}

impl From<&ArchiveToken> for TokenInfo {
    fn from(t: &ArchiveToken) -> Self {
        Self {
            id: t.id.clone(),
            name: t.name.clone(),
            scopes: t.scopes.clone(),
            created: t.created.clone(),
            last_used: t.last_used.clone(),
        }
    }
}

/// A token just created: the only moment its plaintext exists outside the
/// script that will use it.
#[derive(Clone, Serialize)]
pub struct NewToken {
    pub token: String,
    pub info: TokenInfo,
}

impl std::fmt::Debug for NewToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NewToken")
            .field("token", &"<redacted>")
            .field("info", &self.info)
            .finish()
    }
}

/// Lowercase hex SHA-256 of `token`.
pub fn hash_token(token: &str) -> String {
    Sha256::digest(token.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn random_hex(bytes: usize) -> anyhow::Result<String> {
    let mut buf = vec![0u8; bytes];
    getrandom::fill(&mut buf).map_err(|e| anyhow::anyhow!("no OS randomness: {e}"))?;
    Ok(buf.iter().map(|b| format!("{b:02x}")).collect())
}

/// A fresh token: [`TOKEN_PREFIX`] + 64 hex characters.
pub fn generate_token() -> anyhow::Result<String> {
    Ok(format!("{TOKEN_PREFIX}{}", random_hex(TOKEN_BYTES)?))
}

/// A trimmed, non-empty name of at most [`MAX_NAME_CHARS`] printable
/// characters, not already used (case-insensitive).
pub fn validate_name(name: &str, existing: &[ArchiveToken]) -> Result<String, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("give the token a name (what script uses it)".into());
    }
    if name.chars().count() > MAX_NAME_CHARS {
        return Err(format!(
            "the name is too long (at most {MAX_NAME_CHARS} characters)"
        ));
    }
    if name.chars().any(char::is_control) {
        return Err("the name can't contain control characters".into());
    }
    if existing
        .iter()
        .any(|t| t.name.to_lowercase() == name.to_lowercase())
    {
        return Err(format!(
            "a token named \u{201c}{name}\u{201d} already exists"
        ));
    }
    Ok(name.to_string())
}

/// Sorted, without duplicates, not empty.
pub fn normalize_scopes(scopes: &[Scope]) -> Result<Vec<Scope>, String> {
    let mut s = scopes.to_vec();
    s.sort();
    s.dedup();
    if s.is_empty() {
        return Err("choose at least one scope".into());
    }
    Ok(s)
}

/// Create a token named `name` with `scopes` at `now`: the entry to store
/// and the plaintext to show once.
pub fn create(
    existing: &[ArchiveToken],
    name: &str,
    scopes: &[Scope],
    now: DateTime<Utc>,
) -> Result<(ArchiveToken, NewToken), String> {
    let token = generate_token().map_err(|e| e.to_string())?;
    let id = random_hex(8).map_err(|e| e.to_string())?;
    create_with(existing, name, scopes, now, token, id)
}

/// [`create`] with the token and id given (tests).
pub fn create_with(
    existing: &[ArchiveToken],
    name: &str,
    scopes: &[Scope],
    now: DateTime<Utc>,
    token: String,
    id: String,
) -> Result<(ArchiveToken, NewToken), String> {
    if existing.len() >= MAX_TOKENS {
        return Err(format!(
            "at most {MAX_TOKENS} tokens: revoke one you no longer use"
        ));
    }
    let name = validate_name(name, existing)?;
    let scopes = normalize_scopes(scopes)?;
    let stored = ArchiveToken {
        id,
        name,
        sha256: hash_token(&token),
        scopes,
        created: now.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        last_used: None,
    };
    let info = TokenInfo::from(&stored);
    Ok((stored, NewToken { token, info }))
}

/// Remove token `id`; false when there is none.
pub fn revoke(tokens: &mut Vec<ArchiveToken>, id: &str) -> bool {
    let before = tokens.len();
    tokens.retain(|t| t.id != id);
    tokens.len() != before
}

/// The stored token whose hash matches `given`'s. Every entry is compared
/// in constant time, whatever matched first.
pub fn find<'a>(tokens: &'a [ArchiveToken], given: &str) -> Option<&'a ArchiveToken> {
    let hash = hash_token(given);
    let mut found = None;
    for t in tokens {
        if auth::tokens_match(&t.sha256, &hash) && found.is_none() {
            found = Some(t);
        }
    }
    found
}

/// Record that token `id` was used at `now`. True when `last_used` changed
/// (it moves at most every [`LAST_USED_EVERY`]), so the caller saves.
pub fn touch(tokens: &mut [ArchiveToken], id: &str, now: DateTime<Utc>) -> bool {
    let Some(t) = tokens.iter_mut().find(|t| t.id == id) else {
        return false;
    };
    let recent = t
        .last_used
        .as_deref()
        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
        .is_some_and(|last| now.signed_duration_since(last) < LAST_USED_EVERY);
    if recent {
        return false;
    }
    t.last_used = Some(now.to_rfc3339_opts(chrono::SecondsFormat::Secs, true));
    true
}

/// A token bucket per token id ([`RATE_BURST`], [`RATE_PER_SEC`]). Only
/// authenticated requests reach it, so its map holds at most the tokens
/// used since the app started.
#[derive(Debug)]
pub struct RateLimiter {
    burst: f64,
    per_sec: f64,
    buckets: Mutex<HashMap<String, (f64, Instant)>>,
}

impl Default for RateLimiter {
    fn default() -> Self {
        Self::new(RATE_BURST, RATE_PER_SEC)
    }
}

impl RateLimiter {
    pub fn new(burst: f64, per_sec: f64) -> Self {
        Self {
            burst,
            per_sec,
            buckets: Mutex::new(HashMap::new()),
        }
    }

    /// Take one request for `id` at `now`; `Err(wait)` when the bucket is
    /// empty, with the time until the next request is allowed.
    pub fn check(&self, id: &str, now: Instant) -> Result<(), Duration> {
        let mut buckets = self.buckets.lock().unwrap_or_else(|e| e.into_inner());
        if buckets.len() > 2 * MAX_TOKENS && !buckets.contains_key(id) {
            // Revoked tokens' buckets: drop the stale ones.
            buckets.retain(|_, (_, last)| {
                now.saturating_duration_since(*last) < Duration::from_secs(60)
            });
        }
        let (level, last) = buckets.entry(id.to_string()).or_insert((self.burst, now));
        let refill = now.saturating_duration_since(*last).as_secs_f64() * self.per_sec;
        *level = (*level + refill).min(self.burst);
        *last = now;
        if *level >= 1.0 {
            *level -= 1.0;
            Ok(())
        } else {
            Err(Duration::from_secs_f64((1.0 - *level) / self.per_sec))
        }
    }
}

/// The scope a request needs by its method: reading for `GET`/`HEAD`,
/// writing for anything else. (Emails need [`Scope::People`] on top —
/// checked by the people route, #250.)
pub fn required_scope(method: &str) -> Scope {
    if method.eq_ignore_ascii_case("GET") || method.eq_ignore_ascii_case("HEAD") {
        Scope::Read
    } else {
        Scope::Write
    }
}

/// Why an archive request was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Denied {
    /// Any browser `Origin`, an extension's too: 403.
    Origin,
    /// `Settings.api_archive` is off: 403.
    Off,
    /// No bearer token, or no stored token matches: 401.
    Unauthorized,
    /// The token lacks this scope: 403.
    Scope(Scope),
    /// Over the per-token rate limit: 429, retry after this many seconds.
    RateLimited(u64),
}

impl Denied {
    pub fn status(self) -> u16 {
        match self {
            Denied::Origin | Denied::Off | Denied::Scope(_) => 403,
            Denied::Unauthorized => 401,
            Denied::RateLimited(_) => 429,
        }
    }

    /// Stable, machine-readable: scripts branch on it.
    pub fn code(self) -> &'static str {
        match self {
            Denied::Origin => "origin_refused",
            Denied::Off => "archive_api_off",
            Denied::Unauthorized => "unauthorized",
            Denied::Scope(_) => "insufficient_scope",
            Denied::RateLimited(_) => "rate_limited",
        }
    }

    pub fn message(self) -> String {
        match self {
            Denied::Origin => "the archive API is for scripts: browser requests are refused".into(),
            Denied::Off => "the archive API is off: turn it on in Sussurro → Settings → Scripting".into(),
            Denied::Unauthorized => {
                "missing or wrong archive token: send Authorization: Bearer <token> (create one in Settings → Scripting)".into()
            }
            Denied::Scope(s) => format!("this token lacks the \u{201c}{}\u{201d} scope", s.as_str()),
            Denied::RateLimited(_) => "too many requests with this token: slow down".into(),
        }
    }

    /// Extra response headers (never CORS).
    pub fn headers(self) -> Vec<(&'static str, String)> {
        match self {
            Denied::Unauthorized => vec![(
                "WWW-Authenticate",
                "Bearer realm=\"sussurro-archive\"".into(),
            )],
            Denied::Scope(s) => vec![(
                "WWW-Authenticate",
                format!(
                    "Bearer realm=\"sussurro-archive\", error=\"insufficient_scope\", scope=\"{}\"",
                    s.as_str()
                ),
            )],
            Denied::RateLimited(secs) => vec![("Retry-After", secs.to_string())],
            Denied::Origin | Denied::Off => Vec::new(),
        }
    }
}

/// An authorized archive request: which token, and what else it may do
/// (a route that serves more with [`Scope::People`] asks [`Self::has`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Authorized {
    pub id: String,
    /// The token's name: a note created with it records `source: api:<name>`
    /// (#251).
    pub name: String,
    pub scopes: Vec<Scope>,
}

impl Authorized {
    pub fn has(&self, scope: Scope) -> bool {
        self.scopes.contains(&scope)
    }
}

/// The archive middleware, after the #215 guard: Origin, switch, token,
/// rate limit, scope — in that order.
pub fn authorize(
    enabled: bool,
    tokens: &[ArchiveToken],
    authorization: Option<&str>,
    origin: Option<&str>,
    required: Scope,
    limiter: &RateLimiter,
    now: Instant,
) -> Result<Authorized, Denied> {
    if origin.is_some() {
        return Err(Denied::Origin);
    }
    if !enabled {
        return Err(Denied::Off);
    }
    let given = authorization
        .and_then(auth::bearer_token)
        .ok_or(Denied::Unauthorized)?;
    let token = find(tokens, given).ok_or(Denied::Unauthorized)?;
    if let Err(wait) = limiter.check(&token.id, now) {
        return Err(Denied::RateLimited(
            wait.as_secs_f64().ceil().max(1.0) as u64
        ));
    }
    if !token.scopes.contains(&required) {
        return Err(Denied::Scope(required));
    }
    Ok(Authorized {
        id: token.id.clone(),
        name: token.name.clone(),
        scopes: token.scopes.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const T: &str = "sua_0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    fn now() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-09-25T10:00:00Z")
            .unwrap()
            .with_timezone(&Utc)
    }

    fn make(name: &str, scopes: &[Scope], token: &str, id: &str) -> ArchiveToken {
        create_with(&[], name, scopes, now(), token.into(), id.into())
            .unwrap()
            .0
    }

    fn limiter() -> RateLimiter {
        RateLimiter::default()
    }

    #[test]
    fn tokens_are_prefixed_random_hex() {
        let a = generate_token().unwrap();
        let b = generate_token().unwrap();
        assert!(a.starts_with(TOKEN_PREFIX));
        let hex = &a[TOKEN_PREFIX.len()..];
        assert_eq!(hex.len(), 2 * TOKEN_BYTES);
        assert!(hex.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, b);
    }

    #[test]
    fn only_the_hash_is_stored_and_the_plaintext_is_returned_once() {
        let (stored, new) = create(&[], "  backup script ", &[Scope::Read], now()).unwrap();
        assert_eq!(stored.name, "backup script");
        assert_eq!(stored.sha256, hash_token(&new.token));
        assert_eq!(stored.sha256.len(), 64);
        assert_eq!(stored.id.len(), 16);
        assert_eq!(stored.created, "2026-09-25T10:00:00Z");
        assert_eq!(stored.last_used, None);
        // Nothing stored or listed holds the token.
        let on_disk = serde_json::to_string(&stored).unwrap();
        assert!(!on_disk.contains(&new.token));
        assert!(!on_disk.contains(&new.token[TOKEN_PREFIX.len()..]));
        let listed = serde_json::to_string(&new.info).unwrap();
        assert!(!listed.contains(&new.token) && !listed.contains(&stored.sha256));
        assert_eq!(new.info, TokenInfo::from(&stored));
        // Known vector: SHA-256("abc").
        assert_eq!(
            hash_token("abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn debug_output_never_shows_a_token_or_a_hash() {
        let (stored, new) = create(&[], "ci", &[Scope::Write], now()).unwrap();
        let dbg = format!("{stored:?} {new:?} {:?}", vec![stored.clone()]);
        assert!(!dbg.contains(&new.token), "{dbg}");
        assert!(!dbg.contains(&stored.sha256), "{dbg}");
        assert!(dbg.contains("<redacted>"));
        // Refusals say what's wrong, never echo what was sent.
        for d in [
            Denied::Origin,
            Denied::Off,
            Denied::Unauthorized,
            Denied::Scope(Scope::Write),
            Denied::RateLimited(3),
        ] {
            let text = format!("{} {:?} {:?}", d.message(), d, d.headers());
            assert!(!text.contains(&new.token) && !text.contains(&stored.sha256));
        }
    }

    #[test]
    fn names_and_scopes_are_validated() {
        let existing = vec![make("Backup", &[Scope::Read], T, "a")];
        assert!(validate_name("", &existing).is_err());
        assert!(validate_name("   ", &existing).is_err());
        assert!(
            validate_name("backup", &existing).is_err(),
            "duplicate, any case"
        );
        assert!(validate_name("tab\tname", &existing).is_err());
        assert!(validate_name(&"x".repeat(MAX_NAME_CHARS + 1), &existing).is_err());
        assert_eq!(
            validate_name(&"é".repeat(MAX_NAME_CHARS), &existing)
                .unwrap()
                .chars()
                .count(),
            MAX_NAME_CHARS
        );
        assert!(normalize_scopes(&[]).is_err());
        assert_eq!(
            normalize_scopes(&[Scope::Write, Scope::Read, Scope::Write]).unwrap(),
            vec![Scope::Read, Scope::Write]
        );
        // Scopes (de)serialize lowercase; unknown ones are rejected.
        assert_eq!(serde_json::to_string(&Scope::People).unwrap(), "\"people\"");
        assert!(serde_json::from_str::<Scope>("\"admin\"").is_err());
    }

    #[test]
    fn at_most_max_tokens() {
        let mut all = Vec::new();
        for i in 0..MAX_TOKENS {
            let (t, _) = create(&all, &format!("t{i}"), &[Scope::Read], now()).unwrap();
            all.push(t);
        }
        assert!(create(&all, "one more", &[Scope::Read], now()).is_err());
    }

    #[test]
    fn find_matches_only_the_exact_token() {
        let tokens = vec![
            make("a", &[Scope::Read], T, "id-a"),
            make("b", &[Scope::Write], &T.replace('f', "e"), "id-b"),
        ];
        assert_eq!(find(&tokens, T).map(|t| t.id.as_str()), Some("id-a"));
        assert_eq!(
            find(&tokens, &T.replace('f', "e")).map(|t| t.id.as_str()),
            Some("id-b")
        );
        for wrong in [
            "",
            "sua_",
            &T[..T.len() - 1],
            &T.to_uppercase(),
            &tokens[0].sha256,
        ] {
            assert!(find(&tokens, wrong).is_none(), "{wrong}");
        }
        assert!(find(&[], T).is_none());
    }

    #[test]
    fn revoke_removes_the_token_and_it_stops_working() {
        let mut tokens = vec![make("a", &[Scope::Read], T, "id-a")];
        let lim = limiter();
        let bearer = format!("Bearer {T}");
        assert!(authorize(
            true,
            &tokens,
            Some(&bearer),
            None,
            Scope::Read,
            &lim,
            Instant::now()
        )
        .is_ok());
        assert!(!revoke(&mut tokens, "nope"));
        assert!(revoke(&mut tokens, "id-a"));
        assert!(tokens.is_empty());
        assert_eq!(
            authorize(
                true,
                &tokens,
                Some(&bearer),
                None,
                Scope::Read,
                &lim,
                Instant::now()
            ),
            Err(Denied::Unauthorized)
        );
    }

    #[test]
    fn scopes_are_enforced() {
        let read = "sua_read";
        let write = "sua_write";
        let both = "sua_both";
        let tokens = vec![
            make("r", &[Scope::Read], read, "r"),
            make("w", &[Scope::Write], write, "w"),
            make(
                "rpw",
                &[Scope::Read, Scope::People, Scope::Write],
                both,
                "rpw",
            ),
        ];
        let lim = limiter();
        let go = |token: &str, method: &str| {
            authorize(
                true,
                &tokens,
                Some(&format!("Bearer {token}")),
                None,
                required_scope(method),
                &lim,
                Instant::now(),
            )
        };
        assert!(go(read, "GET").is_ok());
        assert_eq!(go(read, "POST"), Err(Denied::Scope(Scope::Write)));
        assert_eq!(go(read, "DELETE"), Err(Denied::Scope(Scope::Write)));
        assert!(go(write, "POST").is_ok());
        assert_eq!(go(write, "GET"), Err(Denied::Scope(Scope::Read)));
        let a = go(both, "HEAD").unwrap();
        assert!(a.has(Scope::People) && a.has(Scope::Write));
        assert!(!go(read, "GET").unwrap().has(Scope::People));
        assert_eq!(Denied::Scope(Scope::Write).status(), 403);
        assert!(Denied::Scope(Scope::Write).headers()[0]
            .1
            .contains("scope=\"write\""));
    }

    #[test]
    fn any_origin_is_refused_before_the_token_is_looked_at() {
        let tokens = vec![make("a", &[Scope::Read], T, "a")];
        let lim = limiter();
        let bearer = format!("Bearer {T}");
        for origin in [
            "chrome-extension://abcdefghijklmnopabcdefghijklmnop",
            "moz-extension://2d6b6c1e-3f7a-4b1e-9d0a-1c2b3d4e5f60",
            "https://evil.example",
            "http://127.0.0.1:4525",
            "null",
            "",
        ] {
            assert_eq!(
                authorize(
                    true,
                    &tokens,
                    Some(&bearer),
                    Some(origin),
                    Scope::Read,
                    &lim,
                    Instant::now()
                ),
                Err(Denied::Origin),
                "{origin}"
            );
            // Refused even when off, so a page learns nothing either way.
            assert_eq!(
                authorize(
                    false,
                    &tokens,
                    Some(&bearer),
                    Some(origin),
                    Scope::Read,
                    &lim,
                    Instant::now()
                ),
                Err(Denied::Origin)
            );
        }
        assert!(Denied::Origin.headers().is_empty(), "no CORS");
    }

    #[test]
    fn off_missing_and_wrong_tokens_are_refused() {
        let tokens = vec![make("a", &[Scope::Read], T, "a")];
        let lim = limiter();
        let bearer = format!("Bearer {T}");
        let at = Instant::now();
        assert_eq!(
            authorize(false, &tokens, Some(&bearer), None, Scope::Read, &lim, at),
            Err(Denied::Off)
        );
        for auth in [
            None,
            Some("Bearer "),
            Some("Basic abc"),
            Some("Bearer sua_nope"),
            Some(T),
        ] {
            assert_eq!(
                authorize(true, &tokens, auth, None, Scope::Read, &lim, at),
                Err(Denied::Unauthorized),
                "{auth:?}"
            );
        }
        // No tokens at all: nothing matches, not even an empty one.
        assert_eq!(
            authorize(true, &[], Some("Bearer x"), None, Scope::Read, &lim, at),
            Err(Denied::Unauthorized)
        );
        assert_eq!(Denied::Unauthorized.status(), 401);
        assert!(Denied::Unauthorized.headers()[0].0 == "WWW-Authenticate");
    }

    #[test]
    fn the_rate_limit_is_per_token_with_a_burst_then_a_steady_rate() {
        let lim = RateLimiter::new(60.0, 10.0);
        let t0 = Instant::now();
        for i in 0..60 {
            assert!(lim.check("a", t0).is_ok(), "burst request {i}");
        }
        let wait = lim.check("a", t0).unwrap_err();
        assert!(
            wait > Duration::ZERO && wait <= Duration::from_millis(100),
            "{wait:?}"
        );
        // Another token has its own bucket.
        assert!(lim.check("b", t0).is_ok());
        // 10/s: after 100 ms one more, after 1 s ten more.
        assert!(lim.check("a", t0 + Duration::from_millis(100)).is_ok());
        assert!(lim.check("a", t0 + Duration::from_millis(100)).is_err());
        let t1 = t0 + Duration::from_millis(1100);
        for _ in 0..10 {
            assert!(lim.check("a", t1).is_ok());
        }
        assert!(lim.check("a", t1).is_err());
        // Idle long enough: the whole burst again, never more.
        let t2 = t1 + Duration::from_secs(3600);
        for _ in 0..60 {
            assert!(lim.check("a", t2).is_ok());
        }
        assert!(lim.check("a", t2).is_err());
    }

    #[test]
    fn a_rate_limited_token_gets_429_with_retry_after() {
        let tokens = vec![make("a", &[Scope::Read], T, "a")];
        let lim = RateLimiter::new(2.0, 0.5);
        let bearer = format!("Bearer {T}");
        let at = Instant::now();
        for _ in 0..2 {
            assert!(authorize(true, &tokens, Some(&bearer), None, Scope::Read, &lim, at).is_ok());
        }
        let d = authorize(true, &tokens, Some(&bearer), None, Scope::Read, &lim, at).unwrap_err();
        assert_eq!(d, Denied::RateLimited(2));
        assert_eq!(d.status(), 429);
        assert_eq!(d.headers(), vec![("Retry-After", "2".to_string())]);
        // A wrong token never touches the limiter (and is never limited).
        for _ in 0..10 {
            assert_eq!(
                authorize(
                    true,
                    &tokens,
                    Some("Bearer sua_x"),
                    None,
                    Scope::Read,
                    &lim,
                    at
                ),
                Err(Denied::Unauthorized)
            );
        }
    }

    #[test]
    fn last_used_moves_at_most_once_a_minute() {
        let mut tokens = vec![make("a", &[Scope::Read], T, "a")];
        assert!(!touch(&mut tokens, "nope", now()));
        assert!(touch(&mut tokens, "a", now()));
        assert_eq!(tokens[0].last_used.as_deref(), Some("2026-09-25T10:00:00Z"));
        assert!(!touch(
            &mut tokens,
            "a",
            now() + chrono::TimeDelta::seconds(59)
        ));
        assert!(touch(
            &mut tokens,
            "a",
            now() + chrono::TimeDelta::seconds(60)
        ));
        assert_eq!(tokens[0].last_used.as_deref(), Some("2026-09-25T10:01:00Z"));
        // An unreadable value is replaced.
        tokens[0].last_used = Some("garbage".into());
        assert!(touch(&mut tokens, "a", now()));
    }

    #[test]
    fn required_scope_by_method() {
        assert_eq!(required_scope("GET"), Scope::Read);
        assert_eq!(required_scope("head"), Scope::Read);
        for m in ["POST", "PUT", "PATCH", "DELETE", "OPTIONS"] {
            assert_eq!(required_scope(m), Scope::Write, "{m}");
        }
    }
}
