//! Items written while a long-form session runs (#153), so a crash, a
//! forced quit or a power loss keeps what was transcribed so far.
//!
//! Lifecycle of a live item:
//! 1. [`begin_session`] creates the folder at session start with the marker
//!    `status: recording` in the frontmatter and no segments.
//! 2. [`checkpoint`] rewrites `.sussurro/segments.json` atomically after
//!    every finished segment and, when asked, regenerates `transcript.md`
//!    (only while it is still what the app last wrote — the content-hash
//!    rule of `store`).
//! 3. [`finish_session`] on a normal stop: final segments, duration and the
//!    other end-of-run metadata, marker removed, final render.
//!    [`discard_session`] on a user cancel or when nothing was said.
//! 4. After a crash the item is left with `status: recording`; at the next
//!    start the app calls [`mark_interrupted`], which turns it into
//!    `status: interrupted` and keeps the saved segments.
//!
//! Every function here takes the store's item lock, so a checkpoint never
//! interleaves with a metadata edit from the UI.

use super::frontmatter;
use super::paths::{folder_name, item_dir, slugify};
use super::render::{format_timestamp, render_transcript};
use super::store::{
    commit_transcript, create_item, delete_item_with, existing_item_dir, is_edited_externally,
    lock_items, read_segments, rerender_if_unchanged, sha256_hex, transcript_path, write_segments,
    ChangedOnDisk,
};
use super::types::{ItemMeta, SegmentsFile, SessionState};
use anyhow::{Context, Result};
use std::path::Path;

