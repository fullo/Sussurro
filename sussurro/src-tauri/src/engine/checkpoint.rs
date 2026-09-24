//! Crash safety for long-form sessions (#153).
//!
//! The item is created when the session starts (marked `status: recording`
//! in its frontmatter) and every finished segment is saved into it right
//! away, so a crash, a forced quit or a power loss keeps everything
//! transcribed up to that moment. See [`crate::archive::live`] for the
//! on-disk side; this module adds the engine's bookkeeping:
//!
//! - [`LiveItem`]: the item a running session writes into. `segments.json`
//!   is replaced atomically after **every** segment; `transcript.md` is
//!   regenerated every [`RENDER_EVERY`] or [`RENDER_EVERY_SEGMENTS`]
//!   segments, whichever comes first (and always at the end). A failed
//!   checkpoint is logged, not fatal: the next one rewrites the whole file.
//!   Dropping a `LiveItem` without [`LiveItem::finish`] / [`LiveItem::discard`]
//!   / [`LiveItem::abandon`] is exactly what a crash leaves behind.
//! - The **session journal** (`engine-sessions.json`, app data dir) lists
//!   the items of sessions in flight. At the next start [`recover`] reads it
//!   and marks those items `interrupted` (and indexes them). The journal —
//!   not a scan of the archive — drives recovery so a normal start never
//!   touches `~/Documents` (macOS asks for permission on first access, and
//!   the plan wants that at onboarding, not at launch) and never walks a
//!   large archive. An item created in the instant before its journal entry
//!   is written stays marked `recording`; the UI can still open it.
//! - **Mic spool files** (`engine-spool-<pid>-<session>.f32`, app data
//!   dir) left by a dead process are deleted at start, with a log line.
//!   Decision: they are raw 16 kHz samples of segments that were queued
//!   for STT, interleaved with segments already transcribed, with no record
//!   of where each segment starts — turning them back into transcript would
//!   need a spool index plus a fresh STT run, which is not "cheap", and
//!   they hold the user's voice, which should not linger in app data.
//!   (Spools of a live second instance are safe: on Unix an open file
//!   survives the unlink, on Windows the delete fails while it is open.)

use crate::archive::{self, live, ItemMeta, Segment, SegmentsFile};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Regenerate `transcript.md` at least this often while segments arrive.
pub const RENDER_EVERY: Duration = Duration::from_secs(30);
/// …or after this many segments (a file backlog runs faster than real time).
pub const RENDER_EVERY_SEGMENTS: usize = 10;
/// File name of the session journal in the app data dir.
pub const JOURNAL_FILE: &str = "engine-sessions.json";
/// Prefix of the mic spool files in the app data dir.
pub const SPOOL_PREFIX: &str = "engine-spool-";

// ---- journal ---------------------------------------------------------------

/// One session in flight: its item and the archive it lives in (the archive
/// folder setting may change before the next start).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct JournalEntry {
    pub archive: PathBuf,
    pub id: String,
}

/// Sessions (mic + files) run on several threads; the journal is small and
/// rewritten whole under this lock.
static JOURNAL_LOCK: Mutex<()> = Mutex::new(());

fn journal_read(path: &Path) -> Vec<JournalEntry> {
    match std::fs::read_to_string(path) {
        Ok(s) => serde_json::from_str(&s).unwrap_or_else(|e| {
            eprintln!("engine: ignoring unreadable {} ({e})", path.display());
            Vec::new()
        }),
        Err(_) => Vec::new(),
    }
}

fn journal_write(path: &Path, entries: &[JournalEntry]) -> Result<()> {
    if entries.is_empty() {
        return match std::fs::remove_file(path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.into()),
            _ => Ok(()),
        };
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    archive::store::write_atomic(path, serde_json::to_string_pretty(entries)?.as_bytes())
}

fn journal_update(path: &Path, f: impl FnOnce(&mut Vec<JournalEntry>)) -> Result<()> {
    let _g = JOURNAL_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut entries = journal_read(path);
    f(&mut entries);
    journal_write(path, &entries)
}

