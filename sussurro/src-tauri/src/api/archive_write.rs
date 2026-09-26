//! The archive's one write route (#251; P15, E14; plan §4.2):
//! `POST /archive/items` creates a **note** from text — the clipboard or a
//! shortcut → note automation. Everything else in the archive API reads.
//!
//! It runs after the archive middleware ([`super::tokens::authorize`]): no
//! browser `Origin`, `api_archive` on, a matching token within its general
//! rate limit, and the `write` scope. Then, in this order:
//!
//! 1. the body: at most [`MAX_BODY_BYTES`] (the declared length is checked
//!    before reading, like `/clean`), `Content-Type: application/json` (or
//!    none), UTF-8 JSON — `{"text", "title"?, "tags"?, "categories"?,
//!    "cleanup"?}`, unknown fields refused ([`parse_note`]);
//! 2. `Idempotency-Key` (optional): the same key with the same body within
//!    [`IDEMPOTENCY_TTL`] answers the item it created before, without
//!    creating another; with another body it is a 422 ([`Idempotency`]);
//! 3. with `cleanup: true`, the cleanup gate ([`cleanup_gate`]): cleanup
//!    runs **only on a local cleanup profile**. An external profile is
//!    refused even when the user opted in to it for dictation: that
//!    consent (#122) is given in the app for the app's own sends, and a
//!    script can't give it — so the API never sends text off this machine;
//! 4. the creation rate limit ([`CreateLimits`]: a burst, then a steady
//!    rate, per token and for all tokens together), on top of the general
//!    per-token limit, so a runaway script can't fill the archive;
//! 5. the note: `type: note`, `source: api:<token name>`, the date now, no
//!    audio, no speakers, no participants — one segment per paragraph (raw
//!    = the text as sent, text = cleaned), through [`crate::archive::create_item`];
//!    then indexed.
//!
//! The answer is `201 Created` with `Location: /archive/items/<id>` and the
//! item's id, location and frontmatter summary. Nothing the caller sent is
//! echoed in an error, and internal errors are a fixed message.

use super::archive::{Body, Reply};
use super::tokens::{Authorized, RateLimiter};
use crate::archive::{self, ItemMeta, ItemType, Segment, SegmentsFile};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Largest request body: the same cap as `/clean` (P15: 1 MiB).
pub const MAX_BODY_BYTES: usize = super::guard::CLEAN_MAX_BYTES;
/// Longest title, in characters.
pub const MAX_TITLE_CHARS: usize = 200;
/// Most tags, and most categories.
pub const MAX_LABELS: usize = 50;
/// Longest tag or category, in characters.
pub const MAX_LABEL_CHARS: usize = 100;
/// Most text cleanup takes (one LLM call per paragraph, while the request
/// waits): longer notes can be created without cleanup.
pub const MAX_CLEANUP_BYTES: usize = 64 << 10;
/// Notes one token may create in a burst…
pub const CREATE_BURST: f64 = 10.0;
/// …then this many per second (30 a minute).
pub const CREATE_PER_SEC: f64 = 0.5;
/// Notes all tokens together may create in a burst…
pub const CREATE_ALL_BURST: f64 = 30.0;
/// …then this many per second (60 a minute).
pub const CREATE_ALL_PER_SEC: f64 = 1.0;
/// How long an `Idempotency-Key` is remembered (in memory: a restart of
/// the app forgets it).
pub const IDEMPOTENCY_TTL: Duration = Duration::from_secs(60 * 60);
/// Longest `Idempotency-Key`.
pub const MAX_IDEMPOTENCY_KEY: usize = 255;
/// Keys remembered at most (the oldest finished one goes first).
pub const MAX_IDEMPOTENCY_ENTRIES: usize = 1000;

/// The fields a body may have.
const FIELDS: [&str; 5] = ["title", "text", "tags", "categories", "cleanup"];

/// Pure: `POST /archive/items` (the one write route).
pub fn is_create(method: &str, path: &str) -> bool {
    method.eq_ignore_ascii_case("POST") && matches!(path, "/archive/items" | "/archive/items/")
}

/// A note request, validated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewNote {
    /// Empty: taken from the first words of the text.
    pub title: String,
    /// One entry per paragraph (text split on blank lines), newlines kept
    /// inside a paragraph.
    pub paragraphs: Vec<String>,
    pub tags: Vec<String>,
    pub categories: Vec<String>,
    pub cleanup: bool,
}

fn bad(message: &str) -> Reply {
    Reply::error(400, "bad_request", message)
}

/// Pure: whether `Content-Type` allows a JSON body: absent, or
/// `application/json` with no charset but UTF-8.
pub fn json_content_type(content_type: Option<&str>) -> bool {
    let Some(ct) = content_type else { return true };
    let mut parts = ct.split(';').map(str::trim);
    if !parts
        .next()
        .is_some_and(|m| m.eq_ignore_ascii_case("application/json"))
    {
        return false;
    }
    parts.all(|p| match p.split_once('=') {
        Some((k, v)) if k.trim().eq_ignore_ascii_case("charset") => {
            v.trim().trim_matches('"').eq_ignore_ascii_case("utf-8")
        }
        _ => true,
    })
}

/// Pure: line endings to `\n`, and control characters other than newline
/// and tab removed (a stray one from a clipboard shouldn't refuse a note).
fn clean_text(text: &str) -> String {
    text.replace("\r\n", "\n")
        .replace('\r', "\n")
        .chars()
        .filter(|c| *c == '\n' || *c == '\t' || !c.is_control())
        .collect()
}

