//! Where the audio of a file transcription came from (#134), so "Identify
//! voices" can run after the fact: the audio itself is never kept (P9), so
//! voices can only be found again from the original file.
//!
//! Privacy: the full path is a local detail (it names the user's folders),
//! so it never goes into the archive — not in the frontmatter (which only
//! says `source: file:<name>`), not in the item's `.sussurro/`, not in an
//! export. It lives in `source-files.json` in the app data dir, on this
//! machine only, next to the dictation history. Only transcriptions are
//! recorded (notes never get speakers, P10); entries of items that no
//! longer exist are dropped at the next write.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// File name in the app data dir.
pub const FILE: &str = "source-files.json";

/// One transcription's original file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    /// The archive folder the item lives in (ids are per archive).
    pub archive: PathBuf,
    pub id: String,
    pub path: PathBuf,
    /// Size when it was transcribed: a file replaced since then (same
    /// name, other audio) must not lend its voices to the transcript.
    pub bytes: u64,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Store {
    #[serde(default)]
    items: Vec<Entry>,
}

/// Read-modify-write of the store is serialized in this process.
static LOCK: Mutex<()> = Mutex::new(());

fn read(store: &Path) -> Store {
    std::fs::read(store)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

fn write(store: &Path, s: &Store) -> Result<()> {
    if let Some(dir) = store.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    }
    let bytes = serde_json::to_vec_pretty(s)?;
    crate::archive::store::write_atomic(store, &bytes)
}

fn item_exists(e: &Entry) -> bool {
    crate::archive::paths::item_dir(&e.archive, &e.id).is_ok_and(|d| d.is_dir())
}

/// Remember that item `id` of `archive` was transcribed from `source`.
pub fn record(store: &Path, archive: &Path, id: &str, source: &Path) -> Result<()> {
    let path = std::fs::canonicalize(source).unwrap_or_else(|_| source.to_path_buf());
    let bytes = std::fs::metadata(&path)
        .with_context(|| format!("reading {}", path.display()))?
        .len();
    let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut s = read(store);
    s.items
        .retain(|e| !(e.archive == archive && e.id == id) && item_exists(e));
    s.items.push(Entry {
        archive: archive.to_path_buf(),
        id: id.to_string(),
        path,
        bytes,
    });
    write(store, &s)
}

/// The original file recorded for item `id` of `archive`, if any.
pub fn lookup(store: &Path, archive: &Path, id: &str) -> Option<Entry> {
    let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    read(store)
        .items
        .into_iter()
        .find(|e| e.archive == archive && e.id == id)
}

/// Forget item `id` of `archive` (it was deleted). A missing store is fine.
pub fn forget(store: &Path, archive: &Path, id: &str) -> Result<()> {
    let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    if !store.exists() {
        return Ok(());
    }
    let mut s = read(store);
    let before = s.items.len();
    s.items.retain(|e| !(e.archive == archive && e.id == id));
    if s.items.len() == before {
        return Ok(());
    }
    write(store, &s)
}

/// Whether the recorded file can still give back the item's voices.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileState {
    /// Same place, same size.
    Available,
    /// Nothing at the recorded path any more.
    Missing,
    /// Something else is there now (other size).
    Changed,
}

/// Checks the file behind `entry`.
pub fn check(entry: &Entry) -> FileState {
    match std::fs::metadata(&entry.path) {
        Ok(m) if m.is_file() && m.len() == entry.bytes => FileState::Available,
        Ok(m) if m.is_file() => FileState::Changed,
        _ => FileState::Missing,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(archive: &Path, id: &str) {
        let dir = archive.join(id);
        std::fs::create_dir_all(&dir).unwrap();
    }

    #[test]
    fn records_looks_up_checks_and_forgets() {
        let tmp = tempfile::tempdir().unwrap();
        let store = tmp.path().join("appdata").join(FILE);
        let archive = tmp.path().join("Sussurro");
        let audio = tmp.path().join("Interview.wav");
        std::fs::write(&audio, b"0123456789").unwrap();
        item(&archive, "2026/09/interview");

        assert_eq!(lookup(&store, &archive, "2026/09/interview"), None);
        record(&store, &archive, "2026/09/interview", &audio).unwrap();
        let e = lookup(&store, &archive, "2026/09/interview").unwrap();
        assert_eq!(e.bytes, 10);
        assert_eq!(e.path, std::fs::canonicalize(&audio).unwrap());
        assert_eq!(check(&e), FileState::Available);
        // Another archive folder is another item.
        assert_eq!(
            lookup(&store, &tmp.path().join("Other"), "2026/09/interview"),
            None
        );

        // Replaced by other audio under the same name.
        std::fs::write(&audio, b"01234").unwrap();
        assert_eq!(check(&e), FileState::Changed);
        std::fs::remove_file(&audio).unwrap();
        assert_eq!(check(&e), FileState::Missing);

        forget(&store, &archive, "2026/09/interview").unwrap();
        assert_eq!(lookup(&store, &archive, "2026/09/interview"), None);
        forget(&store, &archive, "never/was/here").unwrap();
    }

    #[test]
    fn a_new_record_replaces_the_old_one_and_drops_deleted_items() {
        let tmp = tempfile::tempdir().unwrap();
        let store = tmp.path().join(FILE);
        let archive = tmp.path().join("Sussurro");
        let a = tmp.path().join("a.wav");
        let b = tmp.path().join("b.wav");
        std::fs::write(&a, b"aa").unwrap();
        std::fs::write(&b, b"bbb").unwrap();
        item(&archive, "2026/09/one");
        item(&archive, "2026/09/two");
        record(&store, &archive, "2026/09/one", &a).unwrap();
        record(&store, &archive, "2026/09/two", &a).unwrap();
        record(&store, &archive, "2026/09/one", &b).unwrap();
        assert_eq!(lookup(&store, &archive, "2026/09/one").unwrap().bytes, 3);

        // Item two is deleted: its path goes at the next write.
        std::fs::remove_dir_all(archive.join("2026/09/two")).unwrap();
        record(&store, &archive, "2026/09/one", &a).unwrap();
        assert_eq!(lookup(&store, &archive, "2026/09/two"), None);
        assert_eq!(read(&store).items.len(), 1);
    }

    #[test]
    fn a_broken_store_reads_as_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let store = tmp.path().join(FILE);
        std::fs::write(&store, b"{not json").unwrap();
        assert_eq!(lookup(&store, tmp.path(), "x"), None);
    }
}