/// The sessions the journal says are in flight.
pub fn journal_entries(path: &Path) -> Vec<JournalEntry> {
    let _g = JOURNAL_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    journal_read(path)
}

// ---- live item -------------------------------------------------------------

/// The archive item a running session writes into.
pub struct LiveItem {
    archive: PathBuf,
    id: String,
    journal: Option<PathBuf>,
    file: SegmentsFile,
    last_render: Instant,
    since_render: usize,
    /// Started without a title: the folder is named `…-untitled` until the
    /// final title is known.
    untitled: bool,
}

impl LiveItem {
    /// Create the item (`status: recording`, no segments) and record it in
    /// the journal (`None`: no journal, tests).
    pub fn begin(archive: &Path, meta: &ItemMeta, journal: Option<&Path>) -> Result<Self> {
        live::ensure_writable(archive)?;
        let id = live::begin_session(archive, meta).context("creating the archive item")?;
        let item = Self {
            archive: archive.to_path_buf(),
            id,
            journal: journal.map(Path::to_path_buf),
            file: SegmentsFile::default(),
            last_render: Instant::now(),
            since_render: 0,
            untitled: meta.title.trim().is_empty(),
        };
        if let Some(j) = &item.journal {
            let entry = JournalEntry {
                archive: item.archive.clone(),
                id: item.id.clone(),
            };
            // Without the entry a crash would leave the item "recording"
            // instead of "interrupted" — worth a log, not worth failing.
            if let Err(e) = journal_update(j, |es| es.push(entry)) {
                eprintln!("engine: session journal not updated ({e:#})");
            }
        }
        Ok(item)
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn segments(&self) -> &[Segment] {
        &self.file.segments
    }

    /// Some segment has text (not only failed or empty ones).
    pub fn has_text(&self) -> bool {
        self.file.segments.iter().any(|s| !s.text.trim().is_empty())
    }

    /// Add a finished segment and checkpoint it to disk.
    pub fn push(&mut self, segment: Segment) {
        self.file.segments.push(segment);
        self.since_render += 1;
        let render = self.since_render >= RENDER_EVERY_SEGMENTS
            || self.last_render.elapsed() >= RENDER_EVERY;
        match live::checkpoint(&self.archive, &self.id, &self.file, render) {
            Ok(()) if render => {
                self.since_render = 0;
                self.last_render = Instant::now();
            }
            Ok(()) => {}
            Err(e) => eprintln!("engine: checkpoint of {} failed ({e:#})", self.id),
        }
    }

    fn unjournal(&self) {
        if let Some(j) = &self.journal {
            let (archive, id) = (&self.archive, &self.id);
            if let Err(e) = journal_update(j, |es| {
                es.retain(|x| !(&x.archive == archive && &x.id == id))
            }) {
                eprintln!("engine: session journal not updated ({e:#})");
            }
        }
    }

    /// Normal stop: final segments, `finalize` applied to the metadata as it
    /// is in the file, marker cleared, final render; the folder takes the
    /// final title if the session started without one. Returns the final
    /// id and metadata.
    pub fn finish(
        self,
        fallback: &ItemMeta,
        finalize: impl Fn(ItemMeta) -> ItemMeta,
    ) -> Result<(String, ItemMeta)> {
        let meta = live::finish_session(&self.archive, &self.id, &self.file, fallback, finalize);
        let meta = match meta {
            Ok(m) => m,
            Err(e) => {
                // Keep what is on disk, as after a crash.
                let _ = live::mark_interrupted(&self.archive, &self.id);
                self.unjournal();
                return Err(e.context("finalizing the archive item"));
            }
        };
        self.unjournal();
        let id = if self.untitled {
            live::retitle_folder(&self.archive, &self.id, &meta.title)
        } else {
            self.id.clone()
        };
        Ok((id, meta))
    }

    /// The run failed: keep the segments saved so far as an `interrupted`
    /// item (if any has text). Returns the kept item's id.
    pub fn abandon(self) -> Option<String> {
        if !self.has_text() {
            return self.discard();
        }
        let saved = live::checkpoint(&self.archive, &self.id, &self.file, false)
            .and_then(|()| live::mark_interrupted(&self.archive, &self.id));
        self.unjournal();
        match saved {
            Ok(_) => Some(self.id.clone()),
            Err(e) => {
                eprintln!("engine: could not keep {} ({e:#})", self.id);
                None
            }
        }
    }

    /// The user cancelled (or nothing was said): remove the item — unless
    /// the user edited its transcript meanwhile, then it is kept as
    /// `interrupted`. Returns the kept item's id, if any.
    pub fn discard(self) -> Option<String> {
        let kept = match live::discard_session(&self.archive, &self.id) {
            Ok(true) => None,
            Ok(false) => Some(self.id.clone()),
            Err(e) => {
                eprintln!("engine: could not remove {} ({e:#})", self.id);
                None
            }
        };
        self.unjournal();
        kept
    }
}

// ---- recovery at start -----------------------------------------------------

/// What [`recover`] did.
#[derive(Debug, Default, PartialEq)]
pub struct Recovery {
    /// Items marked `interrupted` (ids in the current archive, or in the
    /// archive they were recorded into).
    pub interrupted: Vec<String>,
    /// Stale mic spool files deleted.
    pub spools_removed: usize,
}

/// Run once at app start, before any session: mark the items of sessions
/// that never finished as `interrupted` (indexing those in `current_archive`
/// into `index_db`), empty the journal and delete stale mic spools in
/// `spool_dir`.
pub fn recover(
    journal: &Path,
    current_archive: Option<&Path>,
    index_db: Option<&Path>,
    spool_dir: &Path,
) -> Recovery {
    let mut out = Recovery::default();
    let _g = JOURNAL_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    for entry in journal_read(journal) {
        match live::mark_interrupted(&entry.archive, &entry.id) {
            Ok(true) => {
                eprintln!("engine: session item {} was interrupted; kept", entry.id);
                // The index belongs to one archive root: opening it for
                // another would rebuild it for the wrong folder.
                if let (Some(db), true) =
                    (index_db, current_archive == Some(entry.archive.as_path()))
                {
                    if let Err(e) = archive::Index::open(&entry.archive, db)
                        .and_then(|mut i| i.index_item(&entry.id))
                    {
                        eprintln!("archive index: update failed ({e:#})");
                    }
                }
                out.interrupted.push(entry.id);
            }
            // Already finalized (the crash hit after the marker was
            // cleared) or not marked by us: nothing to do.
            Ok(false) => {}
            // Deleted by the user, or the archive is gone.
            Err(e) => eprintln!("engine: skipping journal entry {} ({e:#})", entry.id),
        }
    }
    if let Err(e) = journal_write(journal, &[]) {
        eprintln!("engine: could not clear the session journal ({e:#})");
    }
    out.spools_removed = remove_stale_spools(spool_dir, std::process::id());
    out
}

/// `engine-spool-<pid>-<session>.f32` → `pid`.
fn spool_pid(name: &str) -> Option<u32> {
    let rest = name.strip_prefix(SPOOL_PREFIX)?.strip_suffix(".f32")?;
    rest.split('-').next()?.parse().ok()
}

/// Delete mic spools not owned by `current_pid` (see the module docs for
/// why they are not recovered). Returns how many were removed.
pub fn remove_stale_spools(dir: &Path, current_pid: u32) -> usize {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    let mut removed = 0;
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let Some(pid) = spool_pid(&name) else {
            continue;
        };
        if pid == current_pid {
            continue;
        }
        let size = e.metadata().map(|m| m.len()).unwrap_or(0);
        match std::fs::remove_file(e.path()) {
            Ok(()) => {
                eprintln!(
                    "engine: removed a stale mic spool {name} ({:.1} s of untranscribed audio from a session that did not finish)",
                    size as f64 / 4.0 / 16_000.0
                );
                removed += 1;
            }
            Err(err) => eprintln!("engine: could not remove {name} ({err})"),
        }
    }
    removed
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::archive::{read_item, ItemType};