/// Pure: the text's paragraphs — split on blank lines, each trimmed, empty
/// ones dropped.
pub fn paragraphs(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current: Vec<&str> = Vec::new();
    let mut flush = |current: &mut Vec<&str>| {
        let p = current.join("\n");
        let p = p.trim();
        if !p.is_empty() {
            out.push(p.to_string());
        }
        current.clear();
    };
    for line in text.split('\n') {
        if line.trim().is_empty() {
            flush(&mut current);
        } else {
            current.push(line.trim_end());
        }
    }
    flush(&mut current);
    out
}

/// A list of labels (`tags`, `categories`): strings, trimmed, empty ones
/// dropped, repeats (ignoring case) kept once.
fn labels(v: &Value, key: &str) -> Result<Vec<String>, Reply> {
    let list = match v {
        Value::Null => return Ok(Vec::new()),
        Value::Array(list) => list,
        _ => return Err(bad(&format!("`{key}` must be a list of strings"))),
    };
    if list.len() > MAX_LABELS {
        return Err(bad(&format!("too many `{key}` (at most {MAX_LABELS})")));
    }
    let mut out: Vec<String> = Vec::new();
    for item in list {
        let Value::String(s) = item else {
            return Err(bad(&format!("`{key}` must be a list of strings")));
        };
        let s = s.trim();
        if s.chars().count() > MAX_LABEL_CHARS {
            return Err(bad(&format!(
                "each of `{key}` may have at most {MAX_LABEL_CHARS} characters"
            )));
        }
        if s.chars().any(char::is_control) {
            return Err(bad(&format!("`{key}` can't contain control characters")));
        }
        if !s.is_empty() && !out.iter().any(|o| o.to_lowercase() == s.to_lowercase()) {
            out.push(s.to_string());
        }
    }
    Ok(out)
}

/// Pure: the body of `POST /archive/items`, validated. Field names in
/// errors are ours; nothing the caller sent is echoed.
pub fn parse_note(body: &[u8]) -> Result<NewNote, Reply> {
    let invalid = || Reply::error(400, "invalid_json", "the body must be a UTF-8 JSON object");
    let text = std::str::from_utf8(body).map_err(|_| invalid())?;
    let Ok(Value::Object(map)) = serde_json::from_str::<Value>(text) else {
        return Err(invalid());
    };
    if map.keys().any(|k| !FIELDS.contains(&k.as_str())) {
        return Err(bad(&format!(
            "unknown field; a note accepts: {}",
            FIELDS.join(", ")
        )));
    }
    let text = match map.get("text") {
        Some(Value::String(s)) => clean_text(s),
        None | Some(Value::Null) => return Err(bad("`text` is required")),
        Some(_) => return Err(bad("`text` must be a string")),
    };
    let paragraphs = paragraphs(&text);
    if paragraphs.is_empty() {
        return Err(bad("`text` is empty"));
    }
    let title = match map.get("title") {
        None | Some(Value::Null) => String::new(),
        Some(Value::String(s)) => {
            if s.chars().any(|c| c.is_control() && !c.is_whitespace()) {
                return Err(bad("`title` can't contain control characters"));
            }
            let t = s.split_whitespace().collect::<Vec<_>>().join(" ");
            if t.chars().count() > MAX_TITLE_CHARS {
                return Err(bad(&format!(
                    "`title` is too long (at most {MAX_TITLE_CHARS} characters)"
                )));
            }
            t
        }
        Some(_) => return Err(bad("`title` must be a string")),
    };
    let tags = labels(map.get("tags").unwrap_or(&Value::Null), "tags")?;
    let categories = labels(map.get("categories").unwrap_or(&Value::Null), "categories")?;
    let cleanup = match map.get("cleanup") {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(_) => return Err(bad("`cleanup` must be true or false")),
    };
    Ok(NewNote {
        title,
        paragraphs,
        tags,
        categories,
        cleanup,
    })
}

/// Pure: an `Idempotency-Key` header is 1–[`MAX_IDEMPOTENCY_KEY`] visible
/// ASCII characters (a UUID fits).
pub fn valid_idempotency_key(key: &str) -> bool {
    (1..=MAX_IDEMPOTENCY_KEY).contains(&key.len()) && key.bytes().all(|b| b.is_ascii_graphic())
}

/// Why `cleanup: true` can't run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CleanupRefused {
    /// Cleanup is off in Settings (level None, no translation), or this
    /// host has none.
    Off,
    /// The cleanup profile is external: never through the API, opted in or
    /// not.
    External,
}

impl CleanupRefused {
    fn reply(self) -> Reply {
        match self {
            CleanupRefused::Off => Reply::error(
                409,
                "cleanup_off",
                "cleanup is off in Sussurro → Settings → Cleanup: create the note without `cleanup`",
            ),
            CleanupRefused::External => Reply::error(
                409,
                "cleanup_external",
                "the cleanup profile sends text off this computer, and the archive API never does: \
                 create the note without `cleanup`, or pick a local cleanup profile in Settings",
            ),
        }
    }
}

/// Pure: whether a note may be cleaned. `active`: cleanup does something
/// (a level, or a translation); `external`: the cleanup profile's server is
/// off this machine — refused whatever the profile's cleanup opt-in says.
pub fn cleanup_gate(active: bool, external: bool) -> Result<(), CleanupRefused> {
    if external {
        return Err(CleanupRefused::External);
    }
    if !active {
        return Err(CleanupRefused::Off);
    }
    Ok(())
}

