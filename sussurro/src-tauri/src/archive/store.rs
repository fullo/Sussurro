//! Item storage on disk: one folder per item.
//!
//! ```text
//! <archive>/YYYY/MM/YYYY-MM-DD-<slug>/
//!   transcript.md            frontmatter + rendered text (user-editable)
//!   .sussurro/segments.json  segments, raw + cleaned text, timings
//!   .sussurro/state.json     SHA-256 of the transcript.md the app last wrote
//! ```
//!
//! Files are the source of truth (E2). The app regenerates `transcript.md`
//! only while it is byte-identical to what the app last wrote; after an
//! external edit the markdown wins. The app never hard-deletes an item: it
//! goes to the OS trash.

use super::frontmatter;
use super::paths::{folder_name, id_from_dir, item_dir, month_dir, slugify};
use super::render::render_transcript;
use super::types::{
    normalize_participants, ItemMeta, SegmentsFile, SessionState, SEGMENTS_VERSION, SESSION_KEY,
};
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

pub const TRANSCRIPT_FILE: &str = "transcript.md";
pub const META_DIR: &str = ".sussurro";
pub const SEGMENTS_FILE: &str = "segments.json";
pub const STATE_FILE: &str = "state.json";
/// How deep `list_items` looks for item folders (`YYYY/MM/item` is 3).
const MAX_SCAN_DEPTH: usize = 4;
/// Collision suffixes tried before giving up (`-2` … `-999`).
const MAX_COLLISIONS: u32 = 999;

/// A full item as returned to the UI.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Item {
    pub id: String,
    pub meta: ItemMeta,
    pub segments: SegmentsFile,
    /// `transcript.md` after the frontmatter (includes the `# title` line).
    pub body: String,
    /// `transcript.md` changed since the app last wrote it: the app will not
    /// regenerate it from the segments any more.
    pub edited_externally: bool,
    /// A capture session is writing this item right now (`status:
    /// recording` in the frontmatter, #153).
    pub recording: bool,
    /// The app stopped before the session was finalized (`status:
    /// interrupted`): the item holds the segments saved until then.
    pub interrupted: bool,
    /// Hosts its text was sent to by an external LLM profile (#122),
    /// sorted; empty = it never left the machine. See
    /// [`super::external::sent_hosts_at`].
    pub external_hosts: Vec<String>,
    /// Lines carrying a speaker embedding (#130): "Re-detect speakers"
    /// has something to work on when this is not zero. Counted before
    /// [`Item::without_embeddings`] strips them for the UI.
    pub embedded_segments: usize,
    /// Saved audio in the item folder (#141): `audio.wav`, or one file per
    /// channel. Empty when no audio was saved (or it was deleted).
    pub audio: Vec<super::audio::AudioFile>,
    /// Size of the item folder on disk, audio included.
    pub folder_bytes: u64,
}

impl Item {
    /// The item as the UI gets it: embeddings (256 floats per line, only
    /// needed by the backend) are left out of the payload.
    pub fn without_embeddings(mut self) -> Self {
        for s in &mut self.segments.segments {
            s.embedding = None;
        }
        self
    }
}

/// A list/search row.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ItemSummary {
    pub id: String,
    pub meta: ItemMeta,
    pub edited_externally: bool,
    /// Highlighted excerpt around the match (search results only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snippet: Option<String>,
    /// See [`Item::recording`].
    pub recording: bool,
    /// See [`Item::interrupted`].
    pub interrupted: bool,
    /// See [`Item::external_hosts`] (the Library's "sent externally" marker).
    pub external_hosts: Vec<String>,
    /// Bytes of saved audio in the item folder (#141); 0 = none.
    pub audio_bytes: u64,
}

impl ItemSummary {
    pub(crate) fn new(
        id: String,
        meta: ItemMeta,
        edited_externally: bool,
        snippet: Option<String>,
    ) -> Self {
        let state = meta.session_state();
        Self {
            id,
            meta,
            edited_externally,
            snippet,
            recording: state == Some(SessionState::Recording),
            interrupted: state == Some(SessionState::Interrupted),
            external_hosts: Vec::new(),
            audio_bytes: 0,
        }
    }

    /// Fill what lives in the item folder rather than in the frontmatter —
    /// [`ItemSummary::external_hosts`] and [`ItemSummary::audio_bytes`] —
    /// (search rows come from the index, which doesn't store them).
    pub(crate) fn with_folder_details(mut self, archive: &Path) -> Self {
        if let Ok(dir) = item_dir(archive, &self.id) {
            self.fill_folder_details(&dir);
        }
        self
    }

    fn fill_folder_details(&mut self, dir: &Path) {
        self.external_hosts = super::external::sent_hosts_at(dir);
        self.audio_bytes = super::audio::files_with_sizes(dir)
            .iter()
            .map(|f| f.bytes)
            .sum();
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct ItemState {
    /// SHA-256 (hex) of the `transcript.md` bytes the app last wrote.
    #[serde(default)]
    transcript_sha256: String,
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(bytes))
}

/// Serializes read-modify-write cycles on items within the process: the
/// engine checkpoints a live item (#153) while the UI may edit its metadata,
/// and both read `transcript.md`, check the content hash and write it back.
static ITEM_LOCK: Mutex<()> = Mutex::new(());

pub(super) fn lock_items() -> std::sync::MutexGuard<'static, ()> {
    // A panic elsewhere must not wedge the archive: the files are the state.
    ITEM_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

/// A fresh temp name next to `path`: dot-prefixed (skipped by the item scan
/// and by most sync clients) and unique per call — process id, a
/// per-process counter and the clock's nanoseconds — so concurrent writers
/// of the same file never share a temp file (#155).
fn temp_path(path: &Path) -> Result<PathBuf> {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let dir = path.parent().context("path without parent")?;
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .context("path without file name")?;
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    Ok(dir.join(format!(".{name}.tmp-{}-{n}-{nanos:09}", std::process::id())))
}

/// Write `bytes` to a new temp file next to `path` (same folder: rename is
/// atomic only within one filesystem) and return its path. `create_new`
/// guarantees the file is ours alone; it is flushed to disk before it is
/// returned (#153), so a later rename never exposes an empty file.
fn write_temp(path: &Path, bytes: &[u8]) -> Result<PathBuf> {
    use std::io::Write;
    let mut attempts = 0;
    let (tmp, mut file) = loop {
        let tmp = temp_path(path)?;
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)
        {
            Ok(file) => break (tmp, file),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists && attempts < 8 => {
                attempts += 1;
            }
            Err(e) => return Err(e).with_context(|| format!("creating {}", tmp.display())),
        }
    };
    if let Err(e) = file.write_all(bytes).and_then(|()| file.sync_all()) {
        drop(file);
        let _ = std::fs::remove_file(&tmp);
        return Err(e).with_context(|| format!("writing {}", tmp.display()));
    }
    Ok(tmp)
}

/// Move a staged temp file over `path` (removing the temp file on failure).
/// On Unix the folder entry is flushed after the rename, so a power loss
/// leaves either the old or the new file, never an empty one.
fn rename_into(tmp: &Path, path: &Path) -> Result<()> {
    if let Err(e) = std::fs::rename(tmp, path) {
        let _ = std::fs::remove_file(tmp);
        return Err(e).with_context(|| format!("replacing {}", path.display()));
    }
    #[cfg(unix)]
    if let Some(Ok(d)) = path.parent().map(std::fs::File::open) {
        let _ = d.sync_all();
    }
    Ok(())
}

/// Write via a dot-prefixed temp file + rename, so a crash or a sync client
/// never sees a half-written document.
pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let tmp = write_temp(path, bytes)?;
    rename_into(&tmp, path)
}

/// Serializes the app's own check-and-replace of `transcript.md` files, so
/// two saves in this process can't both pass the freshness check. Taken
/// after [`lock_items`] when both are held.
static TRANSCRIPT_COMMIT: Mutex<()> = Mutex::new(());

/// [`commit_transcript`] found the file changed since the caller read it.
/// A distinct type so the engine's own writers can re-read and retry
/// (`e.downcast_ref::<ChangedOnDisk>()` sees through added context).
#[derive(Debug)]
pub(super) struct ChangedOnDisk(PathBuf);

impl std::fmt::Display for ChangedOnDisk {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} changed on disk while Sussurro was saving it (edited in another app?). \
             Nothing was overwritten: reopen the item to load the latest version, then \
             make the change again.",
            self.0.display()
        )
    }
}

impl std::error::Error for ChangedOnDisk {}

/// Replace `transcript.md` in `dir` with `doc`, but only if the file still
/// hashes to `expected_sha` (the bytes the caller read and based `doc` on).
/// The new file is staged first, so the gap between the check and the
/// rename is as small as the filesystem allows. On a mismatch — someone
/// saved the file meanwhile, e.g. from Obsidian — nothing is overwritten
/// and a clear error is returned. With `record_state`, `state.json` is
/// updated to the new hash in the same critical section.
///
/// `before_commit` runs after staging, right before the check (a test hook
/// to simulate an external save in that window).
pub(super) fn commit_transcript(
    dir: &Path,
    doc: &[u8],
    expected_sha: &str,
    record_state: bool,
    before_commit: &dyn Fn(&Path),
) -> Result<()> {
    let path = transcript_path(dir);
    let tmp = write_temp(&path, doc)?;
    before_commit(&path);
    let _guard = TRANSCRIPT_COMMIT.lock().unwrap_or_else(|e| e.into_inner());
    let unchanged = std::fs::read(&path)
        .map(|now| sha256_hex(&now) == expected_sha)
        .unwrap_or(false);
    if !unchanged {
        let _ = std::fs::remove_file(&tmp);
        return Err(ChangedOnDisk(path).into());
    }
    rename_into(&tmp, &path)?;
    if record_state {
        write_state(dir, doc)?;
    }
    Ok(())
}