    fn meta(title: &str) -> ItemMeta {
        ItemMeta {
            item_type: ItemType::Note,
            title: title.into(),
            date: "2026-09-24T10:00:00+02:00".into(),
            source: "mic".into(),
            language: "it".into(),
            ..Default::default()
        }
    }

    fn seg(i: u32) -> Segment {
        Segment {
            id: i,
            start_ms: i as u64 * 5_000,
            end_ms: i as u64 * 5_000 + 4_000,
            raw: format!("parola{i}"),
            text: format!("Parola{i}."),
            ..Default::default()
        }
    }

    struct Env {
        _tmp: tempfile::TempDir,
        archive: PathBuf,
        journal: PathBuf,
        db: PathBuf,
        data: PathBuf,
    }

    fn env() -> Env {
        let tmp = tempfile::tempdir().unwrap();
        let data = tmp.path().join("appdata");
        std::fs::create_dir_all(&data).unwrap();
        Env {
            archive: tmp.path().join("Sussurro"),
            journal: data.join(JOURNAL_FILE),
            db: data.join("index.sqlite"),
            data,
            _tmp: tmp,
        }
    }

    #[test]
    fn crashed_session_is_recovered_as_interrupted_on_startup() {
        let e = env();
        let mut live = LiveItem::begin(&e.archive, &meta("Riunione"), Some(&e.journal)).unwrap();
        let id = live.id().to_string();
        for i in 0..3 {
            live.push(seg(i));
        }
        // Crash: the session is dropped without being finalized.
        drop(live);
        let item = read_item(&e.archive, &id).unwrap();
        assert!(item.recording, "still marked in progress on disk");
        assert_eq!(
            item.segments.segments.len(),
            3,
            "every segment checkpointed"
        );
        assert_eq!(journal_entries(&e.journal).len(), 1);

        // Next start.
        let r = recover(&e.journal, Some(&e.archive), Some(&e.db), &e.data);
        assert_eq!(r.interrupted, vec![id.clone()]);
        assert!(!e.journal.exists(), "journal emptied");

        let item = read_item(&e.archive, &id).unwrap();
        assert!(item.interrupted && !item.recording);
        assert_eq!(item.segments.segments.len(), 3);
        assert!(
            item.body.contains("Parola0. Parola1. Parola2."),
            "{}",
            item.body
        );
        assert_eq!(item.meta.duration.as_deref(), Some("00:00:14"));
        let listed = archive::list_items(&e.archive);
        assert!(listed[0].interrupted);
        // Indexed: searchable, flagged in search results too.
        let hits = archive::with_index(&e.archive, &e.db, |i| {
            i.search("parola1", &Default::default())
        })
        .unwrap();
        assert_eq!(hits.len(), 1);
        assert!(hits[0].interrupted);

        // A second start finds nothing more to do.
        assert_eq!(
            recover(&e.journal, Some(&e.archive), Some(&e.db), &e.data),
            Recovery::default()
        );
    }