/// What `POST /archive/items` needs from the app besides the archive:
/// cleanup, and telling the UI. Every [`super::Host`] is one; the defaults
/// (no cleanup, nobody to tell) suit hosts that don't create notes.
pub trait NoteHost {
    /// [`cleanup_gate`] on the current settings, without cleaning.
    fn note_cleanup_gate(&self) -> Result<(), CleanupRefused> {
        Err(CleanupRefused::Off)
    }
    /// Clean each paragraph (with the previous one as context) — after
    /// checking [`cleanup_gate`] again on the same settings it cleans with.
    fn clean_note(&self, _paragraphs: &[String]) -> Result<Vec<String>, CleanupRefused> {
        Err(CleanupRefused::Off)
    }
    /// A note was created: the Library refreshes.
    fn note_created(&self, _id: &str) {}
}

/// The creation rate limits (per token, and for all tokens together).
#[derive(Debug)]
pub struct CreateLimits {
    per_token: RateLimiter,
    all: RateLimiter,
}

impl Default for CreateLimits {
    fn default() -> Self {
        Self::new(
            RateLimiter::new(CREATE_BURST, CREATE_PER_SEC),
            RateLimiter::new(CREATE_ALL_BURST, CREATE_ALL_PER_SEC),
        )
    }
}

impl CreateLimits {
    pub fn new(per_token: RateLimiter, all: RateLimiter) -> Self {
        Self { per_token, all }
    }

    /// Take one creation for `token` at `now`; `Err(seconds to wait)`.
    fn check(&self, token: &str, now: Instant) -> Result<(), u64> {
        let secs = |d: Duration| d.as_secs_f64().ceil().max(1.0) as u64;
        self.per_token.check(token, now).map_err(secs)?;
        self.all.check("*", now).map_err(secs)
    }
}

#[derive(Debug, Clone)]
enum Outcome {
    /// Being created by another request right now.
    Pending,
    /// Created: the answer's body.
    Done(Value),
}

#[derive(Debug, Clone)]
struct Entry {
    /// SHA-256 of the body the key was first used with.
    body_sha256: String,
    at: Instant,
    outcome: Outcome,
}

/// What an `Idempotency-Key` says about a request.
#[derive(Debug, Clone, PartialEq)]
pub enum Claim {
    /// New: this request creates the note (and must [`Idempotency::finish`]
    /// or [`Idempotency::release`]).
    New,
    /// Seen with this body: the earlier answer.
    Replay(Value),
    /// Seen with this body, still being created.
    InProgress,
    /// Seen with another body.
    Mismatch,
}

/// Idempotency keys, per token (two scripts can use the same key), kept
/// [`IDEMPOTENCY_TTL`] in memory.
#[derive(Debug, Default)]
pub struct Idempotency {
    entries: Mutex<HashMap<(String, String), Entry>>,
}

impl Idempotency {
    pub fn claim(&self, token: &str, key: &str, body: &[u8], now: Instant) -> Claim {
        let mut entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        entries.retain(|_, e| now.saturating_duration_since(e.at) < IDEMPOTENCY_TTL);
        let body_sha256 = archive::store::sha256_hex(body);
        let k = (token.to_string(), key.to_string());
        if let Some(e) = entries.get(&k) {
            if e.body_sha256 != body_sha256 {
                return Claim::Mismatch;
            }
            return match &e.outcome {
                Outcome::Pending => Claim::InProgress,
                Outcome::Done(v) => Claim::Replay(v.clone()),
            };
        }
        if entries.len() >= MAX_IDEMPOTENCY_ENTRIES {
            let oldest = entries
                .iter()
                .filter(|(_, e)| matches!(e.outcome, Outcome::Done(_)))
                .min_by_key(|(_, e)| e.at)
                .map(|(k, _)| k.clone());
            if let Some(o) = oldest {
                entries.remove(&o);
            }
        }
        entries.insert(
            k,
            Entry {
                body_sha256,
                at: now,
                outcome: Outcome::Pending,
            },
        );
        Claim::New
    }

    /// The note was created: later requests with the key get `answer`.
    pub fn finish(&self, token: &str, key: &str, answer: Value) {
        let mut entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(e) = entries.get_mut(&(token.to_string(), key.to_string())) {
            e.outcome = Outcome::Done(answer);
        }
    }

    /// Nothing was created: the key may be used again.
    pub fn release(&self, token: &str, key: &str) {
        let mut entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        entries.remove(&(token.to_string(), key.to_string()));
    }
}

/// The state the write route keeps between requests.
#[derive(Debug, Default)]
pub struct WriteState {
    pub limits: CreateLimits,
    pub idempotency: Idempotency,
}

/// The API path of an item.
pub fn location(id: &str) -> String {
    format!("/archive/items/{id}")
}

fn created(answer: Value, id: &str, replayed: bool) -> Reply {
    let mut body = answer;
    body["replayed"] = json!(replayed);
    let mut headers = vec![("Location", location(id))];
    if replayed {
        headers.push(("Idempotent-Replayed", "true".to_string()));
    }
    Reply {
        status: 201,
        body: Body::Json(body),
        headers,
    }
}