pub(super) fn transcript_path(dir: &Path) -> PathBuf {
    dir.join(TRANSCRIPT_FILE)
}

fn read_state(dir: &Path) -> ItemState {
    std::fs::read_to_string(dir.join(META_DIR).join(STATE_FILE))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

pub(super) fn write_state(dir: &Path, transcript: &[u8]) -> Result<()> {
    let meta_dir = dir.join(META_DIR);
    std::fs::create_dir_all(&meta_dir)?;
    let state = ItemState {
        transcript_sha256: sha256_hex(transcript),
    };
    write_atomic(
        &meta_dir.join(STATE_FILE),
        serde_json::to_string_pretty(&state)?.as_bytes(),
    )
}

/// Whether `transcript` differs from what the app last wrote. No stored hash
/// (a folder made by hand, or state lost) counts as edited: never overwrite
/// a file of unknown provenance.
pub(super) fn is_edited_externally(dir: &Path, transcript: &[u8]) -> bool {
    let state = read_state(dir);
    state.transcript_sha256.is_empty() || state.transcript_sha256 != sha256_hex(transcript)
}

pub(super) fn read_segments(dir: &Path) -> Result<SegmentsFile> {
    let path = dir.join(META_DIR).join(SEGMENTS_FILE);
    match std::fs::read_to_string(&path) {
        Ok(s) => serde_json::from_str(&s).with_context(|| format!("parsing {}", path.display())),
        // An item written by hand has only transcript.md.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(SegmentsFile::default()),
        Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
    }
}

/// What voice profiles (#241) need of the item at `dir`: its frontmatter
/// `source` and its segments — or `None` when no speaker of it is linked
/// to a person (checked on the raw bytes first, so a scan of the whole
/// archive parses only the documents that can contribute).
pub(crate) fn read_linked_segments(dir: &Path) -> Result<Option<(String, SegmentsFile)>> {
    let path = dir.join(META_DIR).join(SEGMENTS_FILE);
    let bytes = match std::fs::read(&path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
    };
    if !bytes.windows(11).any(|w| w == b"\"person_id\"") {
        return Ok(None);
    }
    let segments: SegmentsFile =
        serde_json::from_slice(&bytes).with_context(|| format!("parsing {}", path.display()))?;
    if segments.speakers.iter().all(|s| s.person_id.is_none()) {
        return Ok(None);
    }
    let doc = std::fs::read(transcript_path(dir))
        .with_context(|| format!("reading {}", transcript_path(dir).display()))?;
    let (meta, _) = frontmatter::parse(&String::from_utf8_lossy(&doc))?;
    Ok(Some((meta.source, segments)))
}

pub(super) fn write_segments(dir: &Path, segments: &SegmentsFile) -> Result<()> {
    let meta_dir = dir.join(META_DIR);
    std::fs::create_dir_all(&meta_dir)?;
    let mut segments = segments.clone();
    segments.version = SEGMENTS_VERSION;
    write_atomic(
        &meta_dir.join(SEGMENTS_FILE),
        serde_json::to_string(&segments)?.as_bytes(),
    )
}

/// Folder of an existing item: the id must be valid, confined, and point at a
/// folder holding a `transcript.md` (so an id like `2026` can never address a
/// whole year of items).
pub(crate) fn existing_item_dir(archive: &Path, id: &str) -> Result<PathBuf> {
    let dir = item_dir(archive, id)?;
    if !transcript_path(&dir).is_file() {
        bail!("no archive item '{id}'");
    }
    Ok(dir)
}

/// Parse a frontmatter date (RFC 3339) into the calendar date *as written*
/// (its own offset), which names the item folder.
fn item_date(date: &str) -> Result<chrono::NaiveDate> {
    chrono::DateTime::parse_from_rfc3339(date.trim())
        .map(|d| d.date_naive())
        .with_context(|| format!("item date '{date}' is not RFC 3339"))
}

/// Create a new item folder and write its files. An empty `meta.date` is set
/// to now. Returns the new item's id.
pub fn create_item(archive: &Path, meta: &ItemMeta, segments: &SegmentsFile) -> Result<String> {
    let mut meta = meta.clone();
    if meta.date.trim().is_empty() {
        meta.date = chrono::Local::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, false);
    }
    let date = item_date(&meta.date)?;
    let slug = slugify(&meta.title);
    let parent_rel = month_dir(date);
    let parent = item_dir(archive, &parent_rel)?;
    std::fs::create_dir_all(&parent).with_context(|| format!("creating {}", parent.display()))?;

    // create_dir (not _all) is the atomic claim on a name: whoever creates
    // it first owns it, so concurrent saves can't share a folder.
    let mut claimed = None;
    for n in 1..=MAX_COLLISIONS {
        let name = folder_name(date, &slug, n);
        let dir = parent.join(&name);
        match std::fs::create_dir(&dir) {
            Ok(()) => {
                claimed = Some((format!("{parent_rel}/{name}"), dir));
                break;
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e).with_context(|| format!("creating {}", dir.display())),
        }
    }
    let Some((id, dir)) = claimed else {
        bail!("too many items named '{slug}' on {date}");
    };

    let write_all = || -> Result<()> {
        write_segments(&dir, segments)?;
        let doc = render_transcript(&meta, segments)?;
        write_atomic(&transcript_path(&dir), doc.as_bytes())?;
        write_state(&dir, doc.as_bytes())
    };
    if let Err(e) = write_all() {
        // The folder was created by this call a moment ago and holds only
        // what we just wrote — safe to remove so no half-item lingers.
        let _ = std::fs::remove_dir_all(&dir);
        return Err(e);
    }
    Ok(id)
}

/// Read an item: metadata, segments, markdown body, external-edit flag.
pub fn read_item(archive: &Path, id: &str) -> Result<Item> {
    let dir = existing_item_dir(archive, id)?;
    read_item_at(id, &dir)
}

fn read_item_at(id: &str, dir: &Path) -> Result<Item> {
    let bytes = std::fs::read(transcript_path(dir))
        .with_context(|| format!("reading {}", transcript_path(dir).display()))?;
    let doc = String::from_utf8_lossy(&bytes);
    let (mut meta, body) = frontmatter::parse(&doc)?;
    if meta.title.trim().is_empty() {
        meta.title = fallback_title(&body, dir);
    }
    let state = meta.session_state();
    let segments = read_segments(dir)?;
    Ok(Item {
        id: id.to_string(),
        meta,
        embedded_segments: segments
            .segments
            .iter()
            .filter(|s| s.embedding.is_some())
            .count(),
        segments,
        body,
        edited_externally: is_edited_externally(dir, &bytes),
        recording: state == Some(SessionState::Recording),
        interrupted: state == Some(SessionState::Interrupted),
        external_hosts: super::external::sent_hosts_at(dir),
        audio: super::audio::files_with_sizes(dir),
        folder_bytes: super::audio::folder_bytes(dir),
    })
}

/// Title for a file without one: its first `# ` heading, else the folder name.
fn fallback_title(body: &str, dir: &Path) -> String {
    body.lines()
        .find_map(|l| l.strip_prefix("# ").map(|t| t.trim().to_string()))
        .filter(|t| !t.is_empty())
        .or_else(|| dir.file_name().map(|n| n.to_string_lossy().into_owned()))
        .unwrap_or_default()
}

/// Summary of the item at `dir` (no segments), for listing and indexing.
pub(crate) fn summary_at(id: &str, dir: &Path) -> Result<(ItemSummary, String)> {
    let bytes = std::fs::read(transcript_path(dir))
        .with_context(|| format!("reading {}", transcript_path(dir).display()))?;
    let doc = String::from_utf8_lossy(&bytes);
    let (mut meta, body) = frontmatter::parse(&doc)?;
    if meta.title.trim().is_empty() {
        meta.title = fallback_title(&body, dir);
    }
    let edited = is_edited_externally(dir, &bytes);
    Ok((ItemSummary::new(id.to_string(), meta, edited, None), body))
}

/// Every item folder under `archive`: `(id, folder)`. Dot-dirs and symlinks
/// are skipped (no escaping the archive, no loops); a folder holding a
/// `transcript.md` is an item and is not descended into. A missing archive
/// yields nothing.
pub fn scan_item_dirs(archive: &Path) -> Vec<(String, PathBuf)> {
    fn walk(archive: &Path, dir: &Path, depth: usize, out: &mut Vec<(String, PathBuf)>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let name = entry.file_name();
            if name.to_string_lossy().starts_with('.') {
                continue;
            }
            // DirEntry::file_type does not follow symlinks.
            let Ok(ft) = entry.file_type() else { continue };
            if !ft.is_dir() {
                continue;
            }
            let path = entry.path();
            if transcript_path(&path).is_file() {
                if let Some(id) = id_from_dir(archive, &path) {
                    out.push((id, path));
                }
            } else if depth < MAX_SCAN_DEPTH {
                walk(archive, &path, depth + 1, out);
            }
        }
    }
    let mut out = Vec::new();
    walk(archive, archive, 1, &mut out);
    out
}

