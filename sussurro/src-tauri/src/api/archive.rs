//! The archive read routes (#250; P15, E14; plan §4.2): scripts list,
//! search, read and export items, read companion documents and list
//! people. Everything here runs after the archive middleware
//! ([`super::tokens::authorize`]): the request had no browser `Origin`, the
//! `api_archive` switch was on, the token matched, was within its rate
//! limit and had the `read` scope.
//!
//! | Route | Returns |
//! |---|---|
//! | `GET /archive/items?q=&type=&tag=&category=&participant=&participant_key=&from=&to=&date=&today=&facets=&limit=&cursor=` | a page of items (+ facet counts) |
//! | `GET /archive/items/{id}` | frontmatter, transcript text, speakers (id + label), lines |
//! | `GET /archive/items/{id}/export?format=md\|txt\|srt\|vtt` | the export, as a file |
//! | `GET /archive/items/{id}/documents` | the companion documents (no bodies) |
//! | `GET /archive/items/{id}/documents/{name}` | one companion document |
//! | `GET /archive/people?limit=&cursor=` | the People registry: names and aliases |
//!
//! What is never served, whatever the token:
//! - speaker **embeddings** (`segments.json` vectors), **voice profiles**
//!   (they live in app data, P13), saved **audio**, and anything else from
//!   app data (the index path, `source-files.json`, settings) — responses
//!   are built field by field from the archive, never by passing a struct
//!   through, so a field added later doesn't leak by default;
//! - **people's emails** without the `people` scope: participant emails
//!   are left out of items and of the `.md` export's frontmatter, the text
//!   query skips the participants column (it indexes their emails) and
//!   `participant=` matches names only, so an email can't be probed
//!   ([`crate::archive::SearchFilters::names_only`]);
//! - error details: internal errors are a fixed message (an I/O error names
//!   a path), and nothing a caller sent is echoed back.
//!
//! Ids are validated with the archive's own check and confined to the
//! archive ([`crate::archive::paths::item_dir`]); an id may contain `/`
//! (`2026/09/2026-09-24-weekly-sync`) or `%2F`. No route takes a path.
//!
//! Responses are bounded: pages of at most [`MAX_LIMIT`] rows with an
//! opaque cursor, at most [`MAX_FACET_VALUES`] values per facet, and no
//! body larger than [`MAX_BODY_BYTES`] (a longer item answers 422
//! `too_large`).

use super::tokens::{Authorized, Scope};
use crate::archive::{
    self, companion, export::ExportFormat, facets::DateBucket, ItemMeta, ItemType, SearchFilters,
};
use serde_json::{json, Value};
use std::path::Path;

/// Rows per page when `limit` is not given.
pub const DEFAULT_LIMIT: usize = 50;
/// Most rows per page.
pub const MAX_LIMIT: usize = 200;
/// Most values listed per facet (tags, categories, participants).
pub const MAX_FACET_VALUES: usize = 100;
/// Largest response body.
pub const MAX_BODY_BYTES: usize = 16 << 20;
/// Longest text query, in characters.
pub const MAX_QUERY_CHARS: usize = 500;

/// A response body.
#[derive(Debug, Clone, PartialEq)]
pub enum Body {
    Json(Value),
    /// An export: sent as a file.
    File {
        content_type: &'static str,
        filename: String,
        text: String,
    },
}

/// What a route answers.
#[derive(Debug, Clone, PartialEq)]
pub struct Reply {
    pub status: u16,
    pub body: Body,
    /// Extra headers (`Retry-After`…).
    pub headers: Vec<(&'static str, String)>,
}

impl Reply {
    fn ok(body: Value) -> Self {
        Reply {
            status: 200,
            body: Body::Json(body),
            headers: Vec::new(),
        }
    }

    /// An error with a stable `code`.
    pub fn error(status: u16, code: &str, message: &str) -> Self {
        Reply {
            status,
            body: Body::Json(json!({"error": message, "code": code})),
            headers: Vec::new(),
        }
    }

    fn bad(message: &str) -> Self {
        Self::error(400, "bad_request", message)
    }

    fn not_found() -> Self {
        Self::error(404, "not_found", "no such item")
    }

    /// Internal failure: a fixed message (the error may name a path).
    pub fn internal() -> Self {
        Self::error(500, "internal", "the archive could not be read")
    }

    fn too_large() -> Self {
        Self::error(
            422,
            "too_large",
            "the answer would be too large: use a smaller limit or the export route",
        )
    }

