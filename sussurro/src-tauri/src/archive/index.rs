//! Search index: SQLite FTS5 in the app data dir (`archive-index.sqlite`).
//!
//! The index is derived data (E2): the archive folder is the truth. A
//! missing, outdated or foreign (other archive root) database is emptied and
//! rebuilt from the folder; only a file that is not a database any more is
//! replaced. [`Index::sync`] keeps it current incrementally — it only stats
//! files and re-reads items whose `transcript.md` or state changed — so
//! edits made outside the app show up in search without a file watcher.
//!
//! Concurrency (#155): all handles on one database file share a single
//! connection behind a process-wide lock, and every write is an IMMEDIATE
//! transaction with a busy timeout, so parallel commands (searches, the
//! engine indexing a new item, a rebuild) queue instead of failing.

use super::store::{scan_item_dirs, summary_at, ItemSummary};
use super::types::{ItemMeta, ItemType};
use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde::Deserialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, Weak};

/// Bump when the schema changes: an index with another version is rebuilt.
const SCHEMA_VERSION: i64 = 1;

const SCHEMA: &str = "
CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
CREATE TABLE items (
    id          TEXT PRIMARY KEY,
    item_type   TEXT NOT NULL,
    sort_ts     INTEGER,            -- unix seconds; NULL when the date is unparseable
    day         TEXT,               -- YYYY-MM-DD as written in the frontmatter
    fingerprint TEXT NOT NULL,      -- mtimes + size, drives incremental sync
    edited      INTEGER NOT NULL,
    meta_json   TEXT NOT NULL
);
CREATE TABLE item_tags (id TEXT NOT NULL, tag TEXT NOT NULL);
CREATE TABLE item_categories (id TEXT NOT NULL, category TEXT NOT NULL);
CREATE TABLE item_participants (id TEXT NOT NULL, name TEXT NOT NULL, email TEXT NOT NULL);
CREATE INDEX item_tags_tag ON item_tags (tag COLLATE NOCASE);
CREATE INDEX item_tags_id ON item_tags (id);
CREATE INDEX item_categories_category ON item_categories (category COLLATE NOCASE);
CREATE INDEX item_categories_id ON item_categories (id);
CREATE INDEX item_participants_id ON item_participants (id);
CREATE VIRTUAL TABLE items_fts USING fts5(
    title, body, tags, categories, participants,
    tokenize = 'unicode61 remove_diacritics 2'
);
";

/// Search facets; every set field must match (AND).
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct SearchFilters {
    #[serde(rename = "type")]
    pub item_type: Option<ItemType>,
    /// Exact tag, case-insensitive.
    pub tag: Option<String>,
    /// Exact category, case-insensitive.
    pub category: Option<String>,
    /// Participant name or email, exact, case-insensitive.
    pub participant: Option<String>,
    /// Inclusive lower bound, `YYYY-MM-DD` (an RFC 3339 value is cut to its date).
    pub date_from: Option<String>,
    /// Inclusive upper bound, `YYYY-MM-DD`.
    pub date_to: Option<String>,
}

/// A handle on the search index of one archive folder.
///
/// Handles are cheap: every handle on the same database file shares one
/// SQLite connection behind one process-wide lock (see [`Slot`]), taken for
/// the duration of each call.
pub struct Index {
    /// `Some` until dropped (taken in `Drop`, see [`release`]).
    slot: Option<Shared>,
    archive: PathBuf,
    db_path: PathBuf,
}

/// How long a write waits for another process (e.g. a second app instance)
/// holding the database before giving up with SQLITE_BUSY.
const BUSY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// The one connection to an index file, shared by the whole process (#155).
///
/// Every user — search, sync, per-item reindex, rebuild, the long-form
/// engine — goes through the same `Mutex`, so this process never runs two
/// SQLite writers at once ("database is locked"), and a rebuild never deletes
/// or recreates the file under another open connection. The slot lives as
/// long as some [`Index`] handle on it does; the connection closes with the
/// last one, so the file is not held open while the archive is idle.
struct Slot {
    conn: Option<Connection>,
    /// Archive root the open connection was validated (or rebuilt) for.
    ready_for: Option<PathBuf>,
}

type Shared = Arc<Mutex<Slot>>;
type Registry = Mutex<HashMap<PathBuf, Weak<Mutex<Slot>>>>;

