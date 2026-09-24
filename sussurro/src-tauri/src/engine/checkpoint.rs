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
//!   Each entry records the pid of the process running the session (#158):
//!   no single-instance guard stops a second app instance, and its start
//!   must not mark the first one's live sessions interrupted, so entries of
//!   a running process ([`process_alive`]) are left alone.
//! - **Mic spool files** (`engine-spool-<pid>-<session>.f32`, app data
//!   dir) left by a dead process are deleted at start, with a log line.
//!   Decision: they are raw 16 kHz samples of segments that were queued
//!   for STT, interleaved with segments already transcribed, with no record
//!   of where each segment starts — turning them back into transcript would
//!   need a spool index plus a fresh STT run, which is not "cheap", and
//!   they hold the user's voice, which should not linger in app data.
//!   Spools of a running process (a second instance) are left alone.

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
    /// The process running the session (#158). Nothing stops a second app
    /// instance from starting, and its [`recover`] must not mark the first
    /// one's live sessions interrupted: entries of a running process are
    /// left alone. `0` in entries written before this field existed:
    /// treated as a dead process.
    #[serde(default)]
    pub pid: u32,
}

/// Whether the process `pid` is running. Portable enough for the journal:
/// `kill(pid, 0)` on Unix (EPERM: it exists, owned by someone else),
/// `OpenProcess` + `GetExitCodeProcess` on Windows.
///
/// Pid reuse: an unrelated process that got a dead instance's pid makes its
/// entries look alive, so their recovery waits for a later start (the item
/// stays `recording` — read-only — until then). Never the other way round:
/// a live session is never taken for a dead one.
pub fn process_alive(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    #[cfg(unix)]
    {
        let Ok(pid) = libc::pid_t::try_from(pid) else {
            return false;
        };
        // SAFETY: signal 0 only checks existence and permissions.
        if unsafe { libc::kill(pid, 0) } == 0 {
            return true;
        }
        std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
    }
    #[cfg(windows)]
    {
        use windows_sys::Win32::Foundation::{
            CloseHandle, GetLastError, ERROR_ACCESS_DENIED, STILL_ACTIVE,
        };
        use windows_sys::Win32::System::Threading::{
            GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
        };
        // SAFETY: plain Win32 calls; the handle is closed before returning.
        unsafe {
            let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if handle.is_null() {
                // Exists but not ours to query (another user's process).
                return GetLastError() == ERROR_ACCESS_DENIED;
            }
            let mut code = 0u32;
            let ok = GetExitCodeProcess(handle, &mut code) != 0;
            CloseHandle(handle);
            ok && code == STILL_ACTIVE as u32
        }
    }
    #[cfg(not(any(unix, windows)))]
    {
        false
    }
}