    /// `self` unless its body exceeds [`MAX_BODY_BYTES`].
    fn bounded(self) -> Self {
        let size = match &self.body {
            Body::Json(v) => v.to_string().len(),
            Body::File { text, .. } => text.len(),
        };
        if size > MAX_BODY_BYTES {
            Self::too_large()
        } else {
            self
        }
    }
}

/// An archive route, from the raw (still percent-encoded) path.
#[derive(Debug, PartialEq, Eq)]
pub enum ArchiveRoute {
    Items,
    Item(String),
    Export(String),
    Documents(String),
    Document(String, String),
    People,
    /// The path names an item but its id or name is malformed.
    BadId,
    NotFound,
}

/// Pure: `%XX` decoding of one path piece (`None` when malformed).
fn decode_piece(s: &str) -> Option<String> {
    super::percent_decode(s).filter(|d| !d.is_empty())
}

/// Pure: method + raw path → route. Only `GET`/`HEAD` exist here (writes
/// come with #251).
pub fn parse_route(method: &str, path: &str) -> ArchiveRoute {
    if !(method.eq_ignore_ascii_case("GET") || method.eq_ignore_ascii_case("HEAD")) {
        return ArchiveRoute::NotFound;
    }
    match path {
        "/archive/items" | "/archive/items/" => return ArchiveRoute::Items,
        "/archive/people" | "/archive/people/" => return ArchiveRoute::People,
        _ => {}
    }
    let Some(rest) = path.strip_prefix("/archive/items/") else {
        return ArchiveRoute::NotFound;
    };
    let id = |raw: &str| decode_piece(raw).ok_or(ArchiveRoute::BadId);
    let parsed = if let Some(raw) = rest.strip_suffix("/export") {
        id(raw).map(ArchiveRoute::Export)
    } else if let Some(raw) = rest.strip_suffix("/documents") {
        id(raw).map(ArchiveRoute::Documents)
    } else if let Some((raw, name)) = rest.rsplit_once("/documents/") {
        id(raw).and_then(|i| {
            decode_piece(name)
                .map(|n| ArchiveRoute::Document(i, n))
                .ok_or(ArchiveRoute::BadId)
        })
    } else {
        id(rest.strip_suffix('/').unwrap_or(rest)).map(ArchiveRoute::Item)
    };
    parsed.unwrap_or_else(|e| e)
}

/// Pure: a query string as `(key, value)` pairs, percent- and
/// `+`-decoded. `None` on a malformed escape.
pub fn parse_query(query: &str) -> Option<Vec<(String, String)>> {
    let mut out = Vec::new();
    for pair in query.split('&').filter(|p| !p.is_empty()) {
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        let k = super::percent_decode(&k.replace('+', " "))?;
        let v = super::percent_decode(&v.replace('+', " "))?;
        out.push((k, v));
    }
    Some(out)
}

/// The query of one route: every key must be known, single-valued keys
/// may appear once.
struct Params(Vec<(String, String)>);

impl Params {
    fn new(query: &str, known: &[&str]) -> Result<Params, Reply> {
        let pairs = parse_query(query).ok_or_else(|| Reply::bad("malformed query string"))?;
        // The unknown key is not echoed: say what is accepted instead.
        if pairs.iter().any(|(k, _)| !known.contains(&k.as_str())) {
            let known = if known.is_empty() { vec!["none"] } else { known.to_vec() };
            return Err(Reply::bad(&format!(
                "unknown query parameter; this route accepts: {}",
                known.join(", ")
            )));
        }
        Ok(Params(pairs))
    }

    fn all(&self, key: &str) -> Vec<&str> {
        self.0.iter().filter(|(k, _)| k == key).map(|(_, v)| v.as_str()).collect()
    }