fn registry() -> &'static Registry {
    static REGISTRY: OnceLock<Registry> = OnceLock::new();
    REGISTRY.get_or_init(Default::default)
}

/// A panic mid-operation leaves at worst a rolled-back transaction, so a
/// poisoned lock is still usable.
fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// The shared slot for `db_path`, created on first use.
fn shared_slot(db_path: &Path) -> Shared {
    let key = std::path::absolute(db_path).unwrap_or_else(|_| db_path.to_path_buf());
    let mut reg = lock(registry());
    if let Some(slot) = reg.get(&key).and_then(Weak::upgrade) {
        return slot;
    }
    reg.retain(|_, w| w.strong_count() > 0);
    let slot = Arc::new(Mutex::new(Slot {
        conn: None,
        ready_for: None,
    }));
    reg.insert(key, Arc::downgrade(&slot));
    slot
}

/// Drop a handle while holding the registry lock: if it is the last one, the
/// connection is fully closed before anyone can look the slot up again (and,
/// say, replace a corrupt file that must not be open).
fn release(slot: Shared) {
    let reg = lock(registry());
    drop(slot);
    drop(reg);
}

impl Drop for Index {
    fn drop(&mut self) {
        if let Some(slot) = self.slot.take() {
            release(slot);
        }
    }
}

fn remove_db_files(db_path: &Path) {
    let _ = std::fs::remove_file(db_path);
    for suffix in ["-journal", "-wal", "-shm"] {
        let mut p = db_path.as_os_str().to_owned();
        p.push(suffix);
        let _ = std::fs::remove_file(PathBuf::from(p));
    }
}

fn archive_key(archive: &Path) -> String {
    archive.to_string_lossy().into_owned()
}

/// Is this an index for `archive` with the current schema, and readable?
fn validate(conn: &Connection, archive: &Path) -> Result<()> {
    let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    anyhow::ensure!(version == SCHEMA_VERSION, "schema version {version}");
    let root: Option<String> = conn
        .query_row("SELECT value FROM meta WHERE key = 'archive'", [], |r| {
            r.get(0)
        })
        .optional()?;
    anyhow::ensure!(
        root.as_deref() == Some(&archive_key(archive)),
        "different archive"
    );
    conn.query_row("SELECT count(*) FROM items", [], |r| r.get::<_, i64>(0))?;
    conn.query_row("SELECT count(*) FROM items_fts", [], |r| r.get::<_, i64>(0))?;
    Ok(())
}

fn open_conn(db_path: &Path) -> Result<Connection> {
    if let Some(dir) = db_path.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    }
    let conn = Connection::open(db_path)
        .with_context(|| format!("opening index {}", db_path.display()))?;
    conn.busy_timeout(BUSY_TIMEOUT)?;
    Ok(conn)
}

/// Empty the database in place and set it up for `archive`: every table,
/// view and trigger is dropped and the schema recreated, all in one
/// IMMEDIATE transaction — nothing is deleted from the filesystem.
fn reset_schema(conn: &mut Connection, archive: &Path) -> Result<()> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let mut objects: Vec<(String, String, String)> = {
        let mut stmt = tx.prepare(
            "SELECT type, name, coalesce(sql, '') FROM sqlite_master \
             WHERE type IN ('table', 'view', 'trigger') AND name NOT LIKE 'sqlite\\_%' ESCAPE '\\'",
        )?;
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
        rows.collect::<rusqlite::Result<_>>()?
    };
    // Views and triggers first, then virtual tables (they take their shadow
    // tables with them), then plain tables (their indexes go with them).
    objects.sort_by_key(|(kind, _, sql)| match kind.as_str() {
        "table" if sql.to_ascii_uppercase().starts_with("CREATE VIRTUAL") => 1,
        "table" => 2,
        _ => 0,
    });
    for (kind, name, _) in &objects {
        let kind = kind.to_ascii_uppercase();
        let name = name.replace('"', "\"\"");
        tx.execute_batch(&format!("DROP {kind} IF EXISTS \"{name}\""))?;
    }
    tx.execute_batch(SCHEMA)?;
    tx.execute(
        "INSERT INTO meta (key, value) VALUES ('archive', ?1)",
        params![archive_key(archive)],
    )?;
    tx.pragma_update(None, "user_version", SCHEMA_VERSION)?;
    tx.commit()?;
    Ok(())
}

impl Slot {
    fn close(&mut self) {
        self.conn = None;
        self.ready_for = None;
    }