/// The note's frontmatter and segments (pure). `cleaned[i]` is paragraph
/// `i` after cleanup (the paragraph itself without cleanup).
pub fn build_note(
    note: &NewNote,
    cleaned: &[String],
    token_name: &str,
    date: &str,
) -> (ItemMeta, SegmentsFile) {
    let segments: Vec<Segment> = note
        .paragraphs
        .iter()
        .zip(cleaned)
        .enumerate()
        .map(|(i, (raw, text))| {
            // No audio: times only space the paragraphs apart, so the
            // transcript renders one paragraph per segment.
            let at = i as u64 * archive::render::PARAGRAPH_GAP_MS;
            Segment {
                id: i as u32,
                start_ms: at,
                end_ms: at,
                raw: raw.clone(),
                text: text.trim().to_string(),
                ..Default::default()
            }
        })
        .collect();
    let title = if note.title.is_empty() {
        let first = cleaned
            .iter()
            .map(|t| t.trim())
            .find(|t| !t.is_empty())
            .unwrap_or("");
        crate::engine::title_from_text(first)
    } else {
        note.title.clone()
    };
    let meta = ItemMeta {
        item_type: ItemType::Note,
        title,
        date: date.to_string(),
        source: format!("api:{token_name}"),
        tags: note.tags.clone(),
        categories: note.categories.clone(),
        ..Default::default()
    };
    (
        meta,
        SegmentsFile {
            segments,
            ..Default::default()
        },
    )
}

/// `POST /archive/items`, for an authorized request whose body was read
/// (at most [`MAX_BODY_BYTES`]). `index` is the search index file.
#[allow(clippy::too_many_arguments)]
pub fn create(
    host: &dyn NoteHost,
    archive_dir: &Path,
    index: &Path,
    auth: &Authorized,
    body: &[u8],
    content_type: Option<&str>,
    idempotency_key: Option<&str>,
    state: &WriteState,
    now: Instant,
) -> Reply {
    if !json_content_type(content_type) {
        return Reply::error(
            415,
            "unsupported_media_type",
            "send the note as Content-Type: application/json (UTF-8)",
        );
    }
    let key = match idempotency_key.map(str::trim) {
        None => None,
        Some(k) if valid_idempotency_key(k) => Some(k),
        Some(_) => {
            return Reply::error(
                400,
                "invalid_idempotency_key",
                &format!(
                    "`Idempotency-Key` must be 1 to {MAX_IDEMPOTENCY_KEY} visible ASCII characters"
                ),
            )
        }
    };
    let note = match parse_note(body) {
        Ok(n) => n,
        Err(r) => return r,
    };
    if let Some(k) = key {
        match state.idempotency.claim(&auth.id, k, body, now) {
            Claim::New => {}
            Claim::Replay(answer) => {
                let id = answer["id"].as_str().unwrap_or_default().to_string();
                return created(answer, &id, true);
            }
            Claim::InProgress => {
                let mut r = Reply::error(
                    409,
                    "idempotency_in_progress",
                    "a request with this Idempotency-Key is still being handled: retry shortly",
                );
                r.headers.push(("Retry-After", "1".to_string()));
                return r;
            }
            Claim::Mismatch => return Reply::error(
                422,
                "idempotency_mismatch",
                "this Idempotency-Key was used with a different body: use a new key for a new note",
            ),
        }
    }
    let reply = create_claimed(host, archive_dir, index, auth, &note, state, now);
    if let Some(k) = key {
        match &reply.body {
            Body::Json(answer) if reply.status == 201 => {
                state.idempotency.finish(&auth.id, k, answer.clone())
            }
            _ => state.idempotency.release(&auth.id, k),
        }
    }
    reply
}