    #[test]
    fn finish_clears_the_marker_and_the_journal() {
        let e = env();
        let mut live = LiveItem::begin(&e.archive, &meta(""), Some(&e.journal)).unwrap();
        assert!(live.id().ends_with("-untitled"));
        live.push(seg(0));
        live.push(seg(1));
        let (id, m) = live
            .finish(&meta(""), |mut m| {
                m.title = "Parola0 Parola1".into();
                m.duration = Some("00:00:09".into());
                m
            })
            .unwrap();
        assert_eq!(
            id, "2026/09/2026-09-24-parola0-parola1",
            "folder follows the final title"
        );
        assert_eq!(m.session_state(), None);
        let item = read_item(&e.archive, &id).unwrap();
        assert!(!item.recording && !item.interrupted);
        assert_eq!(item.meta.duration.as_deref(), Some("00:00:09"));
        assert!(item.body.contains("# Parola0 Parola1") && item.body.contains("Parola1."));
        assert!(journal_entries(&e.journal).is_empty());
        // Nothing for a later start to recover.
        assert!(recover(&e.journal, Some(&e.archive), None, &e.data)
            .interrupted
            .is_empty());
        assert!(!read_item(&e.archive, &id).unwrap().interrupted);
    }

    #[test]
    fn transcript_is_rendered_periodically_not_on_every_segment() {
        let e = env();
        let mut live = LiveItem::begin(&e.archive, &meta("Cadence"), None).unwrap();
        let path = e.archive.join(live.id()).join("transcript.md");
        for i in 0..(RENDER_EVERY_SEGMENTS as u32 - 1) {
            live.push(seg(i));
        }
        let before = std::fs::read_to_string(&path).unwrap();
        assert!(!before.contains("Parola0."), "not rendered yet");
        live.push(seg(RENDER_EVERY_SEGMENTS as u32 - 1));
        assert!(std::fs::read_to_string(&path).unwrap().contains("Parola0."));
    }