    /// The connection, ready for `archive`: opened on first use, validated,
    /// and emptied for a rebuild when missing, corrupt, outdated or foreign.
    fn ready(&mut self, archive: &Path, db_path: &Path) -> Result<&mut Connection> {
        if self.conn.is_some() && !db_path.is_file() {
            // Deleted behind our back: start over on a new file.
            self.close();
        }
        if self.ready_for.as_deref() != Some(archive) {
            let existed = db_path.is_file();
            if self.conn.is_none() {
                self.conn = Some(open_conn(db_path)?);
            }
            let conn = self.conn.as_ref().expect("opened above");
            match validate(conn, archive) {
                Ok(()) => self.ready_for = Some(archive.to_path_buf()),
                Err(e) => {
                    if existed {
                        eprintln!("archive index: rebuilding ({e:#})");
                    }
                    self.reset(archive, db_path)?;
                }
            }
        }
        Ok(self.conn.as_mut().expect("ready"))
    }

    /// Empty the index for `archive` (the caller syncs it afterwards). Done
    /// in place on the shared connection; only when that fails — the file is
    /// not a database any more — is the connection closed and the file
    /// replaced. That is safe: every user in this process goes through this
    /// slot, and we hold its lock.
    fn reset(&mut self, archive: &Path, db_path: &Path) -> Result<()> {
        self.ready_for = None;
        if self.conn.is_none() {
            self.conn = Some(open_conn(db_path)?);
        }
        let conn = self.conn.as_mut().expect("opened above");
        if let Err(e) = reset_schema(conn, archive) {
            eprintln!("archive index: replacing {} ({e:#})", db_path.display());
            self.close();
            remove_db_files(db_path);
            let mut conn = open_conn(db_path)?;
            reset_schema(&mut conn, archive)?;
            self.conn = Some(conn);
        }
        self.ready_for = Some(archive.to_path_buf());
        Ok(())
    }
}

impl Index {
    /// Open the index for `archive`, rebuilding it when missing, corrupt,
    /// from another schema version or another archive folder. Does not sync
    /// — call [`Index::sync`] before searching.
    pub fn open(archive: &Path, db_path: &Path) -> Result<Index> {
        let index = Index::handle(archive, db_path);
        index.with_conn(|_| Ok(()))?;
        Ok(index)
    }

    fn handle(archive: &Path, db_path: &Path) -> Index {
        Index {
            slot: Some(shared_slot(db_path)),
            archive: archive.to_path_buf(),
            db_path: db_path.to_path_buf(),
        }
    }

    /// Run `f` holding the shared lock.
    fn with_slot<T>(&self, f: impl FnOnce(&mut Slot) -> Result<T>) -> Result<T> {
        let mut slot = lock(self.slot.as_ref().expect("live index"));
        f(&mut slot)
    }

    /// Run `f` on the ready connection, holding the shared lock.
    fn with_conn<T>(&self, f: impl FnOnce(&mut Connection) -> Result<T>) -> Result<T> {
        self.with_slot(|slot| f(slot.ready(&self.archive, &self.db_path)?))
    }

    /// Bring the index in line with the folder: index new or changed items,
    /// drop rows whose folder is gone. Returns the number of indexed items.
    pub fn sync(&mut self) -> Result<usize> {
        self.with_conn(|conn| sync_conn(conn, &self.archive))
    }

    /// (Re)index one item after the app changed it.
    pub fn index_item(&mut self, id: &str) -> Result<()> {
        let dir = super::paths::item_dir(&self.archive, id)?;
        self.with_conn(|conn| {
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            index_at(&tx, id, &dir, &fingerprint(&dir))?;
            tx.commit()?;
            Ok(())
        })
    }

    /// Drop one item from the index (after a delete).
    pub fn remove_from_index(&mut self, id: &str) -> Result<()> {
        self.with_conn(|conn| {
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            delete_rows(&tx, id)?;
            tx.commit()?;
            Ok(())
        })
    }

    /// Full-text search with facets. An empty query lists every item that
    /// passes the filters, newest first; a text query ranks by relevance
    /// (BM25), then newest first, and fills `snippet` with the best excerpt
    /// (matches wrapped in `**`). Each row carries the item's external-send
    /// hosts (#122), read from its folder.
    pub fn search(&self, query: &str, filters: &SearchFilters) -> Result<Vec<ItemSummary>> {
        let rows = self.with_conn(|conn| search_conn(conn, query, filters))?;
        Ok(rows.into_iter().map(|s| s.with_external_hosts(&self.archive)).collect())
    }
}