/// Sort key: parsed date (newest first), unparseable dates last, then id.
pub(crate) fn sort_newest_first(items: &mut [ItemSummary]) {
    items.sort_by(|a, b| {
        let da = chrono::DateTime::parse_from_rfc3339(a.meta.date.trim()).ok();
        let db = chrono::DateTime::parse_from_rfc3339(b.meta.date.trim()).ok();
        db.cmp(&da).then_with(|| b.id.cmp(&a.id))
    });
}

/// All items, newest first, by scanning the folder. Broken items (unreadable
/// file, invalid YAML) are logged and skipped — one bad file must not hide
/// the rest of the archive.
pub fn list_items(archive: &Path) -> Vec<ItemSummary> {
    let mut items: Vec<ItemSummary> = scan_item_dirs(archive)
        .into_iter()
        .filter_map(|(id, dir)| match summary_at(&id, &dir) {
            Ok((mut s, _)) => {
                s.fill_folder_details(&dir);
                Some(s)
            }
            Err(e) => {
                eprintln!("archive: skipping broken item {id}: {e:#}");
                None
            }
        })
        .collect();
    sort_newest_first(&mut items);
    items
}

/// Replace an item's metadata. The folder is never renamed (ids stay stable
/// for links). Frontmatter keys unknown to the app that `meta` doesn't carry
/// are kept from the file. If the file is unchanged since the app wrote it,
/// the whole transcript is regenerated (the `# title` follows the new title);
/// after an external edit only the frontmatter is replaced and the user's
/// body is kept verbatim. Returns the updated item.
///
/// The capture-session marker (`status: recording|interrupted`, #153) is
/// owned by the engine: a marker value in `meta` is ignored and the file's
/// marker is kept, so a UI sending back a stale copy can neither resurrect
/// `recording` on a finished item nor clear it on a live one. The list of
/// saved audio files (`audio:`, #141) is app-owned the same way: only the
/// engine, "Delete audio" and "Compress audio" (#248) change it.
///
/// Participants are written normalized ([`normalize_participants`]). Notes
/// never get participants (P10): an update that would add or change them on
/// a note is refused. A note whose frontmatter already lists participants
/// (typed by hand outside the app) may send them back unchanged, so editing
/// its title or tags still works and the user's list is kept as is.
///
/// Refused (nothing written) when the current frontmatter is not valid YAML
/// — replacing it would silently drop the user's edits there — and when the
/// file changes on disk while the update is being written (#155).
pub fn update_meta(archive: &Path, id: &str, meta: &ItemMeta) -> Result<Item> {
    update_meta_with(archive, id, meta, &|_| {})
}

fn update_meta_with(
    archive: &Path,
    id: &str,
    meta: &ItemMeta,
    before_commit: &dyn Fn(&Path),
) -> Result<Item> {
    let _lock = lock_items();
    let dir = existing_item_dir(archive, id)?;
    let path = transcript_path(&dir);
    let bytes = std::fs::read(&path).with_context(|| format!("reading {}", path.display()))?;
    let current = String::from_utf8_lossy(&bytes);
    let (old, _) = frontmatter::parse(&current).map_err(|e| {
        anyhow::anyhow!(
            "the frontmatter of {} can't be read ({e:#}), so Sussurro won't replace it — \
             that would drop your edits there. Fix the YAML between the two `---` lines \
             in a text editor (or delete that block to start over), then try again.",
            path.display()
        )
    })?;
    let mut meta = meta.clone();
    meta.participants = normalize_participants(&meta.participants);
    check_note_participants(&old, &meta)?;
    // The file's own marker (if any) comes back with the merge below.
    if meta.session_state().is_some() {
        meta.extra.remove(SESSION_KEY);
    }
    meta.extra.remove(super::audio::AUDIO_KEY);
    // Keys the UI doesn't know (e.g. Obsidian's `aliases`) are kept.
    for (k, v) in old.extra {
        meta.extra.entry(k).or_insert(v);
    }
    let expected = sha256_hex(&bytes);
    if is_edited_externally(&dir, &bytes) {
        let doc = frontmatter::replace(&current, &meta)?;
        // State hash deliberately untouched: the file stays "edited outside".
        commit_transcript(&dir, doc.as_bytes(), &expected, false, before_commit)?;
    } else {
        let segments = read_segments(&dir)?;
        let doc = render_transcript(&meta, &segments)?;
        commit_transcript(&dir, doc.as_bytes(), &expected, true, before_commit)?;
    }
    read_item_at(id, &dir)
}

/// P10: participants belong to meetings and transcriptions. A note may only
/// keep, untouched, participants its file already had as a note.
fn check_note_participants(old: &ItemMeta, new: &ItemMeta) -> Result<()> {
    if new.item_type.has_participants() || new.participants.is_empty() {
        return Ok(());
    }
    let unchanged = !old.item_type.has_participants()
        && normalize_participants(&old.participants) == new.participants;
    if !unchanged {
        bail!(
            "notes have no participants — participants belong to meetings and transcriptions. \
             Remove them, or change the item's type first."
        );
    }
    Ok(())
}

/// Store new segments. `segments.json` is always written; `transcript.md`
/// is regenerated only if it is still what the app last wrote. Returns
/// `true` when the transcript was regenerated, `false` when it was left
/// alone because it was edited outside the app. If the transcript changes
/// on disk while it is being regenerated, it is left alone and an error is
/// returned (`segments.json` is saved all the same).
pub fn save_segments(archive: &Path, id: &str, segments: &SegmentsFile) -> Result<bool> {
    save_segments_with(archive, id, segments, &|_| {})
}

fn save_segments_with(
    archive: &Path,
    id: &str,
    segments: &SegmentsFile,
    before_commit: &dyn Fn(&Path),
) -> Result<bool> {
    let _lock = lock_items();
    let dir = existing_item_dir(archive, id)?;
    write_segments(&dir, segments)?;
    rerender_if_unchanged_with(&dir, segments, before_commit)
}

/// Regenerate `transcript.md` from its own frontmatter and `segments` if it
/// is still what the app last wrote (the content-hash rule). Returns whether
/// it was regenerated. The replace itself goes through the freshness check
/// of [`commit_transcript`] (#155). Callers hold [`lock_items`].
pub(super) fn rerender_if_unchanged(dir: &Path, segments: &SegmentsFile) -> Result<bool> {
    rerender_if_unchanged_with(dir, segments, &|_| {})
}

fn rerender_if_unchanged_with(
    dir: &Path,
    segments: &SegmentsFile,
    before_commit: &dyn Fn(&Path),
) -> Result<bool> {
    let path = transcript_path(dir);
    let bytes = std::fs::read(&path).with_context(|| format!("reading {}", path.display()))?;
    if is_edited_externally(dir, &bytes) {
        return Ok(false);
    }
    let (meta, _) = frontmatter::parse(&String::from_utf8_lossy(&bytes))?;
    let doc = render_transcript(&meta, segments)?;
    commit_transcript(
        dir,
        doc.as_bytes(),
        &sha256_hex(&bytes),
        true,
        before_commit,
    )
    .context("the segments were saved, but transcript.md was not regenerated")?;
    Ok(true)
}

/// A change to one transcript line, made from the app's line editor.
#[derive(Debug, Clone, PartialEq)]
pub enum SegmentEdit {
    /// Replace the cleaned text (the raw STT text is kept for reference).
    Text(String),
    /// Remove the segment.
    Delete,
}

/// Returned when a line edit is refused because `transcript.md` changed
/// outside the app: the markdown wins and the app does not overwrite it.
pub const EDITED_OUTSIDE_ERROR: &str = "edited outside Sussurro";

/// Edit or delete one segment and regenerate `transcript.md`. Refused when
/// the transcript was edited outside the app (the content-hash rule): the
/// markdown wins, so a line edit that could never reach it is not saved
/// either. Returns the updated item.
pub fn edit_segment(archive: &Path, id: &str, segment_id: u32, edit: SegmentEdit) -> Result<Item> {
    edit_segment_with(archive, id, segment_id, edit, &|_| {})
}

/// [`edit_segment`] with the [`commit_transcript`] test hook.
fn edit_segment_with(
    archive: &Path,
    id: &str,
    segment_id: u32,
    edit: SegmentEdit,
    before_commit: &dyn Fn(&Path),
) -> Result<Item> {
    modify_segments_with(archive, id, before_commit, |_, segments| {
        let pos = segments
            .segments
            .iter()
            .position(|s| s.id == segment_id)
            .with_context(|| format!("no line {segment_id} in '{id}'"))?;
        match edit {
            SegmentEdit::Text(text) => {
                let text = text.trim();
                if text.is_empty() {
                    bail!("a line can't be empty — delete it instead");
                }
                let seg = &mut segments.segments[pos];
                if seg.text != text {
                    seg.text = text.to_string();
                    seg.edited = true;
                    // Typed in by hand over a stretch STT could not transcribe.
                    seg.stt_error = None;
                }
            }
            SegmentEdit::Delete => {
                segments.segments.remove(pos);
            }
        }
        Ok(())
    })
}