    #[test]
    fn abandon_keeps_partial_results_and_discard_removes_them() {
        let e = env();
        let mut live = LiveItem::begin(&e.archive, &meta("Errore"), Some(&e.journal)).unwrap();
        live.push(seg(0));
        let id = live.abandon().expect("kept");
        assert!(read_item(&e.archive, &id).unwrap().interrupted);
        assert!(journal_entries(&e.journal).is_empty());

        let live = LiveItem::begin(&e.archive, &meta("Vuoto"), Some(&e.journal)).unwrap();
        let empty = live.id().to_string();
        assert_eq!(live.abandon(), None, "nothing to keep");
        assert!(read_item(&e.archive, &empty).is_err());

        let mut live = LiveItem::begin(&e.archive, &meta("Annullato"), Some(&e.journal)).unwrap();
        live.push(seg(0));
        let gone = live.id().to_string();
        assert_eq!(live.discard(), None);
        assert!(read_item(&e.archive, &gone).is_err());
        assert!(journal_entries(&e.journal).is_empty());
    }

    #[test]
    fn recovery_skips_deleted_items_and_other_archives_index() {
        let e = env();
        let other = e.archive.with_file_name("Old");
        let live = LiveItem::begin(&other, &meta("Altrove"), Some(&e.journal)).unwrap();
        let moved = live.id().to_string();
        drop(live);
        let live = LiveItem::begin(&e.archive, &meta("Cancellato"), Some(&e.journal)).unwrap();
        std::fs::remove_dir_all(e.archive.join(live.id())).unwrap();
        drop(live);
        let r = recover(&e.journal, Some(&e.archive), Some(&e.db), &e.data);
        assert_eq!(r.interrupted, vec![moved.clone()]);
        assert!(read_item(&other, &moved).unwrap().interrupted);
        assert!(!e.db.exists(), "another archive's item is not indexed here");
        assert!(!e.journal.exists());
    }

    #[test]
    fn stale_spools_are_removed_at_startup() {
        let e = env();
        // Spools of dead processes, one of this process, unrelated files.
        let dead = [
            e.data.join("engine-spool-1-4.f32"),
            e.data.join("engine-spool-99999-1.f32"),
        ];
        for p in &dead {
            std::fs::write(p, [0u8; 64]).unwrap();
        }
        let mine = e
            .data
            .join(format!("engine-spool-{}-1.f32", std::process::id()));
        std::fs::write(&mine, [0u8; 8]).unwrap();
        let history = e.data.join("history.jsonl");
        std::fs::write(&history, "{}").unwrap();

        let r = recover(&e.journal, Some(&e.archive), Some(&e.db), &e.data);
        assert_eq!(r.spools_removed, 2);
        assert!(dead.iter().all(|p| !p.exists()));
        assert!(mine.exists(), "a live spool of this process is kept");
        assert!(history.exists());
        assert!(r.interrupted.is_empty());
        assert!(
            !e.archive.exists(),
            "no journal entry: the archive is never touched"
        );
    }

    #[test]
    fn spool_names_parse() {
        assert_eq!(spool_pid("engine-spool-123-4.f32"), Some(123));
        assert_eq!(spool_pid("engine-spool-x-4.f32"), None);
        assert_eq!(spool_pid("history.jsonl"), None);
        assert_eq!(remove_stale_spools(Path::new("/nonexistent-dir-xyz"), 1), 0);
    }
}