fn sync_conn(conn: &mut Connection, archive: &Path) -> Result<usize> {
    let on_disk = scan_item_dirs(archive);
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let mut known: HashMap<String, String> = {
        let mut stmt = tx.prepare("SELECT id, fingerprint FROM items")?;
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
        rows.collect::<rusqlite::Result<_>>()?
    };
    let mut count = 0;
    for (id, dir) in &on_disk {
        let fp = fingerprint(dir);
        if known.remove(id).as_deref() == Some(fp.as_str()) {
            count += 1;
            continue;
        }
        match index_at(&tx, id, dir, &fp) {
            Ok(()) => count += 1,
            Err(e) => {
                eprintln!("archive index: skipping broken item {id}: {e:#}");
                delete_rows(&tx, id)?;
            }
        }
    }
    for gone in known.keys() {
        delete_rows(&tx, gone)?;
    }
    tx.commit()?;
    Ok(count)
}

fn search_conn(
    conn: &Connection,
    query: &str,
    filters: &SearchFilters,
) -> Result<Vec<ItemSummary>> {
    let fts = fts_query(query);
    let mut sql = String::from("SELECT i.id, i.meta_json, i.edited, ");
    let mut args: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
    if let Some(q) = &fts {
        sql.push_str(
            "snippet(items_fts, 1, '**', '**', '…', 16) \
             FROM items_fts JOIN items i ON i.rowid = items_fts.rowid \
             WHERE items_fts MATCH ?",
        );
        args.push(Box::new(q.clone()));
    } else {
        sql.push_str("NULL FROM items i WHERE 1");
    }
    if let Some(t) = filters.item_type {
        sql.push_str(" AND i.item_type = ?");
        args.push(Box::new(t.as_str()));
    }
    if let Some(tag) = nonempty(&filters.tag) {
        sql.push_str(
            " AND EXISTS (SELECT 1 FROM item_tags t WHERE t.id = i.id AND t.tag = ? COLLATE NOCASE)",
        );
        args.push(Box::new(tag));
    }
    if let Some(cat) = nonempty(&filters.category) {
        sql.push_str(
            " AND EXISTS (SELECT 1 FROM item_categories c WHERE c.id = i.id \
             AND c.category = ? COLLATE NOCASE)",
        );
        args.push(Box::new(cat));
    }
    if let Some(p) = nonempty(&filters.participant) {
        sql.push_str(
            " AND EXISTS (SELECT 1 FROM item_participants p WHERE p.id = i.id \
             AND (p.name = ? COLLATE NOCASE OR p.email = ? COLLATE NOCASE))",
        );
        args.push(Box::new(p.clone()));
        args.push(Box::new(p));
    }
    if let Some(from) = day_bound(&filters.date_from)? {
        sql.push_str(" AND i.day >= ?");
        args.push(Box::new(from));
    }
    if let Some(to) = day_bound(&filters.date_to)? {
        sql.push_str(" AND i.day <= ?");
        args.push(Box::new(to));
    }
    if fts.is_some() {
        sql.push_str(" ORDER BY bm25(items_fts, 10.0, 1.0, 5.0, 5.0, 5.0), ");
    } else {
        sql.push_str(" ORDER BY ");
    }
    sql.push_str("i.sort_ts IS NULL, i.sort_ts DESC, i.id DESC");

    let mut stmt = conn.prepare(&sql)?;
    let params: Vec<&dyn rusqlite::ToSql> = args.iter().map(|b| b.as_ref()).collect();
    let rows = stmt.query_map(params.as_slice(), |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, bool>(2)?,
            r.get::<_, Option<String>>(3)?,
        ))
    })?;
    let mut out = Vec::new();
    for row in rows {
        let (id, meta_json, edited, snippet) = row?;
        let meta: ItemMeta =
            serde_json::from_str(&meta_json).with_context(|| format!("index row for {id}"))?;
        out.push(ItemSummary::new(
            id,
            meta,
            edited,
            snippet.filter(|s| !s.trim().is_empty()),
        ));
    }
    Ok(out)
}