fn create_claimed(
    host: &dyn NoteHost,
    archive_dir: &Path,
    index: &Path,
    auth: &Authorized,
    note: &NewNote,
    state: &WriteState,
    now: Instant,
) -> Reply {
    // Refusals that depend only on the request and the settings come
    // before the rate limit, so they cost no creation.
    if note.cleanup {
        let bytes: usize = note.paragraphs.iter().map(String::len).sum();
        if bytes > MAX_CLEANUP_BYTES {
            return Reply::error(
                413,
                "too_long_for_cleanup",
                &format!(
                    "cleanup takes at most {} KiB of text: create a longer note without `cleanup`",
                    MAX_CLEANUP_BYTES >> 10
                ),
            );
        }
        if let Err(refused) = host.note_cleanup_gate() {
            return refused.reply();
        }
    }
    if let Err(secs) = state.limits.check(&auth.id, now) {
        let mut r = Reply::error(429, "rate_limited", "too many notes created: slow down");
        r.headers.push(("Retry-After", secs.to_string()));
        return r;
    }
    let cleaned = if note.cleanup {
        match host.clean_note(&note.paragraphs) {
            Ok(c) if c.len() == note.paragraphs.len() => c,
            Ok(_) => return Reply::internal(),
            Err(refused) => return refused.reply(),
        }
    } else {
        note.paragraphs.clone()
    };
    let date = chrono::Local::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, false);
    let (meta, segments) = build_note(note, &cleaned, &auth.name, &date);
    let Ok(id) = archive::create_item(archive_dir, &meta, &segments) else {
        return Reply::error(
            500,
            "internal",
            "the note could not be written to the archive",
        );
    };
    // Best effort: a failed index update is caught up by the next sync.
    if let Err(e) = archive::Index::open(archive_dir, index).and_then(|mut idx| idx.index_item(&id))
    {
        eprintln!("archive index: update failed ({e:#})");
    }
    host.note_created(&id);
    let answer = json!({
        "id": id,
        "location": location(&id),
        "type": meta.item_type.as_str(),
        "title": meta.title,
        "date": meta.date,
        "source": meta.source,
        "tags": meta.tags,
        "categories": meta.categories,
        "paragraphs": segments.segments.len(),
        "cleaned": note.cleanup && cleaned != note.paragraphs,
    });
    created(answer, &id, false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::tokens::Scope;
    use std::cell::RefCell;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A host whose cleanup uppercases and whose gate is set by the test.
    struct Fake {
        gate: RefCell<Result<(), CleanupRefused>>,
        /// `clean_note` answers this instead (the settings changed between
        /// the gate and the cleanup).
        clean_refused: RefCell<Option<CleanupRefused>>,
        cleaned: AtomicUsize,
        created: RefCell<Vec<String>>,
    }

    impl Fake {
        fn new() -> Self {
            Fake {
                gate: RefCell::new(Ok(())),
                clean_refused: RefCell::new(None),
                cleaned: AtomicUsize::new(0),
                created: RefCell::new(Vec::new()),
            }
        }
    }

    impl NoteHost for Fake {
        fn note_cleanup_gate(&self) -> Result<(), CleanupRefused> {
            *self.gate.borrow()
        }
        fn clean_note(&self, paragraphs: &[String]) -> Result<Vec<String>, CleanupRefused> {
            if let Some(r) = *self.clean_refused.borrow() {
                return Err(r);
            }
            self.cleaned.fetch_add(paragraphs.len(), Ordering::SeqCst);
            Ok(paragraphs.iter().map(|p| p.to_uppercase()).collect())
        }
        fn note_created(&self, id: &str) {
            self.created.borrow_mut().push(id.to_string());
        }
    }

    struct Fx {
        _tmp: tempfile::TempDir,
        archive: PathBuf,
        index: PathBuf,
        host: Fake,
        state: WriteState,
    }

    fn fixture() -> Fx {
        let tmp = tempfile::tempdir().unwrap();
        let archive = tmp.path().join("Sussurro");
        let index = tmp.path().join("appdata").join(archive::INDEX_FILE);
        std::fs::create_dir_all(index.parent().unwrap()).unwrap();
        Fx {
            _tmp: tmp,
            archive,
            index,
            host: Fake::new(),
            state: WriteState::default(),
        }
    }

    fn writer() -> Authorized {
        Authorized {
            id: "w1".into(),
            name: "Shortcuts".into(),
            scopes: vec![Scope::Write],
        }
    }

    fn post(f: &Fx, auth: &Authorized, body: &str, key: Option<&str>, at: Instant) -> Reply {
        create(
            &f.host,
            &f.archive,
            &f.index,
            auth,
            body.as_bytes(),
            Some("application/json"),
            key,
            &f.state,
            at,
        )
    }

    fn json_of(r: &Reply) -> Value {
        match &r.body {
            Body::Json(v) => v.clone(),
            Body::File { .. } => panic!("expected JSON"),
        }
    }

    fn code(r: &Reply) -> String {
        json_of(r)["code"].as_str().unwrap_or_default().to_string()
    }

    fn header<'a>(r: &'a Reply, name: &str) -> Option<&'a str> {
        r.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    fn items(f: &Fx) -> usize {
        archive::list_items(&f.archive).len()
    }

    #[test]
    fn only_post_to_the_items_collection_creates() {
        assert!(is_create("POST", "/archive/items"));
        assert!(is_create("post", "/archive/items/"));
        for (m, p) in [
            ("GET", "/archive/items"),
            ("PUT", "/archive/items"),
            ("POST", "/archive/items/2026/09/x"),
            ("POST", "/archive/people"),
            ("POST", "/archive"),
        ] {
            assert!(!is_create(m, p), "{m} {p}");
        }
    }

    #[test]
    fn a_note_is_created_with_its_frontmatter_and_is_readable_and_searchable() {
        let f = fixture();
        let body = json!({
            "title": "  Idea   for\tthe release ",
            "text": "First paragraph,\r\nsecond line.\r\n\r\n\r\n  Second paragraph. \n",
            "tags": ["idea", " Release ", "IDEA", ""],
            "categories": ["work"],
        })
        .to_string();
        let r = post(&f, &writer(), &body, None, Instant::now());
        assert_eq!(r.status, 201, "{:?}", r.body);
        let v = json_of(&r);
        let id = v["id"].as_str().unwrap().to_string();
        assert!(id.ends_with("-idea-for-the-release"), "{id}");
        assert_eq!(
            header(&r, "Location"),
            Some(format!("/archive/items/{id}").as_str())
        );
        assert_eq!(v["location"], location(&id));
        assert_eq!(v["replayed"], false);
        assert_eq!(v["cleaned"], false);
        assert_eq!(v["paragraphs"], 2);
        assert_eq!(*f.host.created.borrow(), vec![id.clone()]);

        let item = archive::read_item(&f.archive, &id).unwrap();
        assert_eq!(item.meta.item_type, ItemType::Note);
        assert_eq!(item.meta.title, "Idea for the release");
        assert_eq!(item.meta.source, "api:Shortcuts");
        assert_eq!(item.meta.tags, vec!["idea", "Release"]);
        assert_eq!(item.meta.categories, vec!["work"]);
        assert!(item.meta.participants.is_empty() && item.meta.duration.is_none());
        assert!(
            item.meta.extra.is_empty(),
            "no audio, no status: {:?}",
            item.meta.extra
        );
        let date = chrono::DateTime::parse_from_rfc3339(&item.meta.date).unwrap();
        assert!(
            (chrono::Utc::now() - date.with_timezone(&chrono::Utc))
                .num_seconds()
                .abs()
                < 60
        );
        assert!(item.segments.speakers.is_empty());
        assert_eq!(item.segments.segments.len(), 2);
        assert_eq!(
            item.segments.segments[0].text,
            "First paragraph,\nsecond line."
        );
        assert_eq!(
            item.body,
            "\n# Idea for the release\n\nFirst paragraph,\nsecond line.\n\nSecond paragraph.\n"
        );
        assert!(!item.edited_externally);
        let dir = f.archive.join(&id);
        let files: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert!(
            files
                .iter()
                .all(|n| !n.ends_with(".wav") && !n.ends_with(".opus")),
            "{files:?}"
        );

        // Through the read routes: listed (indexed) and readable.
        let read = Authorized {
            id: "r".into(),
            name: "reader".into(),
            scopes: vec![Scope::Read],
        };
        let found = super::super::archive::handle(
            &f.archive,
            &f.index,
            &read,
            "GET",
            "/archive/items?q=paragraph",
        );
        assert_eq!(json_of(&found)["items"][0]["id"], id.as_str());
        let got = super::super::archive::handle(&f.archive, &f.index, &read, "GET", &location(&id));
        assert_eq!(got.status, 200);
        assert_eq!(json_of(&got)["meta"]["source"], "api:Shortcuts");
    }

    #[test]
    fn a_missing_title_comes_from_the_text() {
        let f = fixture();
        let r = post(
            &f,
            &writer(),
            r#"{"text": "Comprare il caffè e il latte domani mattina presto, prima delle otto."}"#,
            None,
            Instant::now(),
        );
        assert_eq!(r.status, 201);
        assert_eq!(
            json_of(&r)["title"],
            "Comprare il caffè e il latte domani mattina"
        );
        let r = post(
            &f,
            &writer(),
            r#"{"title": "   ", "text": "Short."}"#,
            None,
            Instant::now(),
        );
        assert_eq!(json_of(&r)["title"], "Short");
    }

    #[test]
    fn invalid_bodies_are_refused_without_echoing_them() {
        let f = fixture();
        let long_title = format!(
            r#"{{"text":"x","title":"{}"}}"#,
            "t".repeat(MAX_TITLE_CHARS + 1)
        );
        let many_tags = json!({"text": "x", "tags": (0..=MAX_LABELS).map(|i| format!("t{i}")).collect::<Vec<_>>()}).to_string();
        let long_tag = json!({"text": "x", "tags": ["t".repeat(MAX_LABEL_CHARS + 1)]}).to_string();
        for (body, want) in [
            ("", "invalid_json"),
            ("not json", "invalid_json"),
            ("[1, 2]", "invalid_json"),
            ("\"text\"", "invalid_json"),
            (
                r#"{"text": "secret-probe", "path": "/etc/passwd"}"#,
                "bad_request",
            ),
            (r#"{"title": "x"}"#, "bad_request"),
            (r#"{"text": null}"#, "bad_request"),
            (r#"{"text": 42}"#, "bad_request"),
            (r#"{"text": "  \n\n \t "}"#, "bad_request"),
            (r#"{"text": "x", "title": 3}"#, "bad_request"),
            (r#"{"text": "x", "title": "a\u0000b"}"#, "bad_request"),
            (r#"{"text": "x", "tags": "one"}"#, "bad_request"),
            (r#"{"text": "x", "tags": [1]}"#, "bad_request"),
            (r#"{"text": "x", "categories": ["a\u0007"]}"#, "bad_request"),
            (r#"{"text": "x", "cleanup": "yes"}"#, "bad_request"),
            (&long_title, "bad_request"),
            (&many_tags, "bad_request"),
            (&long_tag, "bad_request"),
        ] {
            let r = post(&f, &writer(), body, None, Instant::now());
            assert_eq!((r.status, code(&r).as_str()), (400, want), "{body}");
            let text = json_of(&r).to_string();
            assert!(
                !text.contains("/etc/passwd") && !text.contains("secret-probe"),
                "{text}"
            );
        }
        // Not UTF-8.
        let r = create(
            &f.host,
            &f.archive,
            &f.index,
            &writer(),
            b"{\"text\": \"\xff\"}",
            None,
            None,
            &f.state,
            Instant::now(),
        );
        assert_eq!((r.status, code(&r).as_str()), (400, "invalid_json"));
        // Other content types.
        for ct in [
            "text/plain",
            "application/x-www-form-urlencoded",
            "application/json; charset=latin-1",
        ] {
            let r = create(
                &f.host,
                &f.archive,
                &f.index,
                &writer(),
                br#"{"text":"x"}"#,
                Some(ct),
                None,
                &f.state,
                Instant::now(),
            );
            assert_eq!(
                (r.status, code(&r).as_str()),
                (415, "unsupported_media_type"),
                "{ct}"
            );
        }
        assert_eq!(items(&f), 0, "nothing created");
        // Accepted forms.
        for ct in [
            None,
            Some("application/json"),
            Some("Application/JSON; charset=UTF-8"),
        ] {
            let r = create(
                &f.host,
                &f.archive,
                &f.index,
                &writer(),
                br#"{"text":"x","tags":null}"#,
                ct,
                None,
                &f.state,
                Instant::now(),
            );
            assert_eq!(r.status, 201, "{ct:?}");
        }
        // Stray control characters are dropped, not refused.
        let r = post(
            &f,
            &writer(),
            r#"{"text": "bell\u0007 here\u000c"}"#,
            None,
            Instant::now(),
        );
        let id = json_of(&r)["id"].as_str().unwrap().to_string();
        assert_eq!(
            archive::read_item(&f.archive, &id)
                .unwrap()
                .segments
                .segments[0]
                .text,
            "bell here"
        );
    }

    #[test]
    fn paragraphs_split_on_blank_lines() {
        assert_eq!(paragraphs("a\nb\n\n  \nc  \n"), vec!["a\nb", "c"]);
        assert_eq!(paragraphs("\n\n  x  \n\n"), vec!["x"]);
        assert!(paragraphs(" \n\t\n").is_empty());
        assert_eq!(clean_text("a\r\nb\rc\u{0}d\te"), "a\nb\ncd\te");
        assert!(json_content_type(None));
        assert!(json_content_type(Some(
            "application/json;charset=\"utf-8\""
        )));
        assert!(!json_content_type(Some("application/jsonp")));
        assert!(!json_content_type(Some("text/json")));
    }

    #[test]
    fn cleanup_runs_only_when_asked_and_only_on_a_local_profile() {
        assert_eq!(cleanup_gate(true, false), Ok(()));
        assert_eq!(cleanup_gate(false, false), Err(CleanupRefused::Off));
        // External: refused, active or not (the opt-in is not consulted).
        assert_eq!(cleanup_gate(true, true), Err(CleanupRefused::External));
        assert_eq!(cleanup_gate(false, true), Err(CleanupRefused::External));

        let f = fixture();
        // Not asked: never cleaned.
        let r = post(
            &f,
            &writer(),
            r#"{"text": "plain text"}"#,
            None,
            Instant::now(),
        );
        assert_eq!(
            (r.status, json_of(&r)["cleaned"].as_bool()),
            (201, Some(false))
        );
        assert_eq!(f.host.cleaned.load(Ordering::SeqCst), 0);
        // Asked, local: each paragraph cleaned, the text as sent kept as raw.
        let r = post(
            &f,
            &writer(),
            r#"{"text": "one\n\ntwo", "cleanup": true}"#,
            None,
            Instant::now(),
        );
        assert_eq!(
            (r.status, json_of(&r)["cleaned"].as_bool()),
            (201, Some(true))
        );
        assert_eq!(f.host.cleaned.load(Ordering::SeqCst), 2);
        let id = json_of(&r)["id"].as_str().unwrap().to_string();
        let item = archive::read_item(&f.archive, &id).unwrap();
        assert_eq!(
            item.meta.title, "ONE",
            "the title comes from the cleaned text"
        );
        assert_eq!(item.segments.segments[1].raw, "two");
        assert_eq!(item.segments.segments[1].text, "TWO");
        let before = items(&f);
        // External or off: refused, nothing created, nothing cleaned.
        for (gate, want) in [
            (CleanupRefused::External, "cleanup_external"),
            (CleanupRefused::Off, "cleanup_off"),
        ] {
            *f.host.gate.borrow_mut() = Err(gate);
            let r = post(
                &f,
                &writer(),
                r#"{"text": "secret", "cleanup": true}"#,
                None,
                Instant::now(),
            );
            assert_eq!((r.status, code(&r).as_str()), (409, want));
            // Without cleanup the same note goes through.
            assert_eq!(
                post(
                    &f,
                    &writer(),
                    r#"{"text": "fine", "cleanup": false}"#,
                    None,
                    Instant::now()
                )
                .status,
                201
            );
        }
        assert_eq!(f.host.cleaned.load(Ordering::SeqCst), 2);
        assert_eq!(items(&f), before + 2);
        // The settings changed between the gate and the cleanup: still refused.
        *f.host.gate.borrow_mut() = Ok(());
        *f.host.clean_refused.borrow_mut() = Some(CleanupRefused::External);
        let r = post(
            &f,
            &writer(),
            r#"{"text": "secret", "cleanup": true}"#,
            None,
            Instant::now(),
        );
        assert_eq!(code(&r), "cleanup_external");
        assert_eq!(items(&f), before + 2);
        // Too long to clean.
        *f.host.clean_refused.borrow_mut() = None;
        let long = json!({"text": "a".repeat(MAX_CLEANUP_BYTES + 1), "cleanup": true}).to_string();
        let r = post(&f, &writer(), &long, None, Instant::now());
        assert_eq!((r.status, code(&r).as_str()), (413, "too_long_for_cleanup"));
    }

    #[test]
    fn an_idempotency_key_returns_the_same_item() {
        let f = fixture();
        let t0 = Instant::now();
        let body = r#"{"text": "Buy milk"}"#;
        let first = post(&f, &writer(), body, Some("clip-1"), t0);
        assert_eq!(first.status, 201);
        let id = json_of(&first)["id"].as_str().unwrap().to_string();
        let again = post(
            &f,
            &writer(),
            body,
            Some("clip-1"),
            t0 + Duration::from_secs(60),
        );
        assert_eq!(again.status, 201);
        assert_eq!(json_of(&again)["id"], id.as_str());
        assert_eq!(json_of(&again)["replayed"], true);
        assert_eq!(header(&again, "Idempotent-Replayed"), Some("true"));
        assert_eq!(header(&again, "Location"), header(&first, "Location"));
        assert_eq!(items(&f), 1, "one note");
        // Another body with the same key: refused.
        let other = post(
            &f,
            &writer(),
            r#"{"text": "Buy bread"}"#,
            Some("clip-1"),
            t0,
        );
        assert_eq!(
            (other.status, code(&other).as_str()),
            (422, "idempotency_mismatch")
        );
        // Another token's key space is its own.
        let mut other_token = writer();
        other_token.id = "w2".into();
        let theirs = post(&f, &other_token, body, Some("clip-1"), t0);
        assert_ne!(json_of(&theirs)["id"], id.as_str());
        // No key, or a new one: a new note.
        assert_ne!(
            json_of(&post(&f, &writer(), body, None, t0))["id"],
            id.as_str()
        );
        assert_ne!(
            json_of(&post(&f, &writer(), body, Some("clip-2"), t0))["id"],
            id.as_str()
        );
        // After the TTL the key is forgotten.
        let later = post(
            &f,
            &writer(),
            body,
            Some("clip-1"),
            t0 + IDEMPOTENCY_TTL + Duration::from_secs(1),
        );
        assert_eq!(json_of(&later)["replayed"], false);
        assert_ne!(json_of(&later)["id"], id.as_str());
        // Malformed keys.
        let long = "k".repeat(MAX_IDEMPOTENCY_KEY + 1);
        for bad in ["", "   ", "with space", "caffè", long.as_str()] {
            let r = post(&f, &writer(), body, Some(bad), t0);
            assert_eq!(
                (r.status, code(&r).as_str()),
                (400, "invalid_idempotency_key"),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn a_refused_request_frees_its_idempotency_key() {
        let f = fixture();
        *f.host.gate.borrow_mut() = Err(CleanupRefused::External);
        let body = r#"{"text": "x", "cleanup": true}"#;
        assert_eq!(
            post(&f, &writer(), body, Some("k"), Instant::now()).status,
            409
        );
        *f.host.gate.borrow_mut() = Ok(());
        let r = post(&f, &writer(), body, Some("k"), Instant::now());
        assert_eq!(
            (r.status, json_of(&r)["replayed"].as_bool()),
            (201, Some(false))
        );
    }

    #[test]
    fn keys_being_handled_answer_409_and_the_store_is_bounded() {
        let store = Idempotency::default();
        let t0 = Instant::now();
        assert_eq!(store.claim("t", "k", b"a", t0), Claim::New);
        assert_eq!(store.claim("t", "k", b"a", t0), Claim::InProgress);
        assert_eq!(store.claim("t", "k", b"b", t0), Claim::Mismatch);
        store.finish("t", "k", json!({"id": "x"}));
        assert_eq!(
            store.claim("t", "k", b"a", t0),
            Claim::Replay(json!({"id": "x"}))
        );
        store.release("t", "k");
        assert_eq!(store.claim("t", "k", b"b", t0), Claim::New);
        for i in 0..MAX_IDEMPOTENCY_ENTRIES + 10 {
            let k = format!("k{i}");
            assert_eq!(
                store.claim("t", &k, b"a", t0 + Duration::from_millis(i as u64)),
                Claim::New
            );
            store.finish("t", &k, json!({"id": i}));
        }
        let len = store.entries.lock().unwrap().len();
        assert!(len <= MAX_IDEMPOTENCY_ENTRIES, "{len}");
        // The oldest went first; the newest are kept.
        let last = format!("k{}", MAX_IDEMPOTENCY_ENTRIES + 9);
        assert!(matches!(
            store.claim("t", &last, b"a", t0),
            Claim::Replay(_)
        ));
        assert_eq!(store.claim("t", "k0", b"zzz", t0), Claim::New);
    }

    #[test]
    fn creation_is_rate_limited_per_token_and_overall() {
        let f = fixture();
        let t0 = Instant::now();
        let mut created = 0;
        let limited = loop {
            let r = post(&f, &writer(), r#"{"text": "n"}"#, None, t0);
            if r.status != 201 {
                break r;
            }
            created += 1;
            assert!(created < 100, "never limited");
        };
        assert_eq!(created, CREATE_BURST as usize);
        assert_eq!(
            (limited.status, code(&limited).as_str()),
            (429, "rate_limited")
        );
        assert_eq!(header(&limited, "Retry-After"), Some("2"));
        assert_eq!(items(&f), created, "nothing past the limit");
        // A replay and a refusal cost nothing; validation errors neither.
        assert_eq!(post(&f, &writer(), "{}", None, t0).status, 400);
        // 30 a minute afterwards.
        assert_eq!(
            post(
                &f,
                &writer(),
                r#"{"text": "n"}"#,
                None,
                t0 + Duration::from_secs(2)
            )
            .status,
            201
        );
        assert_eq!(
            post(
                &f,
                &writer(),
                r#"{"text": "n"}"#,
                None,
                t0 + Duration::from_secs(2)
            )
            .status,
            429
        );

        // For all tokens together.
        let limits = CreateLimits::new(RateLimiter::new(5.0, 0.5), RateLimiter::new(8.0, 1.0));
        let mut ok = 0;
        for t in 0..4 {
            for _ in 0..5 {
                if limits.check(&format!("t{t}"), t0).is_ok() {
                    ok += 1;
                }
            }
        }
        assert_eq!(ok, 8, "the overall burst caps many tokens");
    }

    #[test]
    fn an_unwritable_archive_is_a_fixed_500() {
        let f = fixture();
        std::fs::create_dir_all(f.archive.parent().unwrap()).unwrap();
        std::fs::write(&f.archive, "a file, not a folder").unwrap();
        let r = post(&f, &writer(), r#"{"text": "x"}"#, Some("k"), Instant::now());
        assert_eq!((r.status, code(&r).as_str()), (500, "internal"));
        assert!(!json_of(&r)
            .to_string()
            .contains(f.archive.to_str().unwrap()));
        // The key was released.
        std::fs::remove_file(&f.archive).unwrap();
        assert_eq!(
            post(&f, &writer(), r#"{"text": "x"}"#, Some("k"), Instant::now()).status,
            201
        );
    }
}