/// A change to a document's speakers, from the speaker panel (#130).
#[derive(Debug, Clone, PartialEq)]
pub enum SpeakerEdit {
    /// Move one line to another speaker of the document;
    /// [`crate::speakers::doc::NEW_VOICE`] opens a new "Voice N".
    Move { segment_id: u32, speaker_id: String },
    /// Rename a speaker for this document only (empty: a voice gets its
    /// "Voice N" name back).
    Rename { speaker_id: String, label: String },
    /// "Re-detect speakers": re-cluster the whole document offline from
    /// the stored embeddings.
    Redetect,
    /// Link a speaker to a person of the People registry (#132): the
    /// person's name as label (unless renamed) and a participant with
    /// name + email in the frontmatter.
    Link { speaker_id: String, person_id: String },
    /// Undo a link; the speaker gets its previous label back.
    Unlink { speaker_id: String },
}

/// Apply a [`SpeakerEdit`] and regenerate `transcript.md`, under the same
/// rules as a line edit ([`edit_segment`]): refused when the transcript
/// was edited outside the app or the item is still being recorded, and
/// undone if the markdown changes on disk meanwhile. Returns the updated
/// item.
pub fn edit_speakers(archive: &Path, id: &str, edit: SpeakerEdit) -> Result<Item> {
    edit_speakers_with(archive, id, edit, &|_| {})
}

fn edit_speakers_with(
    archive: &Path,
    id: &str,
    edit: SpeakerEdit,
    before_commit: &dyn Fn(&Path),
) -> Result<Item> {
    use crate::speakers::doc;
    // Read before taking the item lock (the registry has its own).
    let person = match &edit {
        SpeakerEdit::Link { person_id, .. } => Some(
            super::people::list_people(archive)?
                .into_iter()
                .find(|p| &p.id == person_id)
                .ok_or_else(|| anyhow::anyhow!("that person is no longer in People"))?,
        ),
        _ => None,
    };
    modify_segments_with(archive, id, before_commit, |meta, segments| {
        match edit {
            SpeakerEdit::Move {
                segment_id,
                speaker_id,
            } => {
                doc::move_segment(segments, segment_id, &speaker_id)?;
            }
            SpeakerEdit::Rename { speaker_id, label } => {
                doc::rename_speaker(segments, &speaker_id, &label)?;
            }
            SpeakerEdit::Redetect => {
                doc::redetect(segments)?;
            }
            SpeakerEdit::Link { speaker_id, .. } => {
                let person = person.as_ref().expect("resolved above");
                doc::link_speaker(segments, meta, &speaker_id, person)?;
            }
            SpeakerEdit::Unlink { speaker_id } => {
                doc::unlink_speaker(segments, &speaker_id)?;
            }
        }
        Ok(())
    })
}

/// "Re-detect speakers" ([`SpeakerEdit::Redetect`]), then the user's
/// enrolled voice labels its best match "You" (#243,
/// [`crate::speakers::own_voice::label_you`]). Same rules as
/// [`edit_speakers`].
pub fn redetect_with_own_voice(archive: &Path, id: &str, you: &[f32]) -> Result<Item> {
    modify_segments_with(archive, id, &|_| {}, |meta, segments| {
        crate::speakers::doc::redetect(segments)?;
        crate::speakers::own_voice::label_you(segments, &meta.source, you);
        Ok(())
    })
}

/// *Find my voice* on one document (#243): its best match of the user's
/// enrolled voice becomes "You" ([`crate::speakers::own_voice::label_you`]).
/// Returns the item and whether a voice is "You" now. Refused where "You"
/// can't be told by voice (a two-channel recording, where the mic is "You"
/// already) and on documents without voice data. Same rules as
/// [`edit_speakers`].
pub fn label_own_voice(archive: &Path, id: &str, you: &[f32]) -> Result<(Item, bool)> {
    use crate::speakers::own_voice;
    let mut found = false;
    let item = modify_segments_with(archive, id, &|_| {}, |meta, segments| {
        if !own_voice::single_channel(segments, &meta.source) {
            bail!("in this recording your voice is on its own channel: it is \"You\" already");
        }
        if !segments.segments.iter().any(|s| s.embedding.is_some()) {
            bail!("this document has no voice data to look for your voice in");
        }
        own_voice::label_you(segments, &meta.source, you);
        found = segments.speakers.iter().any(|s| s.own_voice == Some(true));
        Ok(())
    })?;
    Ok((item, found))
}

/// [`modify_segments_with`] for the app's editors outside this module
/// ("Identify voices" on a transcription, #134): same rules as
/// [`edit_speakers`]. Returns the updated item.
pub(crate) fn modify_segments(
    archive: &Path,
    id: &str,
    change: impl FnOnce(&mut SegmentsFile) -> Result<()>,
) -> Result<Item> {
    modify_segments_with(archive, id, &|_| {}, |_meta, segments| change(segments))
}

/// Read-modify-write of an item's `segments.json` from the app's editors
/// (lines, speakers), with `transcript.md` regenerated in step; `change`
/// may also edit the frontmatter (a speaker link adds a participant; the
/// capture-session marker is never touched). Refused
/// when the transcript was edited outside the app (the content-hash rule:
/// the markdown wins, so a change that could never reach it is not saved
/// either) and while a capture session writes the item (#153). `change`
/// failing leaves everything as it was.
fn modify_segments_with(
    archive: &Path,
    id: &str,
    before_commit: &dyn Fn(&Path),
    change: impl FnOnce(&mut ItemMeta, &mut SegmentsFile) -> Result<()>,
) -> Result<Item> {
    // Same lock as the engine's checkpoints and update_meta: the check and
    // the write must not interleave with another writer of this item.
    let _lock = lock_items();
    let dir = existing_item_dir(archive, id)?;
    let path = transcript_path(&dir);
    let bytes = std::fs::read(&path).with_context(|| format!("reading {}", path.display()))?;
    if is_edited_externally(&dir, &bytes) {
        bail!(
            "'{id}' was {EDITED_OUTSIDE_ERROR}: its transcript.md is kept as is — edit it there"
        );
    }
    let (mut meta, _) = frontmatter::parse(&String::from_utf8_lossy(&bytes))?;
    // A live item is still being written by its capture session (#153).
    if meta.session_state() == Some(SessionState::Recording) {
        bail!("'{id}' is still being recorded — edit it when the session ends");
    }
    let original = read_segments(&dir)?;
    let mut segments = original.clone();
    change(&mut meta, &mut segments)?;
    // Rendered from the very bytes checked above, and replaced only if the
    // file still holds them (#155): an external save that lands meanwhile
    // wins, and the change is undone in segments.json too — it could
    // never reach the markdown, so it is not kept anywhere.
    let doc = render_transcript(&meta, &segments)?;
    write_segments(&dir, &segments)?;
    if let Err(e) = commit_transcript(
        &dir,
        doc.as_bytes(),
        &sha256_hex(&bytes),
        true,
        before_commit,
    ) {
        // Undo only if the markdown was not replaced (#158): when the
        // replace went through and only recording its hash failed, the
        // edit is in transcript.md, and rolling segments.json back would
        // leave the two telling different stories.
        let replaced = std::fs::read(&path)
            .map(|now| now == doc.as_bytes())
            .unwrap_or(false);
        if replaced {
            return Err(e.context(
                "the line was saved, but its state could not be recorded — the item \
                 now counts as edited outside Sussurro",
            ));
        }
        if let Err(undo) = write_segments(&dir, &original) {
            return Err(e.context(format!("segments.json could not be restored ({undo:#})")));
        }
        return Err(e);
    }
    read_item_at(id, &dir)
}

/// Move an item folder to the OS trash (never a hard delete). Refused while
/// the item is marked `recording` (#158): a capture session is writing it —
/// possibly another app instance's, which this process can't stop. Ending
/// the session (or restarting after a crash, which marks it `interrupted`)
/// makes it deletable.
pub fn delete_item(archive: &Path, id: &str) -> Result<()> {
    delete_finished_item_with(archive, id, move_to_trash)
}

fn delete_finished_item_with(
    archive: &Path,
    id: &str,
    trash: impl FnOnce(&Path) -> Result<()>,
) -> Result<()> {
    // Same lock as the session's checkpoints: the check and the delete
    // don't interleave with a writer of this item.
    let _lock = lock_items();
    let dir = existing_item_dir(archive, id)?;
    let path = transcript_path(&dir);
    let bytes = std::fs::read(&path).with_context(|| format!("reading {}", path.display()))?;
    let recording = frontmatter::parse(&String::from_utf8_lossy(&bytes))
        .map(|(m, _)| m.session_state() == Some(SessionState::Recording))
        .unwrap_or(false);
    if recording {
        bail!("'{id}' is still being recorded — stop the session before deleting it");
    }
    delete_item_with(archive, id, trash)
}

/// [`delete_item`] with an injectable trash mover (tests must not fill the
/// developer's real trash). Empty `YYYY/MM` parents are removed afterwards.
pub fn delete_item_with(
    archive: &Path,
    id: &str,
    trash: impl FnOnce(&Path) -> Result<()>,
) -> Result<()> {
    let dir = existing_item_dir(archive, id)?;
    trash(&dir)?;
    // remove_dir only succeeds on empty folders: nothing else can go.
    let root = archive
        .canonicalize()
        .unwrap_or_else(|_| archive.to_path_buf());
    let mut parent = dir.parent().map(Path::to_path_buf);
    while let Some(p) = parent {
        let canon = p.canonicalize().unwrap_or_else(|_| p.clone());
        if canon == root || !canon.starts_with(&root) || std::fs::remove_dir(&p).is_err() {
            break;
        }
        parent = p.parent().map(Path::to_path_buf);
    }
    Ok(())
}