/// Empty the index and rebuild it from the archive folder. Returns the
/// number of items indexed. Runs under the shared lock, so concurrent
/// searches simply wait for it instead of failing.
pub fn rebuild_index(archive: &Path, db_path: &Path) -> Result<usize> {
    let index = Index::handle(archive, db_path);
    index.with_slot(|slot| {
        slot.reset(archive, db_path)?;
        sync_conn(slot.ready(archive, db_path)?, archive)
    })
}

/// Run `f` on a synced index; if SQLite reports the file itself is broken
/// mid-operation, rebuild once and retry.
pub fn with_index<T>(
    archive: &Path,
    db_path: &Path,
    f: impl Fn(&mut Index) -> Result<T>,
) -> Result<T> {
    let attempt = || -> Result<T> {
        let mut index = Index::open(archive, db_path)?;
        index.sync()?;
        f(&mut index)
    };
    match attempt() {
        Err(e) if is_corruption(&e) => {
            eprintln!("archive index: corrupt, rebuilding ({e:#})");
            rebuild_index(archive, db_path)?;
            attempt()
        }
        other => other,
    }
}

fn is_corruption(e: &anyhow::Error) -> bool {
    e.chain().any(|c| {
        matches!(
            c.downcast_ref::<rusqlite::Error>(),
            Some(rusqlite::Error::SqliteFailure(f, _))
                if matches!(
                    f.code,
                    rusqlite::ErrorCode::DatabaseCorrupt | rusqlite::ErrorCode::NotADatabase
                )
        )
    })
}

fn nonempty(v: &Option<String>) -> Option<String> {
    v.as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// Normalize a date filter to `YYYY-MM-DD`.
fn day_bound(v: &Option<String>) -> Result<Option<String>> {
    let Some(s) = nonempty(v) else {
        return Ok(None);
    };
    let day = s.get(..10).unwrap_or(&s);
    chrono::NaiveDate::parse_from_str(day, "%Y-%m-%d")
        .with_context(|| format!("invalid date filter '{s}' (expected YYYY-MM-DD)"))?;
    Ok(Some(day.to_string()))
}

/// Turn free user text into a safe FTS5 query: each word becomes a quoted
/// prefix term (`"word"*`), terms are ANDed. FTS5 operators typed by the
/// user are treated as text, so no input can produce a syntax error.
/// `None` when nothing searchable is left.
pub fn fts_query(input: &str) -> Option<String> {
    let terms: Vec<String> = input
        .split_whitespace()
        .filter(|w| w.chars().any(char::is_alphanumeric))
        .map(|w| format!("\"{}\"*", w.replace('"', "\"\"")))
        .collect();
    (!terms.is_empty()).then(|| terms.join(" "))
}

fn mtime_ns(path: &Path) -> String {
    std::fs::metadata(path)
        .ok()
        .map(|m| {
            let t = m
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_nanos())
                .unwrap_or(0);
            format!("{t}:{}", m.len())
        })
        .unwrap_or_else(|| "-".to_string())
}

/// Change detector for an item folder: transcript.md and state.json
/// (the "edited outside" flag depends on both).
fn fingerprint(dir: &Path) -> String {
    format!(
        "{}|{}",
        mtime_ns(&dir.join(super::store::TRANSCRIPT_FILE)),
        mtime_ns(
            &dir.join(super::store::META_DIR)
                .join(super::store::STATE_FILE)
        )
    )
}

fn delete_rows(conn: &Connection, id: &str) -> Result<()> {
    conn.execute(
        "DELETE FROM items_fts WHERE rowid = (SELECT rowid FROM items WHERE id = ?1)",
        params![id],
    )?;
    conn.execute("DELETE FROM items WHERE id = ?1", params![id])?;
    conn.execute("DELETE FROM item_tags WHERE id = ?1", params![id])?;
    conn.execute("DELETE FROM item_categories WHERE id = ?1", params![id])?;
    conn.execute("DELETE FROM item_participants WHERE id = ?1", params![id])?;
    Ok(())
}

