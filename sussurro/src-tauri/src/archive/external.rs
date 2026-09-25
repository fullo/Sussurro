//! What left the machine, per item (0.8, #122; plan §4.5): the Library
//! marks items whose text was sent to an external LLM host.
//!
//! ```text
//! <item>/.sussurro/external-log.json   one entry per external send
//! ```
//!
//! Each entry records when, where (host, profile, model) and why (recipe,
//! question or cleanup) — **never the content**, not even the question.
//! An entry is written *before* the text is sent (a run that then fails or
//! is cancelled may still have sent part of it), and a run whose entry
//! can't be written is not started.
//!
//! The marker is derived from this log plus the provenance of the companion
//! documents (`external: true`, `host:`), so a document copied in from
//! elsewhere still counts.

use super::companion::external_hosts_at;
use super::store::{existing_item_dir, lock_items, write_atomic, META_DIR};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Log file inside the item's `.sussurro/`.
pub const EXTERNAL_LOG_FILE: &str = "external-log.json";
/// Entries kept at most (oldest dropped first): the marker only needs the
/// hosts, the log is not an audit trail of every run forever.
const MAX_ENTRIES: usize = 500;
/// Shown for a document marked external that doesn't say where it went.
pub const UNKNOWN_HOST: &str = "an unknown host";

/// Why the text was sent.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum SendKind {
    /// A recipe (companion document or answer recipe).
    #[default]
    Recipe,
    /// A free question from the Ask panel.
    Question,
    /// Cleanup of a long-form run (mic session or file).
    Cleanup,
}

/// One external send. Lenient on read: a hand-edited entry never hides the
/// others.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(default)]
pub struct ExternalSend {
    /// RFC 3339.
    pub date: String,
    pub host: String,
    /// Profile name as shown in the app.
    pub profile: String,
    pub model: String,
    pub kind: SendKind,
    /// Recipe id, for a recipe run; empty otherwise.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub recipe: String,
}

fn log_path(dir: &Path) -> PathBuf {
    dir.join(META_DIR).join(EXTERNAL_LOG_FILE)
}

/// The log of the item folder `dir` (empty when absent or unreadable).
pub fn read_log_at(dir: &Path) -> Vec<ExternalSend> {
    std::fs::read_to_string(log_path(dir))
        .ok()
        .and_then(|s| serde_json::from_str::<Vec<serde_json::Value>>(&s).ok())
        .map(|v| {
            v.into_iter()
                .filter_map(|e| serde_json::from_value(e).ok())
                .collect()
        })
        .unwrap_or_default()
}

/// The log of item `id`.
pub fn read_log(archive: &Path, id: &str) -> Result<Vec<ExternalSend>> {
    Ok(read_log_at(&existing_item_dir(archive, id)?))
}

/// Append `entry` to item `id`'s log. Called before the text is sent; an
/// error means the run must not start.
pub fn record(archive: &Path, id: &str, entry: &ExternalSend) -> Result<()> {
    let _lock = lock_items();
    let dir = existing_item_dir(archive, id)?;
    let mut log = read_log_at(&dir);
    log.push(entry.clone());
    if log.len() > MAX_ENTRIES {
        log.drain(..log.len() - MAX_ENTRIES);
    }
    std::fs::create_dir_all(dir.join(META_DIR))
        .with_context(|| format!("creating {}", dir.join(META_DIR).display()))?;
    write_atomic(
        &log_path(&dir),
        serde_json::to_string_pretty(&log)?.as_bytes(),
    )
    .context("recording the external send")
}

/// Every host the item in folder `dir` was sent to, from its log and its
/// companions' provenance: lowercased, unique, sorted; a document marked
/// external without a host counts as [`UNKNOWN_HOST`]. Empty = never sent.
pub fn sent_hosts_at(dir: &Path) -> Vec<String> {
    let hosts: std::collections::BTreeSet<String> = read_log_at(dir)
        .into_iter()
        .map(|e| e.host.trim().to_lowercase())
        .chain(external_hosts_at(dir))
        .map(|h| {
            if h.is_empty() {
                UNKNOWN_HOST.to_string()
            } else {
                h
            }
        })
        .collect();
    hosts.into_iter().collect()
}