/// Whether the item `id` of `archive` is being written by a running session
/// — of this process or of another instance — according to the journal.
/// The archive commands refuse to delete or line-edit such an item (#158),
/// on top of the `recording` marker check in the store.
pub fn owned_by_live_session(journal: &Path, archive: &Path, id: &str) -> bool {
    let me = std::process::id();
    journal_entries(journal)
        .iter()
        .any(|e| e.archive == archive && e.id == id && (e.pid == me || process_alive(e.pid)))
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
                pid: std::process::id(),
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

    /// List a speaker the next checkpoint writes (#130); one already
    /// listed is left as is.
    pub fn add_speaker(&mut self, speaker: crate::archive::DocSpeaker) {
        if !self.file.speakers.iter().any(|s| s.id == speaker.id) {
            self.file.speakers.push(speaker);
        }
    }

    /// Fold the session's tiny voices into the nearest ones and number
    /// them 1, 2, … ([`crate::speakers::doc::finalize_live`]); written by
    /// [`Self::finish`].
    pub fn finalize_voices(&mut self) {
        crate::speakers::doc::finalize_live(&mut self.file);
    }

    /// The speakers listed so far.
    pub fn speakers(&self) -> &[crate::archive::DocSpeaker] {
        &self.file.speakers
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
                // Keep what is on disk, as after a crash. The journal entry
                // goes only once the item is no longer `recording` (#158):
                // if even the marker can't be rewritten, the next start
                // retries instead of leaving it "recording" forever.
                match live::mark_interrupted(&self.archive, &self.id) {
                    Ok(_) => self.unjournal(),
                    Err(m) => eprintln!(
                        "engine: {} left in the session journal for the next start ({m:#})",
                        self.id
                    ),
                }
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
        match saved {
            Ok(_) => {
                self.unjournal();
                Some(self.id.clone())
            }
            Err(e) => {
                // Still `recording` on disk: the journal entry stays, so the
                // next start marks it interrupted (#158).
                eprintln!("engine: could not keep {} for now ({e:#})", self.id);
                None
            }
        }
    }

    /// The user cancelled (or nothing was said): drop the item — to the OS
    /// trash if any segment has text, removed only when empty — unless the
    /// user edited its transcript meanwhile, then it is kept as
    /// `interrupted`. Returns the kept item's id, if any.
    pub fn discard(self) -> Option<String> {
        match live::discard_session(&self.archive, &self.id) {
            Ok(done) => {
                self.unjournal();
                (done == live::Discarded::Kept).then(|| self.id.clone())
            }
            Err(e) => {
                // Still on disk and `recording`: the journal entry stays, so
                // the next start marks it interrupted instead (#158).
                eprintln!("engine: could not remove {} for now ({e:#})", self.id);
                None
            }
        }
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
/// into `index_db`), drop their journal entries and delete stale mic spools
/// in `spool_dir`. An entry is dropped only once its item is in a final
/// state (#158): marked, already finished, or deleted by the user. One whose
/// archive folder is missing (an unplugged drive) or whose item could not be
/// rewritten stays for the next start.
pub fn recover(
    journal: &Path,
    current_archive: Option<&Path>,
    index_db: Option<&Path>,
    spool_dir: &Path,
) -> Recovery {
    let mut out = Recovery::default();
    let _g = JOURNAL_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut keep = Vec::new();
    // Run before any session of this process: an entry carrying our own pid
    // is a dead process's whose pid was reused by us.
    let me = std::process::id();
    for entry in journal_read(journal) {
        if entry.pid != me && process_alive(entry.pid) {
            // Another instance is running this session (#158).
            eprintln!(
                "engine: {} belongs to a running Sussurro (pid {}); left alone",
                entry.id, entry.pid
            );
            keep.push(entry);
            continue;
        }
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
            // Deleted by the user: nothing left to recover. The archive
            // folder missing, or the item still there but not rewritable:
            // try again at the next start.
            Err(e) => {
                let item_left = archive::paths::item_dir(&entry.archive, &entry.id)
                    .map(|d| d.join(archive::store::TRANSCRIPT_FILE).is_file())
                    .unwrap_or(false);
                if item_left || !entry.archive.is_dir() {
                    eprintln!("engine: journal entry {} kept for later ({e:#})", entry.id);
                    keep.push(entry);
                } else {
                    eprintln!("engine: skipping journal entry {} ({e:#})", entry.id);
                }
            }
        }
    }
    if let Err(e) = journal_write(journal, &keep) {
        eprintln!("engine: could not update the session journal ({e:#})");
    }
    out.spools_removed = remove_stale_spools(spool_dir, std::process::id());
    out
}

/// `engine-spool-<pid>-<session>.f32` → `pid`.
fn spool_pid(name: &str) -> Option<u32> {
    let rest = name.strip_prefix(SPOOL_PREFIX)?.strip_suffix(".f32")?;
    rest.split('-').next()?.parse().ok()
}

/// Delete mic spools of dead processes: not `current_pid`'s, not a running
/// second instance's (see the module docs for why they are not recovered).
/// Returns how many were removed.
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
        if pid == current_pid || process_alive(pid) {
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
        assert!(
            archive::store::test_trash::contains(&e.archive.join(&gone)),
            "a cancel with text goes to the trash (#158)"
        );
        assert!(journal_entries(&e.journal).is_empty());
    }

    /// #158 finding 3: when finish / abandon / discard can't bring the item
    /// to a final state (here: the archive drive is unplugged), the journal
    /// entry stays, so a later start still marks it interrupted instead of
    /// leaving it `recording` forever.
    #[test]
    fn a_failed_end_of_session_keeps_the_journal_entry() {
        let e = env();
        let mut ids = Vec::new();
        let mut items = Vec::new();
        for title in ["Finish", "Abandon", "Discard"] {
            let mut live = LiveItem::begin(&e.archive, &meta(title), Some(&e.journal)).unwrap();
            live.push(seg(0));
            ids.push(live.id().to_string());
            items.push(live);
        }
        let away = e.archive.with_file_name("Unplugged");
        std::fs::rename(&e.archive, &away).unwrap();
        let mut items = items.into_iter();
        assert!(items
            .next()
            .unwrap()
            .finish(&meta("Finish"), |m| m)
            .is_err());
        assert_eq!(items.next().unwrap().abandon(), None);
        assert_eq!(items.next().unwrap().discard(), None);
        assert_eq!(
            journal_entries(&e.journal).len(),
            3,
            "every entry kept for the next start"
        );

        // A start while the drive is still missing keeps the entries too.
        let r = recover(&e.journal, Some(&e.archive), None, &e.data);
        assert!(r.interrupted.is_empty());
        assert_eq!(journal_entries(&e.journal).len(), 3);

        // Plugged back in: the next start recovers all three.
        std::fs::rename(&away, &e.archive).unwrap();
        let r = recover(&e.journal, Some(&e.archive), None, &e.data);
        assert_eq!(r.interrupted, ids);
        for id in &ids {
            let item = read_item(&e.archive, id).unwrap();
            assert!(item.interrupted && !item.recording, "{id}");
        }
        assert!(!e.journal.exists());
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

    /// A running process that is not this one (killed by the caller).
    fn other_live_process() -> std::process::Child {
        #[cfg(unix)]
        let mut cmd = std::process::Command::new("sleep");
        #[cfg(unix)]
        cmd.arg("30");
        #[cfg(windows)]
        let mut cmd = std::process::Command::new("ping");
        #[cfg(windows)]
        cmd.args(["-n", "30", "127.0.0.1"]);
        cmd.stdout(std::process::Stdio::null()).spawn().unwrap()
    }

    /// The pid of a process that has exited (and was reaped).
    fn dead_pid() -> u32 {
        #[cfg(unix)]
        let mut cmd = std::process::Command::new("true");
        #[cfg(windows)]
        let mut cmd = std::process::Command::new("cmd");
        #[cfg(windows)]
        cmd.args(["/C", "exit 0"]);
        let mut child = cmd.spawn().unwrap();
        let pid = child.id();
        child.wait().unwrap();
        pid
    }

    fn set_pids(journal: &Path, pids: &[u32]) {
        let mut entries = journal_entries(journal);
        for (e, pid) in entries.iter_mut().zip(pids) {
            e.pid = *pid;
        }
        journal_write(journal, &entries).unwrap();
    }

    #[test]
    fn process_liveness() {
        assert!(process_alive(std::process::id()));
        assert!(!process_alive(0));
        assert!(!process_alive(dead_pid()));
        let mut other = other_live_process();
        assert!(process_alive(other.id()));
        other.kill().unwrap();
        other.wait().unwrap();
        assert!(!process_alive(other.id()));
    }

    /// #158 finding 2: a second app instance's start must not mark the first
    /// one's live session interrupted; the backend sees that item as live.
    #[test]
    fn recovery_leaves_a_running_instances_sessions_alone() {
        let e = env();
        let mut other = other_live_process();
        let mut items = Vec::new();
        for title in ["Altra istanza", "Morta", "Vecchia"] {
            let mut live = LiveItem::begin(&e.archive, &meta(title), Some(&e.journal)).unwrap();
            live.push(seg(0));
            items.push(live);
        }
        let ids: Vec<String> = items.iter().map(|l| l.id().to_string()).collect();
        assert!(journal_entries(&e.journal)
            .iter()
            .all(|x| x.pid == std::process::id()));
        // Every entry of this process is live until its session ends.
        assert!(owned_by_live_session(&e.journal, &e.archive, &ids[0]));
        // As if other processes had written them: a live instance's
        // session, a crashed one's, and an entry written before the pid
        // was recorded.
        drop(items);
        set_pids(&e.journal, &[other.id(), dead_pid(), 0]);
        assert!(owned_by_live_session(&e.journal, &e.archive, &ids[0]));
        assert!(!owned_by_live_session(&e.journal, &e.archive, &ids[1]));
        assert!(!owned_by_live_session(&e.journal, &e.archive, &ids[2]));

        let r = recover(&e.journal, Some(&e.archive), None, &e.data);
        assert_eq!(r.interrupted, ids[1..]);
        let running = read_item(&e.archive, &ids[0]).unwrap();
        assert!(running.recording && !running.interrupted, "left alone");
        let kept = journal_entries(&e.journal);
        assert_eq!(kept.len(), 1);
        assert_eq!(
            (kept[0].id.as_str(), kept[0].pid),
            (ids[0].as_str(), other.id())
        );

        // Once that instance is gone, the next start recovers it.
        other.kill().unwrap();
        other.wait().unwrap();
        assert!(!owned_by_live_session(&e.journal, &e.archive, &ids[0]));
        let r = recover(&e.journal, Some(&e.archive), None, &e.data);
        assert_eq!(r.interrupted, [ids[0].clone()]);
        assert!(!e.journal.exists());
    }

    #[test]
    fn a_running_instances_spool_is_kept() {
        let e = env();
        let mut other = other_live_process();
        let theirs = e.data.join(format!("engine-spool-{}-1.f32", other.id()));
        std::fs::write(&theirs, [0u8; 8]).unwrap();
        assert_eq!(remove_stale_spools(&e.data, std::process::id()), 0);
        assert!(theirs.exists());
        other.kill().unwrap();
        other.wait().unwrap();
        assert_eq!(remove_stale_spools(&e.data, std::process::id()), 1);
    }

    #[test]
    fn stale_spools_are_removed_at_startup() {
        let e = env();
        // Spools of dead processes, one of this process, unrelated files.
        let dead = [
            e.data.join(format!("engine-spool-{}-4.f32", dead_pid())),
            e.data.join(format!("engine-spool-{}-1.f32", dead_pid())),
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
