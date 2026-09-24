//! Search index: SQLite FTS5 in the app data dir (`archive-index.sqlite`).
//!
//! The index is derived data (E2): the archive folder is the truth. A
//! missing, corrupt, outdated or foreign (other archive root) database is
//! deleted and rebuilt from the folder. [`Index::sync`] keeps it current
//! incrementally — it only stats files and re-reads items whose
//! `transcript.md` or state changed — so edits made outside the app show up
//! in search without a file watcher.

use super::store::{scan_item_dirs, summary_at, ItemSummary};
use super::types::{ItemMeta, ItemType};
use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension};
use serde::Deserialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

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

pub struct Index {
    conn: Connection,
    archive: PathBuf,
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

fn create_fresh(db_path: &Path, archive: &Path) -> Result<Connection> {
    remove_db_files(db_path);
    if let Some(dir) = db_path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let conn = Connection::open(db_path)
        .with_context(|| format!("creating index {}", db_path.display()))?;
    conn.busy_timeout(std::time::Duration::from_secs(5))?;
    conn.execute_batch(SCHEMA)?;
    conn.execute(
        "INSERT INTO meta (key, value) VALUES ('archive', ?1)",
        params![archive_key(archive)],
    )?;
    conn.pragma_update(None, "user_version", SCHEMA_VERSION)?;
    Ok(conn)
}

impl Index {
    /// Open the index for `archive`, rebuilding it when missing, corrupt,
    /// from another schema version or another archive folder. Does not sync
    /// — call [`Index::sync`] before searching.
    pub fn open(archive: &Path, db_path: &Path) -> Result<Index> {
        let existing = if db_path.is_file() {
            match Connection::open(db_path) {
                Ok(conn) => {
                    let _ = conn.busy_timeout(std::time::Duration::from_secs(5));
                    match validate(&conn, archive) {
                        Ok(()) => Some(conn),
                        Err(e) => {
                            eprintln!("archive index: rebuilding ({e:#})");
                            None
                        }
                    }
                }
                Err(e) => {
                    eprintln!("archive index: rebuilding ({e})");
                    None
                }
            }
        } else {
            None
        };
        let conn = match existing {
            Some(c) => c,
            None => create_fresh(db_path, archive)?,
        };
        Ok(Index {
            conn,
            archive: archive.to_path_buf(),
        })
    }

    /// Bring the index in line with the folder: index new or changed items,
    /// drop rows whose folder is gone. Returns the number of indexed items.
    pub fn sync(&mut self) -> Result<usize> {
        let on_disk = scan_item_dirs(&self.archive);
        let mut known: HashMap<String, String> = {
            let mut stmt = self.conn.prepare("SELECT id, fingerprint FROM items")?;
            let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
            rows.collect::<rusqlite::Result<_>>()?
        };
        let tx = self.conn.transaction()?;
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

    /// (Re)index one item after the app changed it.
    pub fn index_item(&mut self, id: &str) -> Result<()> {
        let dir = super::paths::item_dir(&self.archive, id)?;
        let tx = self.conn.transaction()?;
        index_at(&tx, id, &dir, &fingerprint(&dir))?;
        tx.commit()?;
        Ok(())
    }

    /// Drop one item from the index (after a delete).
    pub fn remove_from_index(&mut self, id: &str) -> Result<()> {
        let tx = self.conn.transaction()?;
        delete_rows(&tx, id)?;
        tx.commit()?;
        Ok(())
    }

    /// Full-text search with facets. An empty query lists every item that
    /// passes the filters, newest first; a text query ranks by relevance
    /// (BM25), then newest first, and fills `snippet` with the best excerpt
    /// (matches wrapped in `**`).
    pub fn search(&self, query: &str, filters: &SearchFilters) -> Result<Vec<ItemSummary>> {
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

        let mut stmt = self.conn.prepare(&sql)?;
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
            out.push(ItemSummary {
                id,
                meta,
                edited_externally: edited,
                snippet: snippet.filter(|s| !s.trim().is_empty()),
            });
        }
        Ok(out)
    }
}

/// Drop the index and rebuild it from the archive folder. Returns the number
/// of items indexed.
pub fn rebuild_index(archive: &Path, db_path: &Path) -> Result<usize> {
    let conn = create_fresh(db_path, archive)?;
    let mut index = Index {
        conn,
        archive: archive.to_path_buf(),
    };
    index.sync()
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