/// [`sent_hosts_at`] for item `id`.
pub fn sent_hosts(archive: &Path, id: &str) -> Result<Vec<String>> {
    Ok(sent_hosts_at(&existing_item_dir(archive, id)?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::archive::companion::{write_companion, CompanionMeta};
    use crate::archive::store::create_item;
    use crate::archive::types::{ItemMeta, SegmentsFile};

    fn item(archive: &Path) -> String {
        let meta = ItemMeta {
            title: "Idee".into(),
            date: "2026-09-24T10:00:00+02:00".into(),
            ..Default::default()
        };
        create_item(archive, &meta, &SegmentsFile::default()).unwrap()
    }

    fn send(host: &str, kind: SendKind) -> ExternalSend {
        ExternalSend {
            date: "2026-09-24T12:00:00+02:00".into(),
            host: host.into(),
            profile: "Work".into(),
            model: "gpt-4o-mini".into(),
            kind,
            recipe: if kind == SendKind::Recipe {
                "summary".into()
            } else {
                String::new()
            },
        }
    }

    #[test]
    fn a_fresh_item_was_never_sent() {
        let tmp = tempfile::tempdir().unwrap();
        let archive = tmp.path().join("Sussurro");
        let id = item(&archive);
        assert!(read_log(&archive, &id).unwrap().is_empty());
        assert!(sent_hosts(&archive, &id).unwrap().is_empty());
        assert!(sent_hosts(&archive, "2026/09/nope").is_err());
    }

    #[test]
    fn the_log_records_metadata_only() {
        let tmp = tempfile::tempdir().unwrap();
        let archive = tmp.path().join("Sussurro");
        let id = item(&archive);
        record(&archive, &id, &send("api.example.com", SendKind::Recipe)).unwrap();
        record(&archive, &id, &send("api.example.com", SendKind::Question)).unwrap();
        let log = read_log(&archive, &id).unwrap();
        assert_eq!(log.len(), 2);
        assert_eq!(log[0].recipe, "summary");
        assert_eq!(log[1].kind, SendKind::Question);
        let raw =
            std::fs::read_to_string(archive.join(&id).join(".sussurro/external-log.json")).unwrap();
        // Exactly these keys: no text, no question.
        let v: Vec<serde_json::Map<String, serde_json::Value>> =
            serde_json::from_str(&raw).unwrap();
        // Sorted: serde_json keeps insertion order when a dependency enables
        // its `preserve_order` feature (as happens on Linux CI).
        let keys: Vec<Vec<&str>> = v
            .iter()
            .map(|e| {
                let mut k: Vec<&str> = e.keys().map(String::as_str).collect();
                k.sort_unstable();
                k
            })
            .collect();
        assert_eq!(
            keys[0],
            ["date", "host", "kind", "model", "profile", "recipe"]
        );
        assert_eq!(keys[1], ["date", "host", "kind", "model", "profile"]);
        assert_eq!(v[1]["kind"], "question");
    }

    #[test]
    fn the_log_is_bounded_and_lenient() {
        let tmp = tempfile::tempdir().unwrap();
        let archive = tmp.path().join("Sussurro");
        let id = item(&archive);
        let path = archive.join(&id).join(".sussurro/external-log.json");
        // A hand-edited file with a broken entry keeps the good ones.
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            r#"[{"host":"a.example"}, 42, {"host":"b.example","kind":"cleanup"}]"#,
        )
        .unwrap();
        let log = read_log(&archive, &id).unwrap();
        assert_eq!(log.len(), 2);
        assert_eq!(log[1].kind, SendKind::Cleanup);
        for _ in 0..MAX_ENTRIES + 5 {
            record(&archive, &id, &send("c.example", SendKind::Cleanup)).unwrap();
        }
        let log = read_log(&archive, &id).unwrap();
        assert_eq!(log.len(), MAX_ENTRIES);
        assert!(
            log.iter().all(|e| e.host == "c.example"),
            "the oldest entries go first"
        );
        // Garbage: nothing, not an error.
        std::fs::write(&path, "not json").unwrap();
        assert!(read_log(&archive, &id).unwrap().is_empty());
    }

    #[test]
    fn marker_hosts_come_from_the_log_and_the_companions() {
        let tmp = tempfile::tempdir().unwrap();
        let archive = tmp.path().join("Sussurro");
        let id = item(&archive);
        record(&archive, &id, &send("API.example.com", SendKind::Cleanup)).unwrap();
        record(&archive, &id, &send("api.example.com", SendKind::Recipe)).unwrap();
        let ext = CompanionMeta {
            recipe: "summary".into(),
            external: true,
            host: "llm.other.example".into(),
            ..Default::default()
        };
        write_companion(&archive, &id, "summary.md", &ext, "x").unwrap();
        // A local companion adds nothing.
        let local = CompanionMeta {
            recipe: "decisions".into(),
            ..Default::default()
        };
        write_companion(&archive, &id, "decisions.md", &local, "x").unwrap();
        assert_eq!(
            sent_hosts(&archive, &id).unwrap(),
            ["api.example.com", "llm.other.example"]
        );
        // A document marked external by another tool, with no host.
        std::fs::write(
            archive.join(&id).join("imported.md"),
            "---\nexternal: true\n---\nx",
        )
        .unwrap();
        assert_eq!(
            sent_hosts(&archive, &id).unwrap(),
            ["an unknown host", "api.example.com", "llm.other.example"]
        );
    }

    #[test]
    fn list_search_and_read_carry_the_marker() {
        let tmp = tempfile::tempdir().unwrap();
        let archive = tmp.path().join("Sussurro");
        let id = item(&archive);
        let other = create_item(
            &archive,
            &ItemMeta {
                title: "Local only".into(),
                date: "2026-09-23T10:00:00+02:00".into(),
                ..Default::default()
            },
            &SegmentsFile::default(),
        )
        .unwrap();
        record(&archive, &id, &send("api.example.com", SendKind::Question)).unwrap();
        let hosts = |v: &[crate::archive::ItemSummary], id: &str| {
            v.iter()
                .find(|s| s.id == id)
                .unwrap()
                .external_hosts
                .clone()
        };
        let listed = crate::archive::list_items(&archive);
        assert_eq!(hosts(&listed, &id), ["api.example.com"]);
        assert!(hosts(&listed, &other).is_empty());
        let db = tmp.path().join("index.sqlite");
        let found =
            crate::archive::with_index(&archive, &db, |idx| idx.search("", &Default::default()))
                .unwrap();
        assert_eq!(hosts(&found, &id), ["api.example.com"]);
        assert!(hosts(&found, &other).is_empty());
        assert_eq!(
            crate::archive::read_item(&archive, &id)
                .unwrap()
                .external_hosts,
            ["api.example.com"]
        );
        // Serialized for the UI.
        let json = serde_json::to_value(&listed).unwrap();
        assert!(json
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s["external_hosts"] == serde_json::json!(["api.example.com"])));
    }

    #[test]
    fn companions_alone_mark_an_item() {
        let tmp = tempfile::tempdir().unwrap();
        let archive = tmp.path().join("Sussurro");
        let id = item(&archive);
        let ext = CompanionMeta {
            recipe: "summary".into(),
            external: true,
            host: "x.example".into(),
            ..Default::default()
        };
        write_companion(&archive, &id, "summary.md", &ext, "x").unwrap();
        assert_eq!(sent_hosts(&archive, &id).unwrap(), ["x.example"]);
    }
}