/// The OS trash. On macOS through `NSFileManager` rather than the crate's
/// default Finder/AppleScript route, which would prompt for Automation
/// permission.
///
/// In unit tests nothing reaches the developer's real trash: the folder is
/// recorded in [`test_trash`] and removed, so tests can assert that a path
/// went "to the trash" rather than being hard-deleted.
pub fn move_to_trash(path: &Path) -> Result<()> {
    #[cfg(test)]
    {
        test_trash::take(path)
    }
    #[cfg(not(test))]
    {
        #[allow(unused_mut)]
        let mut ctx = trash::TrashContext::default();
        #[cfg(target_os = "macos")]
        {
            use trash::macos::{DeleteMethod, TrashContextExtMacos};
            ctx.set_delete_method(DeleteMethod::NsFileManager);
        }
        ctx.delete(path)
            .with_context(|| format!("moving {} to the trash", path.display()))
    }
}

/// The trash as unit tests see it (see [`move_to_trash`]).
#[cfg(test)]
pub(crate) mod test_trash {
    use std::path::{Path, PathBuf};
    use std::sync::Mutex;

    static TRASHED: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());

    pub(crate) fn take(path: &Path) -> anyhow::Result<()> {
        TRASHED
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(path.to_path_buf());
        if path.is_dir() {
            std::fs::remove_dir_all(path)?;
        } else {
            std::fs::remove_file(path)?;
        }
        Ok(())
    }

    /// Whether `path` was moved to the (test) trash. Tests run in parallel:
    /// ask about paths under your own tempdir.
    pub(crate) fn contains(path: &Path) -> bool {
        TRASHED
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .any(|p| p == path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::archive::types::{ItemType, Participant, Segment};

    fn meta(title: &str, date: &str) -> ItemMeta {
        ItemMeta {
            item_type: ItemType::Note,
            title: title.into(),
            date: date.into(),
            source: "mic".into(),
            language: "it".into(),
            engine: "whisper-small".into(),
            tags: vec!["idea".into()],
            ..Default::default()
        }
    }

    fn segs(texts: &[&str]) -> SegmentsFile {
        SegmentsFile {
            segments: texts
                .iter()
                .enumerate()
                .map(|(i, t)| Segment {
                    id: i as u32,
                    start_ms: i as u64 * 1000,
                    end_ms: i as u64 * 1000 + 900,
                    raw: t.to_string(),
                    text: t.to_string(),
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        }
    }

    const DATE: &str = "2026-09-24T10:00:00+02:00";

    #[test]
    fn create_lays_out_folder_and_read_roundtrips() {
        let tmp = tempfile::tempdir().unwrap();
        let archive = tmp.path().join("Sussurro");
        let id = create_item(
            &archive,
            &meta("Idea sul perché", DATE),
            &segs(&["Uno.", "Due."]),
        )
        .unwrap();
        assert_eq!(id, "2026/09/2026-09-24-idea-sul-perche");
        let dir = archive.join("2026/09/2026-09-24-idea-sul-perche");
        assert!(dir.join("transcript.md").is_file());
        assert!(dir.join(".sussurro/segments.json").is_file());
        assert!(dir.join(".sussurro/state.json").is_file());

        let item = read_item(&archive, &id).unwrap();
        assert_eq!(item.meta, meta("Idea sul perché", DATE));
        assert_eq!(item.segments.segments.len(), 2);
        assert_eq!(item.body, "\n# Idea sul perché\n\nUno. Due.\n");
        assert!(!item.edited_externally);
        // No temp files left behind.
        let leftovers: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().contains(".tmp-"))
            .collect();
        assert!(leftovers.is_empty());
    }

    #[test]
    fn create_is_collision_safe_and_fills_missing_date() {
        let tmp = tempfile::tempdir().unwrap();
        let a = create_item(tmp.path(), &meta("Sync", DATE), &segs(&[])).unwrap();
        let b = create_item(tmp.path(), &meta("Sync", DATE), &segs(&[])).unwrap();
        let c = create_item(tmp.path(), &meta("Sync", DATE), &segs(&[])).unwrap();
        assert_eq!(a, "2026/09/2026-09-24-sync");
        assert_eq!(b, "2026/09/2026-09-24-sync-2");
        assert_eq!(c, "2026/09/2026-09-24-sync-3");

        let now = create_item(tmp.path(), &meta("", ""), &segs(&[])).unwrap();
        assert!(now.ends_with("-untitled"), "{now}");
        let item = read_item(tmp.path(), &now).unwrap();
        assert!(chrono::DateTime::parse_from_rfc3339(&item.meta.date).is_ok());
    }

    #[test]
    fn create_uses_the_date_as_written_not_utc() {
        let tmp = tempfile::tempdir().unwrap();
        // 00:30 in Rome is still the previous day in UTC.
        let id = create_item(
            tmp.path(),
            &meta("x", "2026-01-01T00:30:00+01:00"),
            &segs(&[]),
        )
        .unwrap();
        assert_eq!(id, "2026/01/2026-01-01-x");
    }

    #[test]
    fn create_rejects_bad_dates() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(create_item(tmp.path(), &meta("x", "yesterday"), &segs(&[])).is_err());
    }

    #[test]
    fn read_rejects_non_items_and_traversal() {
        let tmp = tempfile::tempdir().unwrap();
        let id = create_item(tmp.path(), &meta("x", DATE), &segs(&[])).unwrap();
        assert!(read_item(tmp.path(), &id).is_ok());
        assert!(read_item(tmp.path(), "2026").is_err()); // a year folder, not an item
        assert!(read_item(tmp.path(), "../x").is_err());
        assert!(read_item(tmp.path(), "2026/09/missing").is_err());
    }

    #[test]
    fn save_segments_regenerates_until_edited_externally() {
        let tmp = tempfile::tempdir().unwrap();
        let archive = tmp.path();
        let id = create_item(archive, &meta("Nota", DATE), &segs(&["Uno."])).unwrap();
        assert!(save_segments(archive, &id, &segs(&["Uno.", "Due."])).unwrap());
        assert!(read_item(archive, &id).unwrap().body.contains("Uno. Due."));

        // The user edits the markdown in another editor.
        let path = archive.join(&id).join("transcript.md");
        let edited = std::fs::read_to_string(&path)
            .unwrap()
            .replace("Due.", "Due, a mano.");
        std::fs::write(&path, &edited).unwrap();

        assert!(!save_segments(archive, &id, &segs(&["Tre."])).unwrap());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), edited);
        let item = read_item(archive, &id).unwrap();
        assert!(item.edited_externally);
        // segments.json still took the new data.
        assert_eq!(item.segments.segments[0].text, "Tre.");
    }

    #[test]
    fn update_meta_regenerates_when_untouched() {
        let tmp = tempfile::tempdir().unwrap();
        let archive = tmp.path();
        let id = create_item(archive, &meta("Vecchio", DATE), &segs(&["Testo."])).unwrap();
        let mut m = read_item(archive, &id).unwrap().meta;
        m.title = "Nuovo".into();
        m.tags = vec!["a".into(), "b".into()];
        let item = update_meta(archive, &id, &m).unwrap();
        assert_eq!(item.id, id); // folder not renamed
        assert_eq!(item.meta.tags, vec!["a", "b"]);
        assert!(item.body.contains("# Nuovo"));
        assert!(!item.edited_externally);
        // Still app-owned: later segment saves keep regenerating.
        assert!(save_segments(archive, &id, &segs(&["Altro."])).unwrap());
    }

    #[test]
    fn update_meta_keeps_external_body_and_unknown_keys() {
        let tmp = tempfile::tempdir().unwrap();
        let archive = tmp.path();
        let id = create_item(archive, &meta("Titolo", DATE), &segs(&["Testo."])).unwrap();
        let path = archive.join(&id).join("transcript.md");
        let doc = std::fs::read_to_string(&path).unwrap();
        // The user adds an Obsidian key and rewrites the body.
        let doc = doc
            .replacen("---\n", "---\naliases: [promemoria]\n", 1)
            .replace("Testo.", "Testo riscritto dall'utente.");
        std::fs::write(&path, &doc).unwrap();

        // The UI sends meta without the unknown key (it never saw it).
        let mut m = meta("Titolo", DATE);
        m.categories = vec!["lavoro".into()];
        let item = update_meta(archive, &id, &m).unwrap();
        assert!(item.edited_externally);
        assert!(item.body.contains("Testo riscritto dall'utente."));
        assert_eq!(item.meta.categories, vec!["lavoro"]);
        assert_eq!(
            item.meta.extra["aliases"],
            serde_json::json!(["promemoria"])
        );
        // And the app still refuses to regenerate it.
        assert!(!save_segments(archive, &id, &segs(&["X."])).unwrap());
    }

    #[test]
    fn edit_segment_updates_text_marks_edited_and_regenerates() {
        let tmp = tempfile::tempdir().unwrap();
        let archive = tmp.path();
        let mut m = meta("Riunione", DATE);
        m.item_type = ItemType::Transcription;
        let id = create_item(archive, &m, &segs(&["Uno.", "Due.", "Tre."])).unwrap();

        let item = edit_segment(archive, &id, 1, SegmentEdit::Text("  Due, corretto. ".into()))
            .unwrap();
        let s = &item.segments.segments[1];
        assert_eq!(s.text, "Due, corretto.");
        assert_eq!(s.raw, "Due."); // raw STT kept
        assert!(s.edited);
        assert!(!item.segments.segments[0].edited);
        assert!(item.body.contains("[00:00:01] Due, corretto."), "{}", item.body);
        assert!(!item.edited_externally); // still app-owned

        // Same text again: nothing changes, not flagged as a new edit.
        let again = edit_segment(archive, &id, 0, SegmentEdit::Text("Uno.".into())).unwrap();
        assert!(!again.segments.segments[0].edited);

        let item = edit_segment(archive, &id, 0, SegmentEdit::Delete).unwrap();
        let ids: Vec<u32> = item.segments.segments.iter().map(|s| s.id).collect();
        assert_eq!(ids, vec![1, 2]);
        assert!(!item.body.contains("Uno."));

        // Unknown line, empty text and non-items are refused.
        assert!(edit_segment(archive, &id, 42, SegmentEdit::Delete).is_err());
        assert!(edit_segment(archive, &id, 1, SegmentEdit::Text("  ".into())).is_err());
        assert!(edit_segment(archive, "2026", 1, SegmentEdit::Delete).is_err());
    }

    #[test]
    fn edit_segment_refuses_live_items_and_clears_stt_errors() {
        let tmp = tempfile::tempdir().unwrap();
        let archive = tmp.path();
        let mut live = meta("Live", DATE);
        live.set_session_state(Some(SessionState::Recording));
        let id = create_item(archive, &live, &segs(&["Uno."])).unwrap();
        let err = edit_segment(archive, &id, 0, SegmentEdit::Delete).unwrap_err();
        assert!(format!("{err:#}").contains("still being recorded"), "{err:#}");

        let mut failed = segs(&["", "Due."]);
        failed.segments[0].stt_error = Some("model crashed".into());
        let id = create_item(archive, &meta("Buco", DATE), &failed).unwrap();
        let item =
            edit_segment(archive, &id, 0, SegmentEdit::Text("Uno, a mano.".into())).unwrap();
        let s = &item.segments.segments[0];
        assert_eq!(s.text, "Uno, a mano.");
        assert!(s.stt_error.is_none() && s.edited);
    }

    #[test]
    fn edit_segment_refuses_items_edited_outside() {
        let tmp = tempfile::tempdir().unwrap();
        let archive = tmp.path();
        let id = create_item(archive, &meta("Nota", DATE), &segs(&["Uno.", "Due."])).unwrap();
        let path = archive.join(&id).join("transcript.md");
        let edited = std::fs::read_to_string(&path).unwrap().replace("Due.", "Due!");
        std::fs::write(&path, &edited).unwrap();
        let before = std::fs::read(archive.join(&id).join(".sussurro/segments.json")).unwrap();

        let err = edit_segment(archive, &id, 0, SegmentEdit::Delete).unwrap_err();
        assert!(format!("{err:#}").contains(EDITED_OUTSIDE_ERROR), "{err:#}");
        // Neither file was touched.
        assert_eq!(std::fs::read_to_string(&path).unwrap(), edited);
        let after = std::fs::read(archive.join(&id).join(".sussurro/segments.json")).unwrap();
        assert_eq!(before, after);
    }

    #[test]
    fn edit_segment_does_not_overwrite_a_concurrent_external_save() {
        let tmp = tempfile::tempdir().unwrap();
        let archive = tmp.path();
        let id = create_item(archive, &meta("Gara", DATE), &segs(&["Uno.", "Due."])).unwrap();
        let path = archive.join(&id).join("transcript.md");
        let seg_path = archive.join(&id).join(".sussurro/segments.json");
        let before = std::fs::read(&seg_path).unwrap();
        // Obsidian saves the file between the line edit's check and its write.
        let external = "---\ntitle: Gara\n---\nRiscritto a mano.\n";
        let err = edit_segment_with(
            archive,
            &id,
            1,
            SegmentEdit::Text("Due, corretto.".into()),
            &|p| std::fs::write(p, external).unwrap(),
        )
        .unwrap_err();
        assert!(format!("{err:#}").contains("changed on disk"), "{err:#}");
        // The markdown wins, and the line edit is not kept in segments.json.
        assert_eq!(std::fs::read_to_string(&path).unwrap(), external);
        assert_eq!(std::fs::read(&seg_path).unwrap(), before);
        assert!(temp_leftovers(&archive.join(&id)).is_empty());
        assert!(read_item(archive, &id).unwrap().edited_externally);
    }

    /// #158 finding 5: when `transcript.md` was replaced but recording its
    /// hash in `state.json` failed, the line edit is in the markdown — so
    /// `segments.json` must keep it too, not be rolled back.
    #[test]
    fn a_failed_state_write_after_the_replace_keeps_segments_in_step() {
        let tmp = tempfile::tempdir().unwrap();
        let archive = tmp.path();
        let id = create_item(archive, &meta("Stato", DATE), &segs(&["Uno.", "Due."])).unwrap();
        let dir = archive.join(&id);
        let state = dir.join(META_DIR).join(STATE_FILE);
        let err = edit_segment_with(
            archive,
            &id,
            1,
            SegmentEdit::Text("Due, corretto.".into()),
            // state.json can't be replaced any more (a folder in its place).
            &|_| {
                std::fs::remove_file(&state).unwrap();
                std::fs::create_dir_all(state.join("blocker")).unwrap();
            },
        )
        .unwrap_err();
        assert!(format!("{err:#}").contains("the line was saved"), "{err:#}");
        let markdown = std::fs::read_to_string(dir.join(TRANSCRIPT_FILE)).unwrap();
        assert!(markdown.contains("Due, corretto."), "{markdown}");
        let saved = read_segments(&dir).unwrap();
        assert_eq!(
            saved.segments[1].text, "Due, corretto.",
            "in step with the markdown"
        );
        assert!(saved.segments[1].edited);
    }

    #[test]
    fn edit_segment_waits_for_a_live_session_to_finish() {
        use crate::archive::live;
        let tmp = tempfile::tempdir().unwrap();
        let archive = tmp.path();
        let id = live::begin_session(archive, &meta("Live", DATE)).unwrap();
        live::checkpoint(archive, &id, &segs(&["Uno.", "Due."]), true).unwrap();
        let err = edit_segment(archive, &id, 0, SegmentEdit::Delete).unwrap_err();
        assert!(format!("{err:#}").contains("still being recorded"), "{err:#}");
        assert_eq!(read_item(archive, &id).unwrap().segments.segments.len(), 2);

        let fallback = meta("Live", DATE);
        live::finish_session(archive, &id, &segs(&["Uno.", "Due."]), &fallback, |m| m).unwrap();
        let item = edit_segment(archive, &id, 0, SegmentEdit::Text("Uno!".into())).unwrap();
        assert!(!item.recording && !item.edited_externally);
        assert!(item.body.contains("Uno!"), "{}", item.body);
    }

    #[test]
    fn hand_made_item_without_state_is_never_overwritten() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("2025/01/mia-nota");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("transcript.md"), "# Mia nota\n\nScritta a mano.\n").unwrap();
        let item = read_item(tmp.path(), "2025/01/mia-nota").unwrap();
        assert!(item.edited_externally);
        assert_eq!(item.meta.title, "Mia nota");
        assert!(item.segments.segments.is_empty());
        assert!(!save_segments(tmp.path(), "2025/01/mia-nota", &segs(&["x"])).unwrap());
        assert_eq!(
            std::fs::read_to_string(dir.join("transcript.md")).unwrap(),
            "# Mia nota\n\nScritta a mano.\n"
        );
    }

    #[test]
    fn list_scans_sorts_and_skips_broken_items() {
        let tmp = tempfile::tempdir().unwrap();
        let archive = tmp.path();
        assert!(list_items(&archive.join("missing")).is_empty());
        let old = create_item(archive, &meta("Old", "2025-01-01T09:00:00Z"), &segs(&[])).unwrap();
        let new = create_item(archive, &meta("New", "2026-03-01T09:00:00Z"), &segs(&[])).unwrap();
        // Broken YAML: skipped, not fatal.
        let broken = archive.join("2026/03/broken");
        std::fs::create_dir_all(&broken).unwrap();
        std::fs::write(broken.join("transcript.md"), "---\ntitle: [oops\n---\n").unwrap();
        // Dot-dirs and plain files are ignored.
        std::fs::create_dir_all(archive.join(".sussurro/fake")).unwrap();
        std::fs::write(archive.join(".sussurro/fake/transcript.md"), "# no\n").unwrap();
        std::fs::write(archive.join("README.md"), "hi").unwrap();

        let ids: Vec<String> = list_items(archive).into_iter().map(|s| s.id).collect();
        assert_eq!(ids, vec![new, old]);
    }

    #[test]
    fn delete_moves_the_folder_and_prunes_empty_parents() {
        let tmp = tempfile::tempdir().unwrap();
        let archive = tmp.path().join("arch");
        let bin = tmp.path().join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let a = create_item(&archive, &meta("A", DATE), &segs(&[])).unwrap();
        let b = create_item(&archive, &meta("B", "2025-05-05T10:00:00Z"), &segs(&[])).unwrap();
        let fake_trash = |p: &Path| -> Result<()> {
            std::fs::rename(p, bin.join(p.file_name().unwrap()))?;
            Ok(())
        };
        delete_item_with(&archive, &a, fake_trash).unwrap();
        assert!(bin.join("2026-09-24-a").join("transcript.md").is_file());
        assert!(!archive.join("2026").exists()); // empty 2026/09 pruned
        assert!(archive.join("2025/05").exists()); // other items untouched
        assert!(archive.exists()); // never the root
        assert!(read_item(&archive, &b).is_ok());
        // Non-items and traversal are refused before the trash is touched.
        let never = |_: &Path| -> Result<()> { panic!("must not trash") };
        assert!(delete_item_with(&archive, "2025", never).is_err());
        assert!(delete_item_with(&archive, "../bin", never).is_err());
    }

    /// #158 finding 2: the backend refuses to delete an item a capture
    /// session is writing (the UI only greys the button out); once the
    /// session has ended it goes to the trash as usual.
    #[test]
    fn delete_refuses_a_recording_item() {
        let tmp = tempfile::tempdir().unwrap();
        let archive = tmp.path().join("arch");
        let id = crate::archive::live::begin_session(&archive, &meta("Live", DATE)).unwrap();
        let never = |_: &Path| -> Result<()> { panic!("must not trash") };
        let err = delete_finished_item_with(&archive, &id, never).unwrap_err();
        assert!(
            format!("{err:#}").contains("still being recorded"),
            "{err:#}"
        );
        assert!(delete_item(&archive, &id).is_err());
        assert!(read_item(&archive, &id).is_ok());

        crate::archive::live::mark_interrupted(&archive, &id).unwrap();
        delete_item(&archive, &id).unwrap();
        assert!(test_trash::contains(&archive.join(&id)));
    }

    fn temp_leftovers(dir: &Path) -> Vec<String> {
        std::fs::read_dir(dir)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.contains(".tmp-"))
            .collect()
    }

    #[test]
    fn temp_names_are_unique_per_call() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("transcript.md");
        let names: Vec<PathBuf> = std::thread::scope(|s| {
            let handles: Vec<_> = (0..8)
                .map(|_| {
                    s.spawn(|| {
                        (0..500)
                            .map(|_| temp_path(&path).unwrap())
                            .collect::<Vec<_>>()
                    })
                })
                .collect();
            handles
                .into_iter()
                .flat_map(|h| h.join().unwrap())
                .collect()
        });
        let unique: std::collections::HashSet<_> = names.iter().collect();
        assert_eq!(unique.len(), names.len());
        let first = names[0].file_name().unwrap().to_string_lossy().into_owned();
        assert!(first.starts_with(".transcript.md.tmp-"), "{first}");

        // Concurrent atomic writes of one file: all succeed, the result is
        // one whole payload, and no temp file is left behind.
        let payloads: Vec<String> = (0..8).map(|i| format!("{i}").repeat(4096)).collect();
        std::thread::scope(|s| {
            for p in &payloads {
                let path = &path;
                s.spawn(move || {
                    for _ in 0..25 {
                        write_atomic(path, p.as_bytes()).unwrap();
                    }
                });
            }
        });
        let result = std::fs::read_to_string(&path).unwrap();
        assert!(payloads.contains(&result));
        assert!(temp_leftovers(tmp.path()).is_empty());
    }

    #[test]
    fn update_meta_does_not_overwrite_a_concurrent_external_save() {
        let tmp = tempfile::tempdir().unwrap();
        let archive = tmp.path();
        for untouched in [true, false] {
            let id = create_item(archive, &meta("Gara", DATE), &segs(&["Testo."])).unwrap();
            let path = archive.join(&id).join("transcript.md");
            if !untouched {
                // Already edited outside: only the frontmatter would change.
                let doc = std::fs::read_to_string(&path).unwrap() + "\nAppunto.\n";
                std::fs::write(&path, doc).unwrap();
            }
            let mut m = read_item(archive, &id).unwrap().meta;
            m.title = "Nuovo titolo".into();
            // Obsidian saves the file between our read and our write.
            let external = "---\ntitle: Salvato da Obsidian\n---\nTesto esterno.\n";
            let err = update_meta_with(archive, &id, &m, &|p| std::fs::write(p, external).unwrap())
                .unwrap_err();
            assert!(format!("{err:#}").contains("changed on disk"), "{err:#}");
            assert_eq!(std::fs::read_to_string(&path).unwrap(), external);
            assert!(temp_leftovers(&archive.join(&id)).is_empty());
            assert!(read_item(archive, &id).unwrap().edited_externally);
        }
    }

    #[test]
    fn save_segments_does_not_overwrite_a_concurrent_external_save() {
        let tmp = tempfile::tempdir().unwrap();
        let archive = tmp.path();
        let id = create_item(archive, &meta("Gara", DATE), &segs(&["Uno."])).unwrap();
        let path = archive.join(&id).join("transcript.md");
        let external = "---\ntitle: Gara\n---\nRiscritto a mano.\n";
        let err = save_segments_with(archive, &id, &segs(&["Due."]), &|p| {
            std::fs::write(p, external).unwrap()
        })
        .unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("changed on disk"), "{msg}");
        assert!(msg.contains("segments were saved"), "{msg}");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), external);
        let item = read_item(archive, &id).unwrap();
        assert!(item.edited_externally);
        assert_eq!(item.segments.segments[0].text, "Due.");
        assert!(temp_leftovers(&archive.join(&id)).is_empty());
    }

    fn person(name: &str, email: Option<&str>) -> Participant {
        Participant {
            name: name.into(),
            email: email.map(str::to_string),
        }
    }

    #[test]
    fn transcription_participants_round_trip_through_the_frontmatter() {
        let tmp = tempfile::tempdir().unwrap();
        let archive = tmp.path();
        let mut m = meta("Podcast", DATE);
        m.item_type = ItemType::Transcription;
        let id = create_item(archive, &m, &segs(&["Benvenuti."])).unwrap();

        let mut m = read_item(archive, &id).unwrap().meta;
        m.participants = vec![
            person("  Anna Rossi ", Some(" anna@example.com ")),
            person("Ospite", Some("  ")),
            person("anna rossi", Some("ANNA@example.com")), // repeat
            person("   ", Some("nobody@example.com")),      // no name
        ];
        let item = update_meta(archive, &id, &m).unwrap();
        let want = vec![
            person("Anna Rossi", Some("anna@example.com")),
            person("Ospite", None),
        ];
        assert_eq!(item.meta.participants, want);
        assert!(!item.edited_externally);

        // Written to the file and read back from it.
        let path = archive.join(&id).join("transcript.md");
        let doc = std::fs::read_to_string(&path).unwrap();
        assert!(doc.contains("participants:"), "{doc}");
        assert!(doc.contains("anna@example.com"), "{doc}");
        assert_eq!(read_item(archive, &id).unwrap().meta.participants, want);

        // Edit one, then remove them all.
        m.participants = vec![person("Anna Rossi", Some("anna.rossi@example.com"))];
        let item = update_meta(archive, &id, &m).unwrap();
        assert_eq!(
            item.meta.participants[0].email.as_deref(),
            Some("anna.rossi@example.com")
        );
        m.participants.clear();
        let item = update_meta(archive, &id, &m).unwrap();
        assert!(item.meta.participants.is_empty());
        assert!(!std::fs::read_to_string(&path).unwrap().contains("anna"));
    }

    #[test]
    fn update_meta_refuses_participants_on_a_note() {
        let tmp = tempfile::tempdir().unwrap();
        let archive = tmp.path();
        let id = create_item(archive, &meta("Idea", DATE), &segs(&["Testo."])).unwrap();
        let path = archive.join(&id).join("transcript.md");
        let before = std::fs::read_to_string(&path).unwrap();

        let mut m = read_item(archive, &id).unwrap().meta;
        m.participants = vec![person("Anna", None)];
        let msg = format!("{:#}", update_meta(archive, &id, &m).unwrap_err());
        assert!(msg.contains("notes have no participants"), "{msg}");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), before);

        // A transcription can't carry its participants into a note either.
        let mut t = meta("Intervista", DATE);
        t.item_type = ItemType::Transcription;
        t.participants = vec![person("Anna", None)];
        let tid = create_item(archive, &t, &segs(&["Ciao."])).unwrap();
        let mut t = read_item(archive, &tid).unwrap().meta;
        t.item_type = ItemType::Note;
        assert!(update_meta(archive, &tid, &t).is_err());
        t.participants.clear();
        let item = update_meta(archive, &tid, &t).unwrap();
        assert_eq!(item.meta.item_type, ItemType::Note);
        assert!(item.meta.participants.is_empty());
    }

    #[test]
    fn a_note_keeps_participants_written_by_hand() {
        let tmp = tempfile::tempdir().unwrap();
        let archive = tmp.path();
        let id = create_item(archive, &meta("Idea", DATE), &segs(&["Testo."])).unwrap();
        let path = archive.join(&id).join("transcript.md");
        let doc: String = std::fs::read_to_string(&path)
            .unwrap()
            .lines()
            .map(|l| match l.starts_with("participants:") {
                true => "participants: [Anna]\n".to_string(),
                false => format!("{l}\n"),
            })
            .collect();
        assert!(doc.contains("[Anna]"), "{doc}");
        std::fs::write(&path, doc).unwrap();

        // The UI round-trips the list it got: a title edit still works.
        let mut m = read_item(archive, &id).unwrap().meta;
        assert_eq!(m.participants, vec![person("Anna", None)]);
        m.title = "Idea migliore".into();
        let item = update_meta(archive, &id, &m).unwrap();
        assert_eq!(item.meta.title, "Idea migliore");
        assert_eq!(item.meta.participants, vec![person("Anna", None)]);

        // Changing them is still refused.
        m.participants.push(person("Bob", None));
        assert!(update_meta(archive, &id, &m).is_err());
    }

    #[test]
    fn update_meta_refuses_unparseable_frontmatter() {
        let tmp = tempfile::tempdir().unwrap();
        let archive = tmp.path();
        let id = create_item(archive, &meta("Rotto", DATE), &segs(&["Testo."])).unwrap();
        let path = archive.join(&id).join("transcript.md");
        let broken = "---\ntitle: [oops\naliases: [mio]\n---\n# Rotto\n\nTesto.\n";
        std::fs::write(&path, broken).unwrap();
        let err = update_meta(archive, &id, &meta("Nuovo", DATE)).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("won't replace it"), "{msg}");
        assert!(msg.contains("---"), "{msg}");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), broken);
    }

    /// A meeting with two voices and embeddings on every line (#130).
    fn voiced_meeting(archive: &Path) -> String {
        use crate::speakers::cluster::tests::{sample, voices};
        use crate::speakers::doc::voice_speaker;
        let (mut rng, v) = voices(2, 8);
        let truth = [0usize, 1, 0, 1, 0, 1];
        let mut m = meta("Riunione", DATE);
        m.item_type = ItemType::Meeting;
        let segments = SegmentsFile {
            speakers: vec![voice_speaker(1), voice_speaker(2)],
            segments: truth
                .iter()
                .enumerate()
                .map(|(i, &t)| Segment {
                    id: i as u32,
                    start_ms: i as u64 * 5000,
                    end_ms: i as u64 * 5000 + 4000,
                    speaker_id: Some(format!("voice:{}", t + 1)),
                    text: format!("Frase {i}."),
                    embedding: Some(sample(&mut rng, &v[t], 0.9)),
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        };
        create_item(archive, &m, &segments).unwrap()
    }

    #[test]
    fn speaker_moves_and_renames_persist_in_segments_and_markdown() {
        let tmp = tempfile::tempdir().unwrap();
        let archive = tmp.path();
        let id = voiced_meeting(archive);
        let item = read_item(archive, &id).unwrap();
        assert_eq!(item.embedded_segments, 6);
        assert!(item.body.contains("**[00:00:05] Voice 2:** Frase 1."), "{}", item.body);

        let move_to = |seg: u32, sp: &str| SpeakerEdit::Move {
            segment_id: seg,
            speaker_id: sp.into(),
        };
        edit_speakers(archive, &id, move_to(1, "voice:1")).unwrap();
        let rename = SpeakerEdit::Rename {
            speaker_id: "voice:1".into(),
            label: "Anna".into(),
        };
        let item = edit_speakers(archive, &id, rename).unwrap();
        assert!(item.body.contains("**[00:00:05] Anna:** Frase 1."), "{}", item.body);
        assert!(!item.edited_externally);

        // On disk, not only in the returned item.
        let dir = archive.join(&id);
        let saved = read_segments(&dir).unwrap();
        assert_eq!(saved.segments[1].speaker_id.as_deref(), Some("voice:1"));
        assert_eq!(saved.speakers[0].label, "Anna");
        assert!(saved.segments.iter().all(|s| s.embedding.is_some()));
        let markdown = std::fs::read_to_string(dir.join(TRANSCRIPT_FILE)).unwrap();
        assert!(markdown.contains("**[00:00:00] Anna:** Frase 0."), "{markdown}");

        // A bad target changes nothing.
        let before = std::fs::read(dir.join(META_DIR).join(SEGMENTS_FILE)).unwrap();
        assert!(edit_speakers(archive, &id, move_to(1, "voice:9")).is_err());
        assert_eq!(std::fs::read(dir.join(META_DIR).join(SEGMENTS_FILE)).unwrap(), before);
    }

    #[test]
    fn redetect_undoes_a_wrong_move_and_is_idempotent_on_disk() {
        let tmp = tempfile::tempdir().unwrap();
        let archive = tmp.path();
        let id = voiced_meeting(archive);
        let original = read_segments(&archive.join(&id)).unwrap();
        edit_speakers(
            archive,
            &id,
            SpeakerEdit::Move {
                segment_id: 2,
                speaker_id: "voice:2".into(),
            },
        )
        .unwrap();
        let item = edit_speakers(archive, &id, SpeakerEdit::Redetect).unwrap();
        assert_eq!(item.segments, original);
        let again = edit_speakers(archive, &id, SpeakerEdit::Redetect).unwrap();
        assert_eq!(again.segments, original);
        assert_eq!(again.body, item.body);
        // The UI copy carries no embeddings; the file keeps them.
        let ui = again.without_embeddings();
        assert!(ui.segments.segments.iter().all(|s| s.embedding.is_none()));
        assert_eq!(ui.embedded_segments, 6);
    }

    #[test]
    fn speaker_edits_follow_the_freshness_rules() {
        let tmp = tempfile::tempdir().unwrap();
        let archive = tmp.path();
        let id = voiced_meeting(archive);
        let path = archive.join(&id).join(TRANSCRIPT_FILE);
        let seg_path = archive.join(&id).join(META_DIR).join(SEGMENTS_FILE);
        let before = std::fs::read(&seg_path).unwrap();

        // Saved in another app between the check and the write: the
        // markdown wins and the re-detect is not kept.
        let external = "---\ntitle: Riunione\n---\nRiscritto.\n";
        let err = edit_speakers_with(archive, &id, SpeakerEdit::Redetect, &|p| {
            std::fs::write(p, external).unwrap()
        })
        .unwrap_err();
        assert!(format!("{err:#}").contains("changed on disk"), "{err:#}");
        assert_eq!(std::fs::read(&seg_path).unwrap(), before);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), external);

        // From now on the item counts as edited outside: refused up front.
        let rename = SpeakerEdit::Rename {
            speaker_id: "voice:1".into(),
            label: "Anna".into(),
        };
        let err = edit_speakers(archive, &id, rename).unwrap_err();
        assert!(format!("{err:#}").contains(EDITED_OUTSIDE_ERROR), "{err:#}");
        assert_eq!(std::fs::read(&seg_path).unwrap(), before);
    }

    #[test]
    fn linking_a_voice_updates_segments_and_frontmatter() {
        use crate::archive::people::{add_person, modify, Person};
        let tmp = tempfile::tempdir().unwrap();
        let archive = tmp.path();
        let id = voiced_meeting(archive);
        let anna = modify(archive, |ps| {
            add_person(
                ps,
                &Person {
                    name: "Anna Rossi".into(),
                    email: Some("anna@example.com".into()),
                    ..Default::default()
                },
            )
        })
        .unwrap();
        let link = SpeakerEdit::Link {
            speaker_id: "voice:2".into(),
            person_id: anna.id.clone(),
        };
        let item = edit_speakers(archive, &id, link).unwrap();
        assert!(item.body.contains("**[00:00:05] Anna Rossi:** Frase 1."), "{}", item.body);
        assert_eq!(item.meta.participants.len(), 1);
        assert_eq!(item.meta.participants[0].email.as_deref(), Some("anna@example.com"));
        // On disk: the frontmatter and segments.json agree.
        let again = read_item(archive, &id).unwrap();
        assert_eq!(again.meta.participants, item.meta.participants);
        assert_eq!(again.segments.speakers[1].person_id.as_deref(), Some(anna.id.as_str()));
        assert!(!again.edited_externally);

        let item = edit_speakers(
            archive,
            &id,
            SpeakerEdit::Unlink {
                speaker_id: "voice:2".into(),
            },
        )
        .unwrap();
        assert_eq!(item.segments.speakers[1].label, "Voice 2");
        assert!(item.segments.speakers[1].person_id.is_none());

        // A person no longer in People: refused, nothing written.
        let gone = SpeakerEdit::Link {
            speaker_id: "voice:1".into(),
            person_id: "p-gone".into(),
        };
        let err = edit_speakers(archive, &id, gone).unwrap_err();
        assert!(format!("{err:#}").contains("no longer in People"), "{err:#}");
    }

    #[test]
    fn speaker_edits_wait_for_a_live_session() {
        use crate::archive::live;
        let tmp = tempfile::tempdir().unwrap();
        let archive = tmp.path();
        let mut m = meta("Live", DATE);
        m.item_type = ItemType::Meeting;
        let id = live::begin_session(archive, &m).unwrap();
        let err = edit_speakers(archive, &id, SpeakerEdit::Redetect).unwrap_err();
        assert!(format!("{err:#}").contains("still being recorded"), "{err:#}");
    }
}