    fn one(&self, key: &str) -> Result<Option<&str>, Reply> {
        match self.all(key).as_slice() {
            [] => Ok(None),
            [v] => Ok(Some(*v).filter(|v| !v.trim().is_empty())),
            _ => Err(Reply::bad(&format!("`{key}` may be given only once"))),
        }
    }
}

/// Pure: an opaque cursor for the row at `offset`.
pub fn encode_cursor(offset: usize) -> String {
    format!("c1.{offset:x}")
        .bytes()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Pure: the offset of a cursor made by [`encode_cursor`].
pub fn decode_cursor(cursor: &str) -> Option<usize> {
    if !cursor.len().is_multiple_of(2) || cursor.len() > 64 {
        return None;
    }
    let bytes: Vec<u8> = (0..cursor.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(cursor.get(i..i + 2)?, 16).ok())
        .collect::<Option<_>>()?;
    let text = String::from_utf8(bytes).ok()?;
    usize::from_str_radix(text.strip_prefix("c1.")?, 16).ok()
}

/// `limit` and `cursor` → `(offset, limit)`.
fn page(params: &Params) -> Result<(usize, usize), Reply> {
    let limit = match params.one("limit")? {
        None => DEFAULT_LIMIT,
        Some(v) => match v.trim().parse::<usize>() {
            Ok(n) if (1..=MAX_LIMIT).contains(&n) => n,
            _ => return Err(Reply::bad(&format!("`limit` must be a number from 1 to {MAX_LIMIT}"))),
        },
    };
    let offset = match params.one("cursor")? {
        None => 0,
        Some(c) => decode_cursor(c.trim())
            .ok_or_else(|| Reply::error(400, "bad_cursor", "invalid cursor: pass `next_cursor` as returned"))?,
    };
    Ok((offset, limit))
}

/// The page of `rows` at `offset`, and the next cursor.
fn slice<T>(rows: Vec<T>, offset: usize, limit: usize) -> (Vec<T>, Option<String>) {
    let total = rows.len();
    let end = offset.saturating_add(limit).min(total);
    let next = (end < total).then(|| encode_cursor(end));
    let page = rows.into_iter().skip(offset).take(end.saturating_sub(offset)).collect();
    (page, next)
}

fn day(v: &str, key: &str) -> Result<String, Reply> {
    let d = v.trim();
    let d = d.get(..10).unwrap_or(d);
    chrono::NaiveDate::parse_from_str(d, "%Y-%m-%d")
        .map(|_| d.to_string())
        .map_err(|_| Reply::bad(&format!("`{key}` must be a date, YYYY-MM-DD")))
}

/// Pure: the search filters of `GET /archive/items`, plus the text query
/// and whether facets were asked for.
fn item_filters(params: &Params, emails: bool) -> Result<(String, SearchFilters, bool), Reply> {
    let q = params.one("q")?.unwrap_or("").to_string();
    if q.chars().count() > MAX_QUERY_CHARS {
        return Err(Reply::bad(&format!("`q` is too long (at most {MAX_QUERY_CHARS} characters)")));
    }
    let mut f = SearchFilters {
        names_only: !emails,
        ..Default::default()
    };
    for t in params.all("type") {
        f.types.push(
            ItemType::parse(t).ok_or_else(|| Reply::bad("`type` must be note, meeting or transcription"))?,
        );
    }
    let values = |key: &str| -> Vec<String> {
        params.all(key).into_iter().map(str::trim).filter(|v| !v.is_empty()).map(str::to_string).collect()
    };
    f.tags = values("tag");
    f.categories = values("category");
    f.participants = values("participant_key");
    for (key, list) in [("tag", &f.tags), ("category", &f.categories), ("participant_key", &f.participants)] {
        if list.len() > archive::facets::MAX_VALUES {
            return Err(Reply::bad(&format!(
                "too many `{key}` values (at most {})",
                archive::facets::MAX_VALUES
            )));
        }
    }
    f.participant = params.one("participant")?.map(|p| p.trim().to_string());
    f.date_from = params.one("from")?.map(|v| day(v, "from")).transpose()?;
    f.date_to = params.one("to")?.map(|v| day(v, "to")).transpose()?;
    f.today = params.one("today")?.map(|v| day(v, "today")).transpose()?;
    f.date_bucket = match params.one("date")? {
        None => None,
        Some(v) => Some(
            DateBucket::ALL
                .into_iter()
                .find(|b| b.as_str().eq_ignore_ascii_case(v.trim()))
                .ok_or_else(|| Reply::bad("`date` must be today, week, month, year or older"))?,
        ),
    };
    let facets = match params.one("facets")? {
        None => false,
        Some(v) => match v.trim() {
            "1" | "true" | "yes" => true,
            "0" | "false" | "no" => false,
            _ => return Err(Reply::bad("`facets` must be true or false")),
        },
    };
    Ok((q, f, facets))
}

/// Participants as served: names only, or with emails for the `people`
/// scope.
fn participants(meta: &ItemMeta, emails: bool) -> Value {
    Value::Array(
        meta.participants
            .iter()
            .map(|p| match (&p.email, emails) {
                (Some(e), true) => json!({"name": p.name, "email": e}),
                _ => json!({"name": p.name}),
            })
            .collect(),
    )
}

/// The frontmatter as JSON: the known fields, then the user's own keys as
/// read. Participant emails only with the `people` scope.
fn meta_json(meta: &ItemMeta, emails: bool) -> Value {
    let mut out = serde_json::Map::new();
    for (k, v) in &meta.extra {
        out.insert(k.clone(), v.clone());
    }
    out.insert("type".into(), json!(meta.item_type.as_str()));
    out.insert("title".into(), json!(meta.title));
    out.insert("date".into(), json!(meta.date));
    out.insert("duration".into(), json!(meta.duration));
    out.insert("source".into(), json!(meta.source));
    out.insert("language".into(), json!(meta.language));
    out.insert("engine".into(), json!(meta.engine));
    out.insert("tags".into(), json!(meta.tags));
    out.insert("categories".into(), json!(meta.categories));
    out.insert("participants".into(), participants(meta, emails));
    Value::Object(out)
}

/// A list row: never the frontmatter's other keys, participants by name.
fn summary_json(s: &archive::ItemSummary) -> Value {
    let mut row = json!({
        "id": s.id,
        "type": s.meta.item_type.as_str(),
        "title": s.meta.title,
        "date": s.meta.date,
        "duration": s.meta.duration,
        "tags": s.meta.tags,
        "categories": s.meta.categories,
        "participants": s.meta.participants.iter().map(|p| p.name.clone()).collect::<Vec<_>>(),
        "recording": s.recording,
        "interrupted": s.interrupted,
    });
    if let Some(snippet) = &s.snippet {
        row["snippet"] = json!(snippet);
    }
    row
}

fn facets_json(facets: &archive::facets::Facets) -> Value {
    let values = |list: &[archive::facets::FacetValue]| {
        json!({
            "values": list.iter().take(MAX_FACET_VALUES).map(|v| json!({"key": v.key, "label": v.label, "count": v.count})).collect::<Vec<_>>(),
            "truncated": list.len() > MAX_FACET_VALUES,
        })
    };
    json!({
        "total": facets.total,
        "types": values(&facets.types),
        "tags": values(&facets.tags),
        "categories": values(&facets.categories),
        "participants": values(&facets.participants),
        "dates": values(&facets.dates),
    })
}

/// Item `id` exists: its id is valid and confined, and it has a transcript.
fn check_item(archive_dir: &Path, id: &str) -> Result<(), Reply> {
    if archive::paths::validate_item_id(id).is_err() {
        return Err(Reply::error(400, "invalid_id", "invalid item id"));
    }
    match archive::paths::item_dir(archive_dir, id) {
        Ok(dir) if dir.join(archive::store::TRANSCRIPT_FILE).is_file() => Ok(()),
        _ => Err(Reply::not_found()),
    }
}

fn read_item(archive_dir: &Path, id: &str) -> Result<archive::Item, Reply> {
    check_item(archive_dir, id)?;
    archive::read_item(archive_dir, id)
        .map_err(|_| Reply::error(422, "unreadable_item", "the item exists but could not be read (invalid frontmatter?)"))
}

fn get_items(archive_dir: &Path, index: &Path, auth: &Authorized, query: &str) -> Reply {
    let known = [
        "q", "type", "tag", "category", "participant", "participant_key", "from", "to", "date", "today",
        "facets", "limit", "cursor",
    ];
    let result = (|| {
        let params = Params::new(query, &known)?;
        let (offset, limit) = page(&params)?;
        let (q, filters, with_facets) = item_filters(&params, auth.has(Scope::People))?;
        let (rows, facets) =
            archive::with_index(archive_dir, index, |idx| idx.search_rows(&q, &filters, with_facets))
                .map_err(|_| Reply::internal())?;
        let total = rows.len();
        let (rows, next) = slice(rows, offset, limit);
        let mut body = json!({
            "items": rows.iter().map(summary_json).collect::<Vec<_>>(),
            "total": total,
            "next_cursor": next,
        });
        if let Some(f) = facets {
            body["facets"] = facets_json(&f);
        }
        Ok(Reply::ok(body))
    })();
    result.unwrap_or_else(|e| e)
}

fn get_item(archive_dir: &Path, auth: &Authorized, id: &str, query: &str) -> Reply {
    let result = (|| {
        Params::new(query, &[])?;
        let item = read_item(archive_dir, id)?;
        let segs = &item.segments;
        // Built field by field: never an embedding, raw words or an error.
        let lines: Vec<Value> = segs
            .segments
            .iter()
            .filter(|s| !s.text.trim().is_empty())
            .map(|s| {
                json!({
                    "id": s.id,
                    "start_ms": s.start_ms,
                    "end_ms": s.end_ms,
                    "channel": s.channel,
                    "speaker": s.speaker_id,
                    "text": s.text,
                })
            })
            .collect();
        let speakers: Vec<Value> = segs
            .speakers
            .iter()
            .map(|sp| json!({"id": sp.id, "label": sp.label}))
            .collect();
        Ok(Reply::ok(json!({
            "id": item.id,
            "meta": meta_json(&item.meta, auth.has(Scope::People)),
            "text": item.body,
            "speakers": speakers,
            "lines": lines,
            "edited_externally": item.edited_externally,
            "recording": item.recording,
            "interrupted": item.interrupted,
        })))
    })();
    result.unwrap_or_else(|e| e)
}

/// `transcript.md` as stored, with participant emails taken out of its
/// frontmatter (only when there are some: otherwise the file as it is).
fn md_without_emails(stored: &str) -> Result<String, Reply> {
    let (mut meta, _) = archive::frontmatter::parse(stored).map_err(|_| Reply::internal())?;
    if meta.participants.iter().all(|p| p.email.is_none()) {
        return Ok(stored.to_string());
    }
    for p in &mut meta.participants {
        p.email = None;
    }
    archive::frontmatter::replace(stored, &meta).map_err(|_| Reply::internal())
}

fn get_export(archive_dir: &Path, auth: &Authorized, id: &str, query: &str) -> Reply {
    let result = (|| {
        let params = Params::new(query, &["format"])?;
        let format = super::export::parse_format(params.one("format")?)
            .ok_or_else(|| Reply::bad("`format` must be md, txt, srt or vtt"))?;
        let item = read_item(archive_dir, id)?;
        if format.is_subtitles() && item.meta.item_type == ItemType::Note {
            return Err(Reply::error(422, "unsupported_format", archive::export::NOTE_SUBTITLES_ERROR));
        }
        let mut text = archive::export::export_item(archive_dir, id, format).map_err(|_| {
            Reply::error(422, "unsupported_format", "this item has nothing to export in that format yet")
        })?;
        if format == ExportFormat::Md && !auth.has(Scope::People) {
            text = md_without_emails(&text)?;
        }
        let folder = id.rsplit('/').next().unwrap_or("transcript");
        Ok(Reply {
            status: 200,
            body: Body::File {
                content_type: super::export::content_type(format),
                filename: format!("{folder}.{}", format.extension()),
                text,
            },
            headers: Vec::new(),
        })
    })();
    result.unwrap_or_else(|e| e)
}

fn get_documents(archive_dir: &Path, id: &str, query: &str) -> Reply {
    let result = (|| {
        Params::new(query, &[])?;
        read_item(archive_dir, id)?;
        let docs = companion::list_companions(archive_dir, id).map_err(|_| Reply::internal())?;
        let docs: Vec<Value> = docs
            .iter()
            .map(|d| {
                json!({
                    "name": d.file,
                    "title": d.meta.title,
                    "recipe": d.meta.recipe,
                    "generated_by": d.meta.generated_by,
                    "date": d.meta.date,
                    "edited_externally": d.edited_externally,
                })
            })
            .collect();
        Ok(Reply::ok(json!({"id": id, "documents": docs})))
    })();
    result.unwrap_or_else(|e| e)
}

fn get_document(archive_dir: &Path, id: &str, name: &str, query: &str) -> Reply {
    let result = (|| {
        Params::new(query, &[])?;
        if companion::validate_companion_name(name).is_err() {
            return Err(Reply::error(400, "invalid_name", "invalid document name: a .md file of the item, not the transcript"));
        }
        read_item(archive_dir, id)?;
        let doc = companion::read_companion(archive_dir, id, name)
            .map_err(|_| Reply::error(404, "not_found", "no such document"))?;
        let m = &doc.meta;
        let mut meta = serde_json::Map::new();
        for (k, v) in &m.extra {
            meta.insert(k.clone(), v.clone());
        }
        for (k, v) in [
            ("title", json!(m.title)),
            ("generated_by", json!(m.generated_by)),
            ("recipe", json!(m.recipe)),
            ("profile", json!(m.profile)),
            ("model", json!(m.model)),
            ("external", json!(m.external)),
            ("host", json!(m.host)),
            ("date", json!(m.date)),
            ("transcript", json!(m.transcript)),
        ] {
            meta.insert(k.into(), v);
        }
        Ok(Reply::ok(json!({
            "id": id,
            "name": doc.file,
            "meta": meta,
            "text": doc.body,
            "edited_externally": doc.edited_externally,
        })))
    })();
    result.unwrap_or_else(|e| e)
}

fn get_people(archive_dir: &Path, auth: &Authorized, query: &str) -> Reply {
    let result = (|| {
        let params = Params::new(query, &["limit", "cursor"])?;
        let (offset, limit) = page(&params)?;
        let people = archive::people::list_people(archive_dir).map_err(|_| {
            Reply::error(422, "unreadable_people", "the People registry could not be read")
        })?;
        let total = people.len();
        let (people, next) = slice(people, offset, limit);
        let emails = auth.has(Scope::People);
        // Built field by field: a person's future fields (voice data,
        // #241) never reach the API by default.
        let rows: Vec<Value> = people
            .iter()
            .map(|p| {
                let mut row = json!({"id": p.id, "name": p.name, "aliases": p.aliases});
                if emails {
                    row["email"] = json!(p.email);
                }
                row
            })
            .collect();
        Ok(Reply::ok(json!({
            "people": rows,
            "total": total,
            "next_cursor": next,
            "emails": emails,
        })))
    })();
    result.unwrap_or_else(|e| e)
}

/// Answer an authorized archive request. `url` is the request target
/// (path and query, as sent); `index` is the search index file.
pub fn handle(archive_dir: &Path, index: &Path, auth: &Authorized, method: &str, url: &str) -> Reply {
    let (path, query) = url.split_once('?').unwrap_or((url, ""));
    let reply = match parse_route(method, path) {
        ArchiveRoute::Items => get_items(archive_dir, index, auth, query),
        ArchiveRoute::Item(id) => get_item(archive_dir, auth, &id, query),
        ArchiveRoute::Export(id) => get_export(archive_dir, auth, &id, query),
        ArchiveRoute::Documents(id) => get_documents(archive_dir, &id, query),
        ArchiveRoute::Document(id, name) => get_document(archive_dir, &id, &name, query),
        ArchiveRoute::People => get_people(archive_dir, auth, query),
        ArchiveRoute::BadId => Reply::error(400, "invalid_id", "invalid item id"),
        ArchiveRoute::NotFound => Reply::error(404, "not_found", "unknown archive endpoint"),
    };
    reply.bounded()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::archive::{Channel, DocSpeaker, Participant, Segment, SegmentsFile};
    use std::path::PathBuf;

    const EMAIL: &str = "anna@example.com";
    const PERSON_EMAIL: &str = "bob@example.org";

    fn read() -> Authorized {
        Authorized { id: "r".into(), scopes: vec![Scope::Read] }
    }

    fn people_scope() -> Authorized {
        Authorized { id: "rp".into(), scopes: vec![Scope::Read, Scope::People] }
    }

    struct Fx {
        _tmp: tempfile::TempDir,
        archive: PathBuf,
        index: PathBuf,
        meeting: String,
        note: String,
    }

    fn fixture() -> Fx {
        let tmp = tempfile::tempdir().unwrap();
        let archive = tmp.path().join("Sussurro");
        let index = tmp.path().join("appdata").join(archive::INDEX_FILE);
        let meeting_meta = ItemMeta {
            item_type: ItemType::Meeting,
            title: "Weekly sync".into(),
            date: "2026-09-24T10:00:00+02:00".into(),
            source: "browser:meet.google.com".into(),
            tags: vec!["release".into()],
            participants: vec![
                Participant { name: "Anna Rossi".into(), email: Some(EMAIL.into()) },
                Participant { name: "Carlo".into(), email: None },
            ],
            ..Default::default()
        };
        let segs = SegmentsFile {
            speakers: vec![DocSpeaker {
                id: "voice:1".into(),
                label: "Anna Rossi".into(),
                color: "#f00".into(),
                person_id: Some("p-anna".into()),
                label_before_link: Some("Voice 1".into()),
            }],
            segments: vec![Segment {
                id: 0,
                channel: Channel::Remote,
                start_ms: 723_000,
                end_ms: 725_000,
                speaker_id: Some("voice:1".into()),
                raw: "ehm roadmap raw".into(),
                text: "Parliamo della roadmap.".into(),
                embedding: Some(vec![0.123_456_79; 8]),
                ..Default::default()
            }],
            ..Default::default()
        };
        let meeting = archive::create_item(&archive, &meeting_meta, &segs).unwrap();
        let note_meta = ItemMeta {
            item_type: ItemType::Note,
            title: "Idea caffè".into(),
            date: "2026-09-20T08:00:00+02:00".into(),
            source: "mic".into(),
            ..Default::default()
        };
        let note_segs = SegmentsFile {
            segments: vec![Segment {
                id: 0,
                start_ms: 0,
                end_ms: 1000,
                text: "Comprare il caffè.".into(),
                ..Default::default()
            }],
            ..Default::default()
        };
        let note = archive::create_item(&archive, &note_meta, &note_segs).unwrap();
        // A companion document, and a People registry with an email and a
        // field this version doesn't know (a later voice profile, say).
        std::fs::write(
            archive.join(&meeting).join("document.md"),
            "---\ntitle: Minutes\nrecipe: formatted-document\ngenerated_by: Formatted document / Local / qwen\n---\n\n# Minutes\n\nRoadmap agreed.\n",
        )
        .unwrap();
        std::fs::create_dir_all(archive.join(".sussurro")).unwrap();
        std::fs::write(
            archive.join(".sussurro").join("people.json"),
            json!({"version": 1, "people": [
                {"id": "p-bob", "name": "Bob", "email": PERSON_EMAIL, "aliases": ["Roberto"], "voice": {"centroid": [0.5, 0.25]}},
                {"id": "p-anna", "name": "Anna Rossi", "aliases": []},
            ]})
            .to_string(),
        )
        .unwrap();
        Fx { _tmp: tmp, archive, index, meeting, note }
    }

    fn get(f: &Fx, auth: &Authorized, url: &str) -> Reply {
        handle(&f.archive, &f.index, auth, "GET", url)
    }

    fn json_of(r: &Reply) -> Value {
        match &r.body {
            Body::Json(v) => v.clone(),
            Body::File { .. } => panic!("expected JSON, got a file"),
        }
    }

    fn text_of(r: &Reply) -> String {
        match &r.body {
            Body::Json(v) => v.to_string(),
            Body::File { text, .. } => text.clone(),
        }
    }

    fn code(r: &Reply) -> String {
        json_of(r)["code"].as_str().unwrap_or_default().to_string()
    }

    fn ids(r: &Reply) -> Vec<String> {
        json_of(r)["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|i| i["id"].as_str().unwrap().to_string())
            .collect()
    }

    #[test]
    fn routes_parse_from_the_raw_path() {
        use ArchiveRoute::*;
        assert_eq!(parse_route("GET", "/archive/items"), Items);
        assert_eq!(parse_route("HEAD", "/archive/items/"), Items);
        assert_eq!(parse_route("GET", "/archive/people"), People);
        let id = "2026/09/2026-09-24-weekly-sync";
        assert_eq!(parse_route("GET", &format!("/archive/items/{id}")), Item(id.into()));
        assert_eq!(parse_route("GET", "/archive/items/2026%2F09%2F2026-09-24-weekly-sync"), Item(id.into()));
        assert_eq!(parse_route("GET", &format!("/archive/items/{id}/export")), Export(id.into()));
        assert_eq!(parse_route("GET", &format!("/archive/items/{id}/documents")), Documents(id.into()));
        assert_eq!(
            parse_route("GET", &format!("/archive/items/{id}/documents/document.md")),
            Document(id.into(), "document.md".into())
        );
        assert_eq!(
            parse_route("GET", "/archive/items/a%2Fb/documents/my%20notes.md"),
            Document("a/b".into(), "my notes.md".into())
        );
        for bad in ["/archive/items/%zz", "/archive/items/%zz/export", "/archive/items/x/documents/%ff"] {
            assert_eq!(parse_route("GET", bad), BadId, "{bad}");
        }
        for (m, p) in [
            ("POST", "/archive/items"),
            ("DELETE", "/archive/items/x"),
            ("PUT", "/archive/people"),
            ("GET", "/archive"),
            ("GET", "/archive/"),
            ("GET", "/archive/voices"),
            ("GET", "/archive/items//export"),
        ] {
            assert!(matches!(parse_route(m, p), NotFound | BadId), "{m} {p}");
        }
    }

    #[test]
    fn query_strings_decode_and_cursors_are_opaque() {
        let q = parse_query("q=caff%C3%A8+latte&tag=a&tag=b&flag").unwrap();
        assert_eq!(q[0], ("q".into(), "caffè latte".into()));
        assert_eq!(q.iter().filter(|(k, _)| k == "tag").count(), 2);
        assert_eq!(q[3], ("flag".into(), "".into()));
        assert!(parse_query("q=%").is_none());
        for n in [0, 1, 50, 199, 123_456] {
            let c = encode_cursor(n);
            assert!(c.chars().all(|ch| ch.is_ascii_hexdigit()), "{c}");
            assert_eq!(decode_cursor(&c), Some(n));
        }
        let long = "00".repeat(40);
        for bad in ["", "1", "zz", "c1.10", long.as_str(), "63312e7a"] {
            assert_eq!(decode_cursor(bad), None, "{bad}");
        }
    }

    #[test]
    fn items_are_listed_with_filters_and_facets() {
        let f = fixture();
        let r = get(&f, &read(), "/archive/items");
        assert_eq!(r.status, 200);
        let v = json_of(&r);
        assert_eq!(v["total"], 2);
        assert_eq!(v["next_cursor"], Value::Null);
        assert!(v.get("facets").is_none(), "facets only on request");
        let first = &v["items"][0];
        assert_eq!(first["id"], f.meeting.as_str());
        assert_eq!(first["type"], "meeting");
        assert_eq!(first["title"], "Weekly sync");
        assert_eq!(first["participants"], json!(["Anna Rossi", "Carlo"]));
        assert_eq!(first["tags"], json!(["release"]));
        assert_eq!(v["items"][1]["id"], f.note.as_str());

        let only = |url: &str| -> Vec<String> {
            let r = get(&f, &read(), url);
            assert_eq!(r.status, 200, "{url}: {}", text_of(&r));
            ids(&r)
        };
        assert_eq!(only("/archive/items?type=note"), vec![f.note.clone()]);
        assert_eq!(only("/archive/items?type=note&type=MEETING").len(), 2);
        assert_eq!(only("/archive/items?tag=Release"), vec![f.meeting.clone()]);
        assert_eq!(only("/archive/items?participant=anna%20rossi"), vec![f.meeting.clone()]);
        assert_eq!(only("/archive/items?from=2026-09-21&to=2026-09-30"), vec![f.meeting.clone()]);
        assert_eq!(only("/archive/items?date=week&today=2026-09-24"), vec![f.meeting.clone()]);
        assert_eq!(only("/archive/items?q=caff"), vec![f.note.clone()]);
        let hit = get(&f, &read(), "/archive/items?q=roadmap");
        assert!(json_of(&hit)["items"][0]["snippet"].as_str().unwrap().contains("**"));

        let faceted = json_of(&get(&f, &read(), "/archive/items?facets=true"));
        assert_eq!(faceted["facets"]["total"], 2);
        assert_eq!(faceted["facets"]["types"]["values"].as_array().unwrap().len(), 3);
        assert_eq!(faceted["facets"]["tags"]["truncated"], false);
        let key = faceted["facets"]["participants"]["values"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["label"] == "Carlo")
            .unwrap()["key"]
            .as_str()
            .unwrap()
            .to_string();
        assert_eq!(only(&format!("/archive/items?participant_key={key}")), vec![f.meeting.clone()]);
    }

    #[test]
    fn pagination_is_bounded_and_walks_every_item_once() {
        let f = fixture();
        for i in 0..5 {
            let meta = ItemMeta {
                title: format!("Extra {i}"),
                date: format!("2026-08-0{}T08:00:00+02:00", i + 1),
                ..Default::default()
            };
            archive::create_item(&f.archive, &meta, &SegmentsFile::default()).unwrap();
        }
        let mut seen = Vec::new();
        let mut url = "/archive/items?limit=3".to_string();
        loop {
            let r = get(&f, &read(), &url);
            let v = json_of(&r);
            assert_eq!(v["total"], 7);
            let page = ids(&r);
            assert!(page.len() <= 3);
            seen.extend(page);
            match v["next_cursor"].as_str() {
                Some(c) => url = format!("/archive/items?limit=3&cursor={c}"),
                None => break,
            }
        }
        assert_eq!(seen.len(), 7);
        let mut unique = seen.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(unique.len(), 7, "no item twice");
        // A cursor past the end is an empty page, not an error.
        let past = get(&f, &read(), &format!("/archive/items?cursor={}", encode_cursor(1000)));
        assert!(ids(&past).is_empty());
        assert_eq!(json_of(&past)["next_cursor"], Value::Null);
        // Bounds.
        for url in [
            "/archive/items?limit=0",
            "/archive/items?limit=201",
            "/archive/items?limit=-1",
            "/archive/items?limit=abc",
            "/archive/items?limit=1&limit=2",
            "/archive/items?q=a&q=b",
            "/archive/items?type=memo",
            "/archive/items?from=yesterday",
            "/archive/items?date=decade",
            "/archive/items?facets=maybe",
            "/archive/items?path=/etc/passwd",
            "/archive/items?q=%",
        ] {
            let r = get(&f, &read(), url);
            assert_eq!((r.status, code(&r).as_str()), (400, "bad_request"), "{url}");
            assert!(!text_of(&r).contains("/etc/passwd"), "never echoed");
        }
        let long = format!("/archive/items?q={}", "a".repeat(MAX_QUERY_CHARS + 1));
        assert_eq!(get(&f, &read(), &long).status, 400);
        let too_many = format!(
            "/archive/items?{}",
            (0..=archive::facets::MAX_VALUES).map(|i| format!("tag=t{i}")).collect::<Vec<_>>().join("&")
        );
        assert_eq!(get(&f, &read(), &too_many).status, 400);
        for c in ["nope", "c1.1", "zz"] {
            let r = get(&f, &read(), &format!("/archive/items?cursor={c}"));
            assert_eq!((r.status, code(&r).as_str()), (400, "bad_cursor"), "{c}");
        }
        assert_eq!(ids(&get(&f, &read(), "/archive/items?limit=200")).len(), 7);
    }

    #[test]
    fn an_item_is_its_frontmatter_text_speakers_and_lines_never_embeddings() {
        let f = fixture();
        let r = get(&f, &read(), &format!("/archive/items/{}", f.meeting));
        assert_eq!(r.status, 200);
        let v = json_of(&r);
        assert_eq!(v["id"], f.meeting.as_str());
        assert_eq!(v["meta"]["type"], "meeting");
        assert_eq!(v["meta"]["source"], "browser:meet.google.com");
        assert_eq!(v["meta"]["participants"], json!([{"name": "Anna Rossi"}, {"name": "Carlo"}]));
        assert!(v["text"].as_str().unwrap().contains("**[00:12:03] Anna Rossi:** Parliamo della roadmap."));
        assert_eq!(v["speakers"], json!([{"id": "voice:1", "label": "Anna Rossi"}]));
        assert_eq!(
            v["lines"],
            json!([{"id": 0, "start_ms": 723000, "end_ms": 725000, "channel": "remote", "speaker": "voice:1", "text": "Parliamo della roadmap."}])
        );
        let body = text_of(&r);
        for forbidden in [
            "embedding",
            "0.12345",
            "person_id",
            "p-anna",
            "label_before_link",
            "ehm roadmap raw",
            "folder_bytes",
            "external_hosts",
            "audio",
            EMAIL,
        ] {
            assert!(!body.contains(forbidden), "{forbidden} in {body}");
        }
        // With the people scope, the participant's email.
        let full = json_of(&get(&f, &people_scope(), &format!("/archive/items/{}", f.meeting)));
        assert_eq!(full["meta"]["participants"][0], json!({"name": "Anna Rossi", "email": EMAIL}));
        assert!(!full.to_string().contains("embedding"));
        // Also through %2F.
        assert_eq!(get(&f, &read(), &format!("/archive/items/{}", f.meeting.replace('/', "%2F"))).status, 200);
    }

    #[test]
    fn unknown_and_invalid_ids_are_refused() {
        let f = fixture();
        std::fs::write(f.archive.parent().unwrap().join("secret.md"), "outside").unwrap();
        for id in ["2026/09/nope", "nope", "2026/09"] {
            for suffix in ["", "/export", "/documents", "/documents/document.md"] {
                let r = get(&f, &read(), &format!("/archive/items/{id}{suffix}"));
                assert_eq!((r.status, code(&r).as_str()), (404, "not_found"), "{id}{suffix}");
                assert!(!text_of(&r).contains(id), "the id is not echoed");
            }
        }
        for id in ["..%2Fsecret", ".sussurro", "2026/../..", "%2Fetc%2Fpasswd", "a%5Cb", "C:%2Fx", "a%00b"] {
            let r = get(&f, &read(), &format!("/archive/items/{id}"));
            assert_eq!((r.status, code(&r).as_str()), (400, "invalid_id"), "{id}");
        }
        // Unknown parameters on the item routes too.
        assert_eq!(get(&f, &read(), &format!("/archive/items/{}?x=1", f.meeting)).status, 400);
    }

    #[test]
    fn exports_every_format_and_leaves_emails_out_without_the_people_scope() {
        let f = fixture();
        let export = |auth: &Authorized, id: &str, fmt: &str| {
            get(&f, auth, &format!("/archive/items/{id}/export?format={fmt}"))
        };
        let md = export(&read(), &f.meeting, "md");
        assert_eq!(md.status, 200);
        let Body::File { content_type, filename, text } = &md.body else { panic!("a file") };
        assert!(content_type.starts_with("text/markdown"));
        assert!(filename.ends_with("weekly-sync.md"), "{filename}");
        assert!(text.contains("Anna Rossi") && !text.contains(EMAIL), "{text}");
        assert!(text.contains("Parliamo della roadmap."));
        // The people scope gets the file as stored.
        let stored = std::fs::read_to_string(f.archive.join(&f.meeting).join("transcript.md")).unwrap();
        assert!(stored.contains(EMAIL));
        assert_eq!(text_of(&export(&people_scope(), &f.meeting, "md")), stored);
        // Without emails in the frontmatter the file is as stored for anyone.
        let note_stored = std::fs::read_to_string(f.archive.join(&f.note).join("transcript.md")).unwrap();
        assert_eq!(text_of(&export(&read(), &f.note, "md")), note_stored);
        assert_eq!(
            text_of(&get(&f, &read(), &format!("/archive/items/{}/export", f.note))),
            note_stored,
            "md by default"
        );

        assert_eq!(text_of(&export(&read(), &f.meeting, "txt")), "[00:12:03] Anna Rossi: Parliamo della roadmap.\n");
        assert!(text_of(&export(&read(), &f.meeting, "srt")).contains("00:12:03,000 --> "));
        assert!(text_of(&export(&read(), &f.meeting, "VTT")).starts_with("WEBVTT"));
        let note_srt = export(&read(), &f.note, "srt");
        assert_eq!((note_srt.status, code(&note_srt).as_str()), (422, "unsupported_format"));
        for fmt in ["docx", "wav", "json"] {
            assert_eq!(export(&read(), &f.meeting, fmt).status, 400, "{fmt}");
        }
        assert_eq!(get(&f, &read(), &format!("/archive/items/{}/export?format=md&path=x", f.meeting)).status, 400);
    }

    #[test]
    fn companion_documents_are_listed_and_read_by_name_only() {
        let f = fixture();
        let list = json_of(&get(&f, &read(), &format!("/archive/items/{}/documents", f.meeting)));
        assert_eq!(list["documents"].as_array().unwrap().len(), 1);
        assert_eq!(list["documents"][0]["name"], "document.md");
        assert_eq!(list["documents"][0]["title"], "Minutes");
        assert!(list["documents"][0].get("text").is_none(), "no bodies in the list");
        let none = json_of(&get(&f, &read(), &format!("/archive/items/{}/documents", f.note)));
        assert_eq!(none["documents"], json!([]));

        let doc = get(&f, &read(), &format!("/archive/items/{}/documents/document.md", f.meeting));
        assert_eq!(doc.status, 200);
        let d = json_of(&doc);
        assert_eq!(d["meta"]["recipe"], "formatted-document");
        assert!(d["text"].as_str().unwrap().contains("Roadmap agreed."));

        let missing = get(&f, &read(), &format!("/archive/items/{}/documents/other.md", f.meeting));
        assert_eq!((missing.status, code(&missing).as_str()), (404, "not_found"));
        for name in [
            "transcript.md",
            ".hidden.md",
            "segments.json",
            "..%2F..%2Fsecret.md",
            "a%5Cb.md",
            "audio.wav",
            ".sussurro%2Fsegments.json",
        ] {
            let r = get(&f, &read(), &format!("/archive/items/{}/documents/{name}", f.meeting));
            assert_eq!((r.status, code(&r).as_str()), (400, "invalid_name"), "{name}");
        }
    }

    #[test]
    fn people_are_names_and_aliases_emails_only_with_the_people_scope() {
        let f = fixture();
        let r = get(&f, &read(), "/archive/people");
        assert_eq!(r.status, 200);
        let v = json_of(&r);
        assert_eq!(v["total"], 2);
        assert_eq!(v["emails"], false);
        assert_eq!(v["people"][0], json!({"id": "p-anna", "name": "Anna Rossi", "aliases": []}));
        assert_eq!(v["people"][1], json!({"id": "p-bob", "name": "Bob", "aliases": ["Roberto"]}));
        let body = text_of(&r);
        for forbidden in [PERSON_EMAIL, "\"email\"", "voice", "centroid", "0.25"] {
            assert!(!body.contains(forbidden), "{forbidden} in {body}");
        }
        let full = get(&f, &people_scope(), "/archive/people");
        let v = json_of(&full);
        assert_eq!(v["emails"], true);
        assert_eq!(v["people"][1]["email"], PERSON_EMAIL);
        assert!(!text_of(&full).contains("centroid"), "unknown fields never served");
        // Paged like the items.
        let page = json_of(&get(&f, &read(), "/archive/people?limit=1"));
        assert_eq!(page["people"].as_array().unwrap().len(), 1);
        let next = page["next_cursor"].as_str().unwrap().to_string();
        let rest = json_of(&get(&f, &read(), &format!("/archive/people?limit=1&cursor={next}")));
        assert_eq!(rest["people"][0]["name"], "Bob");
        assert_eq!(rest["next_cursor"], Value::Null);
        assert_eq!(get(&f, &read(), "/archive/people?limit=500").status, 400);
        assert_eq!(get(&f, &read(), "/archive/people?q=bob").status, 400);
        // An unreadable registry says so, without a path.
        std::fs::write(f.archive.join(".sussurro").join("people.json"), "not json").unwrap();
        let broken = get(&f, &read(), "/archive/people");
        assert_eq!((broken.status, code(&broken).as_str()), (422, "unreadable_people"));
        assert!(!text_of(&broken).contains(f.archive.to_str().unwrap()));
    }

    /// Without the `people` scope no route matches or shows an email.
    #[test]
    fn an_email_is_never_an_oracle_without_the_people_scope() {
        let f = fixture();
        for q in [EMAIL, "example", "anna%40example.com"] {
            let hits = |auth: &Authorized| json_of(&get(&f, auth, &format!("/archive/items?q={q}")))["total"].clone();
            assert_eq!(hits(&read()), 0, "{q}");
            assert_eq!(hits(&people_scope()), 1, "{q}");
        }
        let by = |auth: &Authorized| {
            json_of(&get(&f, auth, &format!("/archive/items?participant={EMAIL}")))["total"].clone()
        };
        assert_eq!(by(&read()), 0);
        assert_eq!(by(&people_scope()), 1);
        // Every read-only answer, gathered: no email, no embedding, no path.
        let mut all = String::new();
        for url in [
            "/archive/items?facets=1".to_string(),
            format!("/archive/items/{}", f.meeting),
            format!("/archive/items/{}/export?format=md", f.meeting),
            format!("/archive/items/{}/export?format=txt", f.meeting),
            format!("/archive/items/{}/export?format=vtt", f.meeting),
            format!("/archive/items/{}/documents", f.meeting),
            format!("/archive/items/{}/documents/document.md", f.meeting),
            "/archive/people".to_string(),
        ] {
            let r = get(&f, &read(), &url);
            assert_eq!(r.status, 200, "{url}");
            all.push_str(&text_of(&r));
        }
        let appdata = f.index.parent().unwrap().to_string_lossy().into_owned();
        let root = f.archive.to_string_lossy().into_owned();
        for forbidden in [EMAIL, PERSON_EMAIL, "embedding", "0.12345", "archive-index", &appdata, &root] {
            assert!(!all.contains(forbidden), "{forbidden}");
        }
    }

    #[test]
    fn internal_errors_and_oversized_answers_say_nothing_more() {
        let f = fixture();
        // The index can't be created (its folder is a file): a fixed message.
        let blocked = f.index.parent().unwrap().to_path_buf();
        std::fs::write(&blocked, "a file, not a folder").unwrap();
        let r = get(&f, &read(), "/archive/items");
        assert_eq!((r.status, code(&r).as_str()), (500, "internal"));
        assert!(!text_of(&r).contains(blocked.to_str().unwrap()));
        // An item with broken YAML is there, but unreadable.
        std::fs::write(f.archive.join(&f.note).join("transcript.md"), "---\ntitle: [unclosed\n---\n").unwrap();
        let r = get(&f, &read(), &format!("/archive/items/{}", f.note));
        assert_eq!((r.status, code(&r).as_str()), (422, "unreadable_item"));
        // Bodies over the cap become a 422.
        let big = Reply::ok(json!({"text": "x".repeat(MAX_BODY_BYTES)})).bounded();
        assert_eq!((big.status, code(&big).as_str()), (422, "too_large"));
        let small = Reply::ok(json!({"text": "x"}));
        assert_eq!(small.clone().bounded(), small);
    }
}
