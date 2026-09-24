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
use super::types::{ItemMeta, SegmentsFile, SEGMENTS_VERSION};
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

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

/// Write via a dot-prefixed temp file + rename, so a crash or a sync client
/// never sees a half-written document.
fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let dir = path.parent().context("path without parent")?;
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .context("path without file name")?;
    let tmp = dir.join(format!(".{name}.tmp-{}", std::process::id()));
    std::fs::write(&tmp, bytes).with_context(|| format!("writing {}", tmp.display()))?;
    std::fs::rename(&tmp, path).with_context(|| {
        let _ = std::fs::remove_file(&tmp);
        format!("replacing {}", path.display())
    })
}

fn transcript_path(dir: &Path) -> PathBuf {
    dir.join(TRANSCRIPT_FILE)
}

fn read_state(dir: &Path) -> ItemState {
    std::fs::read_to_string(dir.join(META_DIR).join(STATE_FILE))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn write_state(dir: &Path, transcript: &[u8]) -> Result<()> {
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
fn is_edited_externally(dir: &Path, transcript: &[u8]) -> bool {
    let state = read_state(dir);
    state.transcript_sha256.is_empty() || state.transcript_sha256 != sha256_hex(transcript)
}

fn read_segments(dir: &Path) -> Result<SegmentsFile> {
    let path = dir.join(META_DIR).join(SEGMENTS_FILE);
    match std::fs::read_to_string(&path) {
        Ok(s) => serde_json::from_str(&s).with_context(|| format!("parsing {}", path.display())),
        // An item written by hand has only transcript.md.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(SegmentsFile::default()),
        Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
    }
}

fn write_segments(dir: &Path, segments: &SegmentsFile) -> Result<()> {
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
fn existing_item_dir(archive: &Path, id: &str) -> Result<PathBuf> {
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
    Ok(Item {
        id: id.to_string(),
        meta,
        segments: read_segments(dir)?,
        body,
        edited_externally: is_edited_externally(dir, &bytes),
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
    Ok((
        ItemSummary {
            id: id.to_string(),
            meta,
            edited_externally: is_edited_externally(dir, &bytes),
            snippet: None,
        },
        body,
    ))
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
            Ok((s, _)) => Some(s),
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
pub fn update_meta(archive: &Path, id: &str, meta: &ItemMeta) -> Result<Item> {
    let dir = existing_item_dir(archive, id)?;
    let path = transcript_path(&dir);
    let bytes = std::fs::read(&path).with_context(|| format!("reading {}", path.display()))?;
    let current = String::from_utf8_lossy(&bytes);
    let mut meta = meta.clone();
    // A user-broken frontmatter is simply replaced; extras are merged only
    // when the old one still parses.
    if let Ok((old, _)) = frontmatter::parse(&current) {
        for (k, v) in old.extra {
            meta.extra.entry(k).or_insert(v);
        }
    }
    if is_edited_externally(&dir, &bytes) {
        let doc = frontmatter::replace(&current, &meta)?;
        write_atomic(&path, doc.as_bytes())?;
        // State hash deliberately untouched: the file stays "edited outside".
    } else {
        let segments = read_segments(&dir)?;
        let doc = render_transcript(&meta, &segments)?;
        write_atomic(&path, doc.as_bytes())?;
        write_state(&dir, doc.as_bytes())?;
    }
    read_item_at(id, &dir)
}

/// Store new segments. `segments.json` is always written; `transcript.md`
/// is regenerated only if it is still what the app last wrote. Returns
/// `true` when the transcript was regenerated, `false` when it was left
/// alone because it was edited outside the app.
pub fn save_segments(archive: &Path, id: &str, segments: &SegmentsFile) -> Result<bool> {
    let dir = existing_item_dir(archive, id)?;
    write_segments(&dir, segments)?;
    let path = transcript_path(&dir);
    let bytes = std::fs::read(&path).with_context(|| format!("reading {}", path.display()))?;
    if is_edited_externally(&dir, &bytes) {
        return Ok(false);
    }
    let (meta, _) = frontmatter::parse(&String::from_utf8_lossy(&bytes))?;
    let doc = render_transcript(&meta, segments)?;
    write_atomic(&path, doc.as_bytes())?;
    write_state(&dir, doc.as_bytes())?;
    Ok(true)
}

/// Move an item folder to the OS trash (never a hard delete).
pub fn delete_item(archive: &Path, id: &str) -> Result<()> {
    delete_item_with(archive, id, move_to_trash)
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
pub fn move_to_trash(path: &Path) -> Result<()> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::archive::types::{ItemType, Segment};

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
}