/// Make sure items can be written under `archive`: create the folder and
/// write (then remove) a probe file. Called before a session captures any
/// audio, so a folder the OS won't let us write (macOS asks for Documents
/// access the first time; a read-only or unplugged custom folder) fails
/// the start with a clear message instead of costing a recording.
pub fn ensure_writable(archive: &Path) -> Result<()> {
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    // Unique per call: a mic session and a file may start together.
    let probe = archive.join(format!(
        ".sussurro-write-test-{}-{}",
        std::process::id(),
        SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    std::fs::create_dir_all(archive)
        .and_then(|()| std::fs::write(&probe, b"ok"))
        .and_then(|()| std::fs::remove_file(&probe))
        .with_context(|| {
            format!(
                "cannot write to the archive folder {} — pick another folder in \
                 Settings or allow Sussurro to access it (macOS: System Settings → \
                 Privacy & Security → Files and Folders)",
                archive.display()
            )
        })
}

/// Create the item for a session that is starting: `meta` plus the
/// `recording` marker, no segments. Returns the item id.
pub fn begin_session(archive: &Path, meta: &ItemMeta) -> Result<String> {
    let mut meta = meta.clone();
    meta.set_session_state(Some(SessionState::Recording));
    create_item(archive, &meta, &SegmentsFile::default())
}

/// Save the segments so far (atomic replace of `segments.json`) and, with
/// `render`, regenerate `transcript.md` if the user has not edited it.
pub fn checkpoint(archive: &Path, id: &str, segments: &SegmentsFile, render: bool) -> Result<()> {
    let _lock = lock_items();
    let dir = existing_item_dir(archive, id)?;
    write_segments(&dir, segments)?;
    if render {
        rerender_if_unchanged(&dir, segments)?;
    }
    Ok(())
}

/// How often a session-owned rewrite re-reads the file and tries again when
/// it changed on disk between the read and the replace (#155).
const REWRITE_ATTEMPTS: usize = 3;

/// Rewrite the frontmatter through `update`. An app-owned transcript is
/// regenerated from `segments`; one edited outside the app keeps its body
/// and only gets the new frontmatter (its hash stays stale, so it stays
/// "edited outside"). A frontmatter the user broke is replaced by
/// `fallback` when one is given, otherwise the file is left alone.
///
/// The replace goes through the store's freshness check, so an external
/// save that lands mid-write is never overwritten. The session owns the
/// marker and the end-of-run metadata, though, so it is the last writer:
/// on such a race the file is read again — now counted as edited outside,
/// so the user's body is kept and only the frontmatter is replaced — and
/// the rewrite is retried.
fn rewrite_meta(
    dir: &Path,
    segments: &SegmentsFile,
    fallback: Option<&ItemMeta>,
    update: &dyn Fn(ItemMeta) -> ItemMeta,
    before_commit: &dyn Fn(&Path),
) -> Result<ItemMeta> {
    let mut attempt = 1;
    loop {
        match rewrite_meta_once(dir, segments, fallback, update, before_commit) {
            Err(e) if attempt < REWRITE_ATTEMPTS && e.downcast_ref::<ChangedOnDisk>().is_some() => {
                attempt += 1;
            }
            result => return result,
        }
    }
}

fn rewrite_meta_once(
    dir: &Path,
    segments: &SegmentsFile,
    fallback: Option<&ItemMeta>,
    update: &dyn Fn(ItemMeta) -> ItemMeta,
    before_commit: &dyn Fn(&Path),
) -> Result<ItemMeta> {
    let path = transcript_path(dir);
    let bytes = std::fs::read(&path).with_context(|| format!("reading {}", path.display()))?;
    let current = String::from_utf8_lossy(&bytes);
    let meta = match (frontmatter::parse(&current), fallback) {
        (Ok((m, _)), _) => m,
        (Err(_), Some(f)) => f.clone(),
        (Err(e), None) => return Err(e),
    };
    let meta = update(meta);
    let expected = sha256_hex(&bytes);
    if is_edited_externally(dir, &bytes) {
        let doc = frontmatter::replace(&current, &meta)?;
        commit_transcript(dir, doc.as_bytes(), &expected, false, before_commit)?;
    } else {
        let doc = render_transcript(&meta, segments)?;
        commit_transcript(dir, doc.as_bytes(), &expected, true, before_commit)?;
    }
    Ok(meta)
}

/// Finalize a live item on a normal stop: write the final segments, apply
/// `finalize` (duration, title, language…) to the metadata as it is in the
/// file now (so edits made during the session survive), clear the marker
/// and render. `fallback` stands in for a frontmatter the user broke.
/// Returns the final metadata.
pub fn finish_session(
    archive: &Path,
    id: &str,
    segments: &SegmentsFile,
    fallback: &ItemMeta,
    finalize: impl Fn(ItemMeta) -> ItemMeta,
) -> Result<ItemMeta> {
    finish_session_with(archive, id, segments, fallback, &finalize, &|_| {})
}

fn finish_session_with(
    archive: &Path,
    id: &str,
    segments: &SegmentsFile,
    fallback: &ItemMeta,
    finalize: &dyn Fn(ItemMeta) -> ItemMeta,
    before_commit: &dyn Fn(&Path),
) -> Result<ItemMeta> {
    let _lock = lock_items();
    let dir = existing_item_dir(archive, id)?;
    write_segments(&dir, segments)?;
    rewrite_meta(
        &dir,
        segments,
        Some(fallback),
        &|m| {
            let mut m = finalize(m);
            m.set_session_state(None);
            m
        },
        before_commit,
    )
}

/// Turn a `recording` item left behind by a crash into an `interrupted`
/// one, keeping its saved segments; a missing duration is set from the
/// last saved segment. Returns `false` (and changes nothing) when the item
/// is not marked `recording`.
pub fn mark_interrupted(archive: &Path, id: &str) -> Result<bool> {
    let _lock = lock_items();
    let dir = existing_item_dir(archive, id)?;
    let path = transcript_path(&dir);
    let doc = std::fs::read_to_string(&path).unwrap_or_default();
    let recording = frontmatter::parse(&doc)
        .map(|(m, _)| m.session_state() == Some(SessionState::Recording))
        .unwrap_or(false);
    if !recording {
        return Ok(false);
    }
    // A corrupt segments.json (should not happen: it is replaced
    // atomically) must not block the recovery of the item itself.
    let segments = read_segments(&dir).unwrap_or_else(|e| {
        eprintln!("archive: {id}: unreadable segments kept aside ({e:#})");
        SegmentsFile::default()
    });
    rewrite_meta(
        &dir,
        &segments,
        None,
        &|mut m| {
            m.set_session_state(Some(SessionState::Interrupted));
            if m.duration.is_none() {
                if let Some(last) = segments.segments.iter().map(|s| s.end_ms).max() {
                    m.duration = Some(format_timestamp(last));
                }
            }
            m
        },
        &|_| {},
    )?;
    Ok(true)
}

/// Drop a live item the user cancelled (or that captured no speech). The
/// folder was created by this session, so it is removed outright — unless
/// the user edited `transcript.md` meanwhile: then it is kept, marked
/// `interrupted`. Returns whether the folder was removed.
pub fn discard_session(archive: &Path, id: &str) -> Result<bool> {
    let _lock = lock_items();
    let dir = existing_item_dir(archive, id)?;
    let bytes = std::fs::read(transcript_path(&dir))?;
    if is_edited_externally(&dir, &bytes) {
        drop(_lock);
        mark_interrupted(archive, id)?;
        return Ok(false);
    }
    delete_item_with(archive, id, |p| {
        std::fs::remove_dir_all(p).with_context(|| format!("removing {}", p.display()))
    })?;
    Ok(true)
}

/// Rename a live item's folder after its final title (a session started
/// without a title gets a `…-untitled` folder, named before any text
/// existed). Same month folder, same date prefix, collision suffixes as on
/// create. Returns the (possibly unchanged) id; on any failure the item
/// keeps its old id — a name is not worth losing a transcript over.
pub fn retitle_folder(archive: &Path, id: &str, title: &str) -> String {
    let _lock = lock_items();
    let rename = || -> Result<String> {
        let dir = existing_item_dir(archive, id)?;
        let (parent_id, name) = id.rsplit_once('/').context("item at the archive root")?;
        let date = name
            .get(..10)
            .and_then(|d| chrono::NaiveDate::parse_from_str(d, "%Y-%m-%d").ok())
            .context("folder name without a date")?;
        let slug = slugify(title);
        for n in 1..=999 {
            let candidate = folder_name(date, &slug, n);
            if candidate == name {
                return Ok(id.to_string());
            }
            let new_id = format!("{parent_id}/{candidate}");
            let target = item_dir(archive, &new_id)?;
            if target.exists() {
                continue;
            }
            std::fs::rename(&dir, &target)
                .with_context(|| format!("renaming {} to {}", dir.display(), target.display()))?;
            return Ok(new_id);
        }
        anyhow::bail!("too many items named '{slug}'")
    };
    rename().unwrap_or_else(|e| {
        eprintln!("archive: keeping folder {id} ({e:#})");
        id.to_string()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::archive::store::read_item;
    use crate::archive::types::{ItemType, Segment, SESSION_KEY};

    const DATE: &str = "2026-09-24T10:00:00+02:00";

    fn meta(title: &str) -> ItemMeta {
        ItemMeta {
            item_type: ItemType::Note,
            title: title.into(),
            date: DATE.into(),
            source: "mic".into(),
            ..Default::default()
        }
    }

    fn segs(n: usize) -> SegmentsFile {
        SegmentsFile {
            segments: (0..n)
                .map(|i| Segment {
                    id: i as u32,
                    start_ms: i as u64 * 1000,
                    end_ms: i as u64 * 1000 + 900,
                    raw: format!("s{i}"),
                    text: format!("S{i}."),
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        }
    }

    fn transcript(archive: &Path, id: &str) -> String {
        std::fs::read_to_string(archive.join(id).join("transcript.md")).unwrap()
    }

    #[test]
    fn begin_marks_recording_and_checkpoint_saves_segments() {
        let tmp = tempfile::tempdir().unwrap();
        let archive = tmp.path();
        let id = begin_session(archive, &meta("Live")).unwrap();
        let item = read_item(archive, &id).unwrap();
        assert!(item.recording && !item.interrupted);
        assert!(item.segments.segments.is_empty());
        assert!(transcript(archive, &id).contains("status: recording"));

        // Segments only: transcript.md untouched until a render.
        checkpoint(archive, &id, &segs(2), false).unwrap();
        assert_eq!(read_item(archive, &id).unwrap().segments.segments.len(), 2);
        assert!(!transcript(archive, &id).contains("S1."));
        checkpoint(archive, &id, &segs(3), true).unwrap();
        let t = transcript(archive, &id);
        assert!(
            t.contains("S0. S1. S2.") && t.contains("status: recording"),
            "{t}"
        );
    }

    #[test]
    fn checkpoint_write_is_atomic_and_leaves_no_temp_files() {
        let tmp = tempfile::tempdir().unwrap();
        let archive = tmp.path();
        let id = begin_session(archive, &meta("Atomic")).unwrap();
        let meta_dir = archive.join(&id).join(".sussurro");
        let target = meta_dir.join("segments.json");
        for n in 1..=20 {
            checkpoint(archive, &id, &segs(n), n % 5 == 0).unwrap();
            // Whatever moment a crash hits, the file on disk parses and
            // holds a complete checkpoint.
            let on_disk: SegmentsFile =
                serde_json::from_str(&std::fs::read_to_string(&target).unwrap()).unwrap();
            assert_eq!(on_disk.segments.len(), n);
        }
        let leftovers: Vec<_> = std::fs::read_dir(&meta_dir)
            .unwrap()
            .chain(std::fs::read_dir(archive.join(&id)).unwrap())
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().contains(".tmp-"))
            .collect();
        assert!(leftovers.is_empty(), "{leftovers:?}");

        // A temp file left by a crash mid-write is ignored by readers and
        // replaced (not appended to) by the next checkpoint.
        std::fs::write(meta_dir.join(".segments.json.tmp-1-1"), "{ half").unwrap();
        assert_eq!(read_item(archive, &id).unwrap().segments.segments.len(), 20);
        checkpoint(archive, &id, &segs(21), false).unwrap();
        assert_eq!(read_item(archive, &id).unwrap().segments.segments.len(), 21);
    }

    #[test]
    fn finish_fills_meta_clears_marker_and_keeps_session_edits() {
        let tmp = tempfile::tempdir().unwrap();
        let archive = tmp.path();
        let id = begin_session(archive, &meta("")).unwrap();
        checkpoint(archive, &id, &segs(1), true).unwrap();
        // The user tags the live item from the UI; the UI's copy carries a
        // stale marker value, which must not override the file's.
        let mut m = read_item(archive, &id).unwrap().meta;
        m.tags = vec!["idea".into()];
        m.set_session_state(Some(SessionState::Interrupted));
        let during = crate::archive::update_meta(archive, &id, &m).unwrap();
        assert!(during.recording, "marker is app-owned");

        let fin = finish_session(archive, &id, &segs(2), &meta(""), |mut m| {
            m.duration = Some("00:00:02".into());
            m.title = "Final title".into();
            m
        })
        .unwrap();
        assert_eq!(fin.session_state(), None);
        let item = read_item(archive, &id).unwrap();
        assert!(!item.recording && !item.interrupted && !item.edited_externally);
        assert!(!item.meta.extra.contains_key(SESSION_KEY));
        assert_eq!(item.meta.tags, vec!["idea"]);
        assert_eq!(item.meta.duration.as_deref(), Some("00:00:02"));
        assert_eq!(item.segments.segments.len(), 2);
        assert!(item.body.contains("# Final title") && item.body.contains("S0. S1."));

        // A stale UI copy can't bring the marker back on a finished item.
        let mut stale = item.meta.clone();
        stale.set_session_state(Some(SessionState::Recording));
        assert!(
            !crate::archive::update_meta(archive, &id, &stale)
                .unwrap()
                .recording
        );
    }

    #[test]
    fn finish_on_an_externally_edited_transcript_keeps_the_body() {
        let tmp = tempfile::tempdir().unwrap();
        let archive = tmp.path();
        let id = begin_session(archive, &meta("Edited")).unwrap();
        checkpoint(archive, &id, &segs(1), true).unwrap();
        let path = archive.join(&id).join("transcript.md");
        let doc = transcript(archive, &id).replace("S0.", "Mine.");
        std::fs::write(&path, &doc).unwrap();
        // Later checkpoints respect the edit.
        checkpoint(archive, &id, &segs(3), true).unwrap();
        assert!(
            transcript(archive, &id).contains("Mine.") && !transcript(archive, &id).contains("S2.")
        );

        finish_session(archive, &id, &segs(3), &meta("Edited"), |mut m| {
            m.duration = Some("00:00:03".into());
            m
        })
        .unwrap();
        let item = read_item(archive, &id).unwrap();
        assert!(item.body.contains("Mine.") && item.edited_externally);
        assert_eq!(item.meta.duration.as_deref(), Some("00:00:03"));
        assert_eq!(item.meta.session_state(), None);
        assert_eq!(item.segments.segments.len(), 3);
    }

    #[test]
    fn finish_retries_after_a_concurrent_external_save_and_keeps_it() {
        let tmp = tempfile::tempdir().unwrap();
        let archive = tmp.path();
        let id = begin_session(archive, &meta("Race")).unwrap();
        // The engine's own periodic writes never trip the freshness check.
        for n in 1..=3 {
            checkpoint(archive, &id, &segs(n), true).unwrap();
        }
        let external = transcript(archive, &id).replace("S0.", "Obsidian.");
        let calls = std::cell::Cell::new(0);
        // Obsidian saves the file between the finish's read and its replace.
        let fin = finish_session_with(
            archive,
            &id,
            &segs(4),
            &meta("Race"),
            &|mut m| {
                m.duration = Some("00:00:04".into());
                m
            },
            &|p| {
                if calls.get() == 0 {
                    std::fs::write(p, &external).unwrap();
                }
                calls.set(calls.get() + 1);
            },
        )
        .unwrap();
        assert_eq!(calls.get(), 2, "re-read and retried once");
        assert_eq!(fin.session_state(), None);
        let item = read_item(archive, &id).unwrap();
        // The external save survives; the session still clears its marker.
        assert!(item.body.contains("Obsidian.") && item.edited_externally);
        assert!(!item.recording && !item.interrupted);
        assert_eq!(item.meta.duration.as_deref(), Some("00:00:04"));
        assert_eq!(item.segments.segments.len(), 4);
        let leftovers: Vec<_> = std::fs::read_dir(archive.join(&id))
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().contains(".tmp-"))
            .collect();
        assert!(leftovers.is_empty(), "{leftovers:?}");
    }

    #[test]
    fn mark_interrupted_only_touches_recording_items() {
        let tmp = tempfile::tempdir().unwrap();
        let archive = tmp.path();
        let id = begin_session(archive, &meta("Crash")).unwrap();
        checkpoint(archive, &id, &segs(4), false).unwrap(); // crash before a render
        assert!(mark_interrupted(archive, &id).unwrap());
        let item = read_item(archive, &id).unwrap();
        assert!(item.interrupted && !item.recording);
        assert_eq!(item.meta.extra[SESSION_KEY], "interrupted");
        assert_eq!(item.meta.duration.as_deref(), Some("00:00:03"));
        assert!(
            item.body.contains("S3."),
            "rendered from the saved segments"
        );
        assert!(!item.edited_externally);
        // Idempotent; complete items are left alone.
        assert!(!mark_interrupted(archive, &id).unwrap());
        let done = create_item(archive, &meta("Done"), &segs(1)).unwrap();
        let before = transcript(archive, &done);
        assert!(!mark_interrupted(archive, &done).unwrap());
        assert_eq!(transcript(archive, &done), before);
    }

    #[test]
    fn discard_removes_untouched_items_and_keeps_edited_ones() {
        let tmp = tempfile::tempdir().unwrap();
        let archive = tmp.path().join("arch");
        let id = begin_session(&archive, &meta("Gone")).unwrap();
        assert!(discard_session(&archive, &id).unwrap());
        assert!(!archive.join(&id).exists());
        assert!(!archive.join("2026").exists(), "empty month folders pruned");

        let id = begin_session(&archive, &meta("Kept")).unwrap();
        let path = archive.join(&id).join("transcript.md");
        std::fs::write(&path, transcript(&archive, &id) + "my notes\n").unwrap();
        assert!(!discard_session(&archive, &id).unwrap());
        assert!(read_item(&archive, &id).unwrap().interrupted);
    }

    #[test]
    fn ensure_writable_creates_the_folder_and_reports_failures() {
        let tmp = tempfile::tempdir().unwrap();
        let archive = tmp.path().join("a/b/Sussurro");
        ensure_writable(&archive).unwrap();
        assert!(archive.is_dir());
        assert_eq!(
            std::fs::read_dir(&archive).unwrap().count(),
            0,
            "probe removed"
        );
        // A regular file where the folder should be: a clear error.
        let blocked = tmp.path().join("file");
        std::fs::write(&blocked, "x").unwrap();
        let err = ensure_writable(&blocked.join("Sussurro")).unwrap_err();
        assert!(format!("{err:#}").contains("cannot write to the archive folder"));
    }

    #[test]
    fn retitle_renames_the_folder_once() {
        let tmp = tempfile::tempdir().unwrap();
        let archive = tmp.path();
        let id = begin_session(archive, &meta("")).unwrap();
        assert_eq!(id, "2026/09/2026-09-24-untitled");
        let taken = create_item(archive, &meta("Hello world"), &segs(0)).unwrap();
        let new_id = retitle_folder(archive, &id, "Hello world");
        assert_eq!(new_id, "2026/09/2026-09-24-hello-world-2");
        assert_ne!(new_id, taken);
        assert!(read_item(archive, &new_id).is_ok());
        assert!(!archive.join(&id).exists());
        // Already named after the title: unchanged.
        assert_eq!(retitle_folder(archive, &new_id, "Hello world"), new_id);
        // A missing item keeps its id rather than failing the session.
        assert_eq!(retitle_folder(archive, "2026/09/nope", "x"), "2026/09/nope");
    }
}