fn index_at(conn: &Connection, id: &str, dir: &Path, fp: &str) -> Result<()> {
    let (summary, body) = summary_at(id, dir)?;
    let meta = &summary.meta;
    let parsed = chrono::DateTime::parse_from_rfc3339(meta.date.trim()).ok();
    let sort_ts = parsed.map(|d| d.timestamp());
    let day = parsed.map(|d| d.date_naive().format("%Y-%m-%d").to_string());
    delete_rows(conn, id)?;
    conn.execute(
        "INSERT INTO items (id, item_type, sort_ts, day, fingerprint, edited, meta_json)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            id,
            meta.item_type.as_str(),
            sort_ts,
            day,
            fp,
            summary.edited_externally,
            serde_json::to_string(meta)?
        ],
    )?;
    let rowid = conn.last_insert_rowid();
    let participants: Vec<String> = meta
        .participants
        .iter()
        .map(|p| format!("{} {}", p.name, p.email.as_deref().unwrap_or("")))
        .collect();
    conn.execute(
        "INSERT INTO items_fts (rowid, title, body, tags, categories, participants)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            rowid,
            meta.title,
            body,
            meta.tags.join(" "),
            meta.categories.join(" "),
            participants.join(" ")
        ],
    )?;
    for tag in &meta.tags {
        conn.execute(
            "INSERT INTO item_tags (id, tag) VALUES (?1, ?2)",
            params![id, tag],
        )?;
    }
    for c in &meta.categories {
        conn.execute(
            "INSERT INTO item_categories (id, category) VALUES (?1, ?2)",
            params![id, c],
        )?;
    }
    for p in &meta.participants {
        conn.execute(
            "INSERT INTO item_participants (id, name, email) VALUES (?1, ?2, ?3)",
            params![id, p.name, p.email.as_deref().unwrap_or("")],
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::archive::store::{create_item, delete_item_with, update_meta};
    use crate::archive::types::{Participant, Segment, SegmentsFile};

    fn seg(text: &str) -> SegmentsFile {
        SegmentsFile {
            segments: vec![Segment {
                id: 0,
                start_ms: 0,
                end_ms: 1000,
                raw: text.into(),
                text: text.into(),
                ..Default::default()
            }],
            ..Default::default()
        }
    }

    fn add(archive: &Path, t: ItemType, title: &str, date: &str, text: &str) -> String {
        let meta = ItemMeta {
            item_type: t,
            title: title.into(),
            date: date.into(),
            tags: vec!["Release".into()],
            categories: if t == ItemType::Meeting {
                vec!["team".into()]
            } else {
                vec![]
            },
            participants: if t == ItemType::Meeting {
                vec![Participant {
                    name: "Anna Rossi".into(),
                    email: Some("anna@example.com".into()),
                }]
            } else {
                vec![]
            },
            ..Default::default()
        };
        create_item(archive, &meta, &seg(text)).unwrap()
    }

    struct Fixture {
        _tmp: tempfile::TempDir,
        archive: PathBuf,
        db: PathBuf,
        note: String,
        meeting: String,
        podcast: String,
    }

    fn fixture() -> Fixture {
        let tmp = tempfile::tempdir().unwrap();
        let archive = tmp.path().join("Sussurro");
        let db = tmp.path().join("appdata").join("archive-index.sqlite");
        let note = add(
            &archive,
            ItemType::Note,
            "Idea caffè",
            "2026-01-10T08:00:00+01:00",
            "Comprare il caffè perché è finito.",
        );
        let meeting = add(
            &archive,
            ItemType::Meeting,
            "Weekly sync",
            "2026-03-02T10:00:00+01:00",
            "Parliamo della release e del roadmap.",
        );
        let podcast = add(
            &archive,
            ItemType::Transcription,
            "Podcast",
            "2025-12-31T23:00:00Z",
            "Intervista sulla release di fine anno.",
        );
        Fixture {
            _tmp: tmp,
            archive,
            db,
            note,
            meeting,
            podcast,
        }
    }

    fn ids(v: Vec<ItemSummary>) -> Vec<String> {
        v.into_iter().map(|s| s.id).collect()
    }

    #[test]
    fn empty_query_lists_everything_newest_first() {
        let f = fixture();
        assert_eq!(rebuild_index(&f.archive, &f.db).unwrap(), 3);
        let idx = Index::open(&f.archive, &f.db).unwrap();
        let all = idx.search("  ", &SearchFilters::default()).unwrap();
        assert_eq!(
            ids(all),
            vec![f.meeting.clone(), f.note.clone(), f.podcast.clone()]
        );
    }

    #[test]
    fn text_search_folds_accents_prefixes_and_snippets() {
        let f = fixture();
        rebuild_index(&f.archive, &f.db).unwrap();
        let idx = Index::open(&f.archive, &f.db).unwrap();
        let hits = idx
            .search("perche caff", &SearchFilters::default())
            .unwrap();
        assert_eq!(ids(hits.clone()), vec![f.note.clone()]);
        assert!(
            hits[0].snippet.as_deref().unwrap().contains("**"),
            "{hits:?}"
        );
        // Title, tags and participants are searchable too.
        assert_eq!(
            ids(idx.search("weekly", &Default::default()).unwrap()),
            vec![f.meeting.clone()]
        );
        assert_eq!(
            ids(idx.search("anna", &Default::default()).unwrap()),
            vec![f.meeting.clone()]
        );
        // FTS syntax typed by the user is harmless.
        for q in ["\"", "release OR", "NEAR(", "-", "a AND \"b", "*"] {
            idx.search(q, &Default::default()).unwrap();
        }
    }

    #[test]
    fn filters_combine() {
        let f = fixture();
        rebuild_index(&f.archive, &f.db).unwrap();
        let idx = Index::open(&f.archive, &f.db).unwrap();
        let by = |filters: SearchFilters| ids(idx.search("", &filters).unwrap());
        assert_eq!(
            by(SearchFilters {
                item_type: Some(ItemType::Note),
                ..Default::default()
            }),
            vec![f.note.clone()]
        );
        assert_eq!(
            by(SearchFilters {
                tag: Some("release".into()),
                ..Default::default()
            })
            .len(),
            3
        );
        assert_eq!(
            by(SearchFilters {
                category: Some("TEAM".into()),
                ..Default::default()
            }),
            vec![f.meeting.clone()]
        );
        assert_eq!(
            by(SearchFilters {
                participant: Some("anna@example.com".into()),
                ..Default::default()
            }),
            vec![f.meeting.clone()]
        );
        assert_eq!(
            by(SearchFilters {
                date_from: Some("2026-01-01".into()),
                date_to: Some("2026-02-28T00:00:00Z".into()),
                ..Default::default()
            }),
            vec![f.note.clone()]
        );
        // The podcast's day is the one written (Dec 31), not local/UTC shifted.
        assert_eq!(
            by(SearchFilters {
                date_to: Some("2025-12-31".into()),
                ..Default::default()
            }),
            vec![f.podcast.clone()]
        );
        let hits = idx
            .search(
                "release",
                &SearchFilters {
                    item_type: Some(ItemType::Meeting),
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(ids(hits), vec![f.meeting.clone()]);
        assert!(idx
            .search(
                "",
                &SearchFilters {
                    date_from: Some("yesterday".into()),
                    ..Default::default()
                }
            )
            .is_err());
    }

    #[test]
    fn sync_follows_external_edits_and_deletions() {
        let f = fixture();
        with_index(&f.archive, &f.db, |i| i.search("", &Default::default())).unwrap();

        // Edit outside the app.
        let path = f.archive.join(&f.podcast).join("transcript.md");
        let doc = std::fs::read_to_string(&path)
            .unwrap()
            .replace("Intervista", "Chiacchierata zanzibar");
        std::fs::write(&path, doc).unwrap();
        // Delete outside the app.
        std::fs::remove_dir_all(f.archive.join(&f.note)).unwrap();

        let hits = with_index(&f.archive, &f.db, |i| {
            i.search("zanzibar", &Default::default())
        })
        .unwrap();
        assert_eq!(hits.len(), 1);
        assert!(hits[0].edited_externally);
        let all = with_index(&f.archive, &f.db, |i| i.search("", &Default::default())).unwrap();
        assert_eq!(ids(all), vec![f.meeting.clone(), f.podcast.clone()]);
    }

    #[test]
    fn index_item_and_remove_from_index() {
        let f = fixture();
        let mut idx = Index::open(&f.archive, &f.db).unwrap();
        idx.sync().unwrap();
        let mut meta = crate::archive::store::read_item(&f.archive, &f.note)
            .unwrap()
            .meta;
        meta.tags = vec!["urgente".into()];
        update_meta(&f.archive, &f.note, &meta).unwrap();
        idx.index_item(&f.note).unwrap();
        let tagged = idx
            .search(
                "",
                &SearchFilters {
                    tag: Some("urgente".into()),
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(ids(tagged), vec![f.note.clone()]);

        delete_item_with(&f.archive, &f.note, |p| Ok(std::fs::remove_dir_all(p)?)).unwrap();
        idx.remove_from_index(&f.note).unwrap();
        assert_eq!(idx.search("caffè", &Default::default()).unwrap().len(), 0);
        assert!(idx.index_item("../escape").is_err());
    }

    #[test]
    fn corrupt_missing_or_foreign_index_is_rebuilt() {
        let f = fixture();
        // Garbage file where the db should be.
        std::fs::create_dir_all(f.db.parent().unwrap()).unwrap();
        std::fs::write(&f.db, b"definitely not sqlite").unwrap();
        let n = with_index(&f.archive, &f.db, |i| {
            Ok(i.search("", &Default::default())?.len())
        })
        .unwrap();
        assert_eq!(n, 3);

        // Index built for another archive folder: rebuilt for this one.
        let other = f.archive.with_file_name("Other");
        std::fs::create_dir_all(&other).unwrap();
        let n = with_index(&other, &f.db, |i| {
            Ok(i.search("", &Default::default())?.len())
        })
        .unwrap();
        assert_eq!(n, 0);
        let n = with_index(&f.archive, &f.db, |i| {
            Ok(i.search("", &Default::default())?.len())
        })
        .unwrap();
        assert_eq!(n, 3);

        // Deleted: recreated.
        std::fs::remove_file(&f.db).unwrap();
        assert_eq!(rebuild_index(&f.archive, &f.db).unwrap(), 3);
    }

    #[test]
    fn broken_items_are_skipped_by_the_index() {
        let f = fixture();
        let broken = f.archive.join("2026/03/broken");
        std::fs::create_dir_all(&broken).unwrap();
        std::fs::write(broken.join("transcript.md"), "---\ntitle: [oops\n---\n").unwrap();
        assert_eq!(rebuild_index(&f.archive, &f.db).unwrap(), 3);
    }

    #[test]
    fn parallel_searches_updates_and_rebuilds_never_error() {
        let f = fixture();
        let search = || {
            with_index(&f.archive, &f.db, |i| {
                i.search("release", &Default::default())
            })
        };
        std::thread::scope(|s| {
            // Searches: each one syncs first, so they write too.
            for _ in 0..6 {
                s.spawn(|| {
                    for _ in 0..20 {
                        search().unwrap();
                    }
                });
            }
            // Rebuilds under them (on Windows this used to fail on
            // remove_file, then with "table meta already exists").
            s.spawn(|| {
                for _ in 0..10 {
                    assert_eq!(rebuild_index(&f.archive, &f.db).unwrap(), 3);
                }
            });
            // App edits + per-item reindex, which change fingerprints and
            // make the concurrent syncs rewrite rows.
            s.spawn(|| {
                for n in 0..15 {
                    let mut meta = crate::archive::store::read_item(&f.archive, &f.note)
                        .unwrap()
                        .meta;
                    meta.tags = vec!["Release".into(), format!("giro{n}")];
                    update_meta(&f.archive, &f.note, &meta).unwrap();
                    Index::open(&f.archive, &f.db)
                        .unwrap()
                        .index_item(&f.note)
                        .unwrap();
                }
            });
        });
        let hits = with_index(&f.archive, &f.db, |i| {
            i.search(
                "",
                &SearchFilters {
                    tag: Some("giro14".into()),
                    ..Default::default()
                },
            )
        })
        .unwrap();
        assert_eq!(ids(hits), vec![f.note.clone()]);
    }

    #[test]
    fn rebuild_empties_in_place_under_an_open_handle() {
        let f = fixture();
        let idx = Index::open(&f.archive, &f.db).unwrap();
        assert!(idx.search("", &Default::default()).unwrap().is_empty());
        // A rebuild while a handle is open neither fails nor strands it.
        assert_eq!(rebuild_index(&f.archive, &f.db).unwrap(), 3);
        assert_eq!(idx.search("", &Default::default()).unwrap().len(), 3);
        assert_eq!(rebuild_index(&f.archive, &f.db).unwrap(), 3);
        assert_eq!(idx.search("release", &Default::default()).unwrap().len(), 3);
    }

    #[test]
    fn fts_query_quotes_terms() {
        assert_eq!(
            fts_query("ciao mondo").as_deref(),
            Some("\"ciao\"* \"mondo\"*")
        );
        assert_eq!(fts_query("a\"b").as_deref(), Some("\"a\"\"b\"*"));
        assert_eq!(fts_query(" - * "), None);
        assert_eq!(fts_query(""), None);
    }
}
