//! Exports (plan §5, #133) and the `transcript.srt` sidecar (P7).
//!
//! - [`render_export`]: `.md` (the transcript as stored), `.txt` (plain
//!   lines, `[00:12:03] Anna: …`), `.srt` and `.vtt` (see [`super::subtitles`]).
//!   Subtitles are refused for notes (P10).
//! - [`create_subtitles`] / [`refresh_subtitles`]: `transcript.srt` next to
//!   the transcript, on request or — with the *Always* setting — every time
//!   the transcript is saved.
//!
//! The sidecar is app-owned with the archive's content-hash rule (plan
//! §4.4): its SHA-256 is recorded in `.sussurro/subtitles.json` when the app
//! writes it, and a `transcript.srt` that no longer matches (the user
//! edited it, or it was never written by the app) is never overwritten.

use super::frontmatter;
use super::render::{format_timestamp, note_paragraphs};
use super::store::{
    existing_item_dir, lock_items, read_item, read_segments, sha256_hex, transcript_path,
    write_atomic, META_DIR,
};
use super::subtitles::{build_cues, to_srt, to_vtt};
use super::types::{ItemMeta, ItemType, SegmentsFile, SessionState};
use super::Item;
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// The sidecar subtitles file inside an item folder.
pub const SUBTITLES_FILE: &str = "transcript.srt";
/// SHA-256 of the sidecar the app last wrote, inside the item's `.sussurro/`.
pub const SUBTITLES_STATE_FILE: &str = "subtitles.json";

/// Refusal for subtitles on a note (P10).
pub const NOTE_SUBTITLES_ERROR: &str =
    "notes have no subtitles — subtitles belong to meetings and transcriptions";

/// What an item can be exported as.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ExportFormat {
    Md,
    Txt,
    Srt,
    Vtt,
}

impl ExportFormat {
    pub fn extension(self) -> &'static str {
        match self {
            ExportFormat::Md => "md",
            ExportFormat::Txt => "txt",
            ExportFormat::Srt => "srt",
            ExportFormat::Vtt => "vtt",
        }
    }

    /// Case-insensitive, with or without the dot; `None` for anything else.
    pub fn parse(s: &str) -> Option<Self> {
        match s
            .trim()
            .trim_start_matches('.')
            .to_ascii_lowercase()
            .as_str()
        {
            "md" => Some(ExportFormat::Md),
            "txt" => Some(ExportFormat::Txt),
            "srt" => Some(ExportFormat::Srt),
            "vtt" => Some(ExportFormat::Vtt),
            _ => None,
        }
    }

    pub fn is_subtitles(self) -> bool {
        matches!(self, ExportFormat::Srt | ExportFormat::Vtt)
    }
}

/// `.txt`: meetings and transcriptions one line per segment
/// (`[HH:MM:SS] Label: text`, the label when the speaker is known), notes as
/// their paragraphs. An item whose markdown was edited outside the app
/// exports that markdown's body — the markdown wins.
fn plain_text(item: &Item) -> String {
    if item.edited_externally {
        let body = item.body.trim();
        return if body.is_empty() {
            String::new()
        } else {
            format!("{body}\n")
        };
    }
    let lines: Vec<String> = match item.meta.item_type {
        ItemType::Note => note_paragraphs(&item.segments),
        ItemType::Meeting | ItemType::Transcription => timed_lines(&item.segments),
    };
    let sep = if item.meta.item_type == ItemType::Note {
        "\n\n"
    } else {
        "\n"
    };
    if lines.is_empty() {
        String::new()
    } else {
        format!("{}\n", lines.join(sep))
    }
}

fn timed_lines(segs: &SegmentsFile) -> Vec<String> {
    segs.segments
        .iter()
        .filter(|s| !s.text.trim().is_empty())
        .map(|s| {
            let text = s.text.split_whitespace().collect::<Vec<_>>().join(" ");
            let label = s
                .speaker_id
                .as_deref()
                .and_then(|id| segs.speakers.iter().find(|sp| sp.id == id))
                .map(|sp| sp.label.split_whitespace().collect::<Vec<_>>().join(" "))
                .filter(|l| !l.is_empty());
            match label {
                Some(l) => format!("[{}] {l}: {text}", format_timestamp(s.start_ms)),
                None => format!("[{}] {text}", format_timestamp(s.start_ms)),
            }
        })
        .collect()
}

/// Subtitles for `segments`, refused for notes (P10) and when there is no
/// transcribed line to show.
fn subtitles_text(
    meta: &ItemMeta,
    segments: &SegmentsFile,
    format: ExportFormat,
) -> Result<String> {
    if meta.item_type == ItemType::Note {
        bail!("{NOTE_SUBTITLES_ERROR}");
    }
    let cues = build_cues(segments);
    if cues.is_empty() {
        bail!("no transcribed lines to make subtitles from");
    }
    Ok(match format {
        ExportFormat::Vtt => to_vtt(&cues),
        _ => to_srt(&cues),
    })
}

/// The export of `item` in `format`; `stored` is its `transcript.md` as on
/// disk (the `.md` export). Pure.
pub fn render_export(item: &Item, stored: &str, format: ExportFormat) -> Result<String> {
    match format {
        ExportFormat::Md => Ok(stored.to_string()),
        ExportFormat::Txt => Ok(plain_text(item)),
        ExportFormat::Srt | ExportFormat::Vtt => subtitles_text(&item.meta, &item.segments, format),
    }
}

/// The export of item `id` in `format` (for the Export menu and, from
/// #126, `GET /items/{id}/export`).
pub fn export_item(archive: &Path, id: &str, format: ExportFormat) -> Result<String> {
    let item = read_item(archive, id)?;
    let dir = existing_item_dir(archive, id)?;
    let stored = std::fs::read(transcript_path(&dir))
        .with_context(|| format!("reading {}", transcript_path(&dir).display()))?;
    render_export(&item, &String::from_utf8_lossy(&stored), format)
}

/// `path` with `format`'s extension: kept when it already has it (any
/// case), appended otherwise (a Linux save dialog may not add it).
pub fn with_extension(path: &Path, format: ExportFormat) -> std::path::PathBuf {
    let ext = format.extension();
    let has = path
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case(ext));
    if has {
        path.to_path_buf()
    } else {
        let mut s = path.as_os_str().to_owned();
        s.push(format!(".{ext}"));
        s.into()
    }
}

/// Export item `id` to the file the user picked. Returns the path written.
pub fn export_to_file(
    archive: &Path,
    id: &str,
    format: ExportFormat,
    path: &Path,
) -> Result<std::path::PathBuf> {
    let text = export_item(archive, id, format)?;
    let path = with_extension(path, format);
    write_atomic(&path, text.as_bytes())?;
    Ok(path)
}

// ---- transcript.srt --------------------------------------------------------

#[derive(Debug, Default, Serialize, Deserialize)]
struct SidecarState {
    /// SHA-256 (hex) of the `transcript.srt` bytes the app last wrote.
    #[serde(default)]
    srt_sha256: String,
}

fn state_path(dir: &Path) -> std::path::PathBuf {
    dir.join(META_DIR).join(SUBTITLES_STATE_FILE)
}

fn read_state(dir: &Path) -> SidecarState {
    std::fs::read_to_string(state_path(dir))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn write_state(dir: &Path, srt: &[u8]) -> Result<()> {
    std::fs::create_dir_all(dir.join(META_DIR))?;
    let state = SidecarState {
        srt_sha256: sha256_hex(srt),
    };
    write_atomic(
        &state_path(dir),
        serde_json::to_string_pretty(&state)?.as_bytes(),
    )
}

/// Where an item's `transcript.srt` stands, for the Export section.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct SubtitlesStatus {
    /// File name inside the item folder (`transcript.srt`).
    pub file: String,
    /// The item can have subtitles: a meeting or a transcription (P10).
    pub applicable: bool,
    /// `transcript.srt` exists.
    pub exists: bool,
    /// It exists and is not what the app last wrote (edited by the user,
    /// or put there by hand): the app will not overwrite it.
    pub edited_externally: bool,
}

/// The sidecar as found in `dir`: `None` when missing, otherwise whether it
/// is still what the app wrote. A folder or a symlink by that name is never
/// the app's (and is never written through).
fn sidecar_on_disk(dir: &Path) -> Result<Option<bool>> {
    let path = dir.join(SUBTITLES_FILE);
    match std::fs::symlink_metadata(&path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| format!("checking {}", path.display())),
        Ok(m) if !m.file_type().is_file() => Ok(Some(false)),
        Ok(_) => {
            let bytes =
                std::fs::read(&path).with_context(|| format!("reading {}", path.display()))?;
            let recorded = read_state(dir).srt_sha256;
            Ok(Some(!recorded.is_empty() && recorded == sha256_hex(&bytes)))
        }
    }
}

/// The item's metadata as the frontmatter has it now.
fn current_meta(dir: &Path) -> Result<ItemMeta> {
    let path = transcript_path(dir);
    let bytes = std::fs::read(&path).with_context(|| format!("reading {}", path.display()))?;
    Ok(frontmatter::parse(&String::from_utf8_lossy(&bytes))?.0)
}

/// [`SubtitlesStatus`] of item `id`.
pub fn subtitles_status(archive: &Path, id: &str) -> Result<SubtitlesStatus> {
    let dir = existing_item_dir(archive, id)?;
    let applicable = current_meta(&dir)
        .map(|m| m.item_type != ItemType::Note)
        .unwrap_or(false);
    let on_disk = sidecar_on_disk(&dir)?;
    Ok(SubtitlesStatus {
        file: SUBTITLES_FILE.to_string(),
        applicable,
        exists: on_disk.is_some(),
        edited_externally: on_disk == Some(false),
    })
}

/// What a sidecar write did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SidecarOutcome {
    /// `transcript.srt` was written (or already held exactly this).
    Written,
    /// A note (P10), an item still being recorded, an unreadable
    /// frontmatter, or nothing transcribed: no subtitles to write.
    NotApplicable,
    /// The user's `transcript.srt` is kept as is.
    KeptEdited,
}

/// Write `transcript.srt` in `dir` from its segments, following the rules
/// of the module docs. Callers hold [`lock_items`]. `explicit`: the user
/// asked, so every reason not to write is an error; otherwise (the
/// *Always* setting) those are quiet outcomes.
fn sync_sidecar(dir: &Path, id: &str, explicit: bool) -> Result<SidecarOutcome> {
    let meta = match current_meta(dir) {
        Ok(m) => m,
        Err(e) if explicit => return Err(e),
        Err(_) => return Ok(SidecarOutcome::NotApplicable),
    };
    if meta.session_state() == Some(SessionState::Recording) {
        if explicit {
            bail!("'{id}' is still being recorded — create subtitles when the session ends");
        }
        return Ok(SidecarOutcome::NotApplicable);
    }
    let segments = read_segments(dir)?;
    let srt = match subtitles_text(&meta, &segments, ExportFormat::Srt) {
        Ok(s) => s,
        Err(e) if explicit => return Err(e),
        Err(_) => return Ok(SidecarOutcome::NotApplicable),
    };
    if sidecar_on_disk(dir)? == Some(false) {
        if explicit {
            bail!(
                "{SUBTITLES_FILE} was edited outside Sussurro, so it is kept as is — rename or \
                 delete it to create a new one, or use Export → .srt to save a copy elsewhere"
            );
        }
        return Ok(SidecarOutcome::KeptEdited);
    }
    let path = dir.join(SUBTITLES_FILE);
    let same = std::fs::read(&path).is_ok_and(|now| now == srt.as_bytes());
    if !same {
        write_atomic(&path, srt.as_bytes())?;
    }
    write_state(dir, srt.as_bytes())?;
    Ok(SidecarOutcome::Written)
}

/// "Create .srt" (subtitles on request): write or update `transcript.srt`
/// for item `id`. Refused, with the reason, for notes (P10), items still
/// being recorded, items with nothing transcribed and a `transcript.srt`
/// edited outside the app. Returns the new status.
pub fn create_subtitles(archive: &Path, id: &str) -> Result<SubtitlesStatus> {
    {
        let _lock = lock_items();
        let dir = existing_item_dir(archive, id)?;
        sync_sidecar(&dir, id, true)?;
    }
    subtitles_status(archive, id)
}

/// The *Always* setting: bring `transcript.srt` up to date after the
/// transcript was saved. Notes, live items and a user-edited sidecar are
/// left alone (see [`SidecarOutcome`]).
pub fn refresh_subtitles(archive: &Path, id: &str) -> Result<SidecarOutcome> {
    let _lock = lock_items();
    let dir = existing_item_dir(archive, id)?;
    sync_sidecar(&dir, id, false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::archive::store::{create_item, edit_segment, SegmentEdit};
    use crate::archive::types::{DocSpeaker, Segment, Word};

    const DATE: &str = "2026-09-24T10:00:00+02:00";

    fn meta(t: ItemType, title: &str) -> ItemMeta {
        ItemMeta {
            item_type: t,
            title: title.into(),
            date: DATE.into(),
            source: "file:intervista.m4a".into(),
            ..Default::default()
        }
    }

    fn segs(texts: &[&str]) -> SegmentsFile {
        SegmentsFile {
            segments: texts
                .iter()
                .enumerate()
                .map(|(i, t)| {
                    let start = i as u64 * 3_000;
                    Segment {
                        id: i as u32,
                        start_ms: start,
                        end_ms: start + 2_000,
                        raw: t.to_string(),
                        text: t.to_string(),
                        words: vec![Word {
                            w: t.to_string(),
                            start_ms: start,
                            end_ms: start + 2_000,
                        }],
                        ..Default::default()
                    }
                })
                .collect(),
            ..Default::default()
        }
    }

    fn srt_path(archive: &Path, id: &str) -> std::path::PathBuf {
        archive.join(id).join(SUBTITLES_FILE)
    }

    #[test]
    fn formats_parse_and_name_their_extension() {
        assert_eq!(ExportFormat::parse(".SRT"), Some(ExportFormat::Srt));
        assert_eq!(ExportFormat::parse("vtt"), Some(ExportFormat::Vtt));
        assert_eq!(ExportFormat::parse("docx"), None);
        assert_eq!(ExportFormat::Txt.extension(), "txt");
        assert!(ExportFormat::Vtt.is_subtitles() && !ExportFormat::Md.is_subtitles());
        assert_eq!(
            serde_json::from_str::<ExportFormat>("\"md\"").unwrap(),
            ExportFormat::Md
        );
        let p = with_extension(Path::new("/x/Intervista"), ExportFormat::Srt);
        assert_eq!(p, Path::new("/x/Intervista.srt"));
        let p = with_extension(Path::new("/x/a.SRT"), ExportFormat::Srt);
        assert_eq!(p, Path::new("/x/a.SRT"));
        let p = with_extension(Path::new("/x/a.txt"), ExportFormat::Vtt);
        assert_eq!(p, Path::new("/x/a.txt.vtt"));
    }

    #[test]
    fn exports_every_format_of_a_transcription() {
        let tmp = tempfile::tempdir().unwrap();
        let archive = tmp.path();
        let mut s = segs(&["Benvenuti.", "Grazie."]);
        s.speakers = vec![DocSpeaker {
            id: "meet:anna".into(),
            label: "Anna".into(),
            ..Default::default()
        }];
        s.segments[0].speaker_id = Some("meet:anna".into());
        let id = create_item(archive, &meta(ItemType::Transcription, "Intervista"), &s).unwrap();

        let md = export_item(archive, &id, ExportFormat::Md).unwrap();
        assert_eq!(
            md,
            std::fs::read_to_string(archive.join(&id).join("transcript.md")).unwrap()
        );
        assert_eq!(
            export_item(archive, &id, ExportFormat::Txt).unwrap(),
            "[00:00:00] Anna: Benvenuti.\n[00:00:03] Grazie.\n"
        );
        assert_eq!(
            export_item(archive, &id, ExportFormat::Srt).unwrap(),
            "1\n00:00:00,000 --> 00:00:02,000\nAnna: Benvenuti.\n\n\
             2\n00:00:03,000 --> 00:00:05,000\nGrazie.\n\n"
        );
        let vtt = export_item(archive, &id, ExportFormat::Vtt).unwrap();
        assert!(vtt.starts_with("WEBVTT\n\n00:00:00.000 --> 00:00:02.000\nAnna: Benvenuti."));

        let out = tmp.path().join("out");
        std::fs::create_dir_all(&out).unwrap();
        let written =
            export_to_file(archive, &id, ExportFormat::Vtt, &out.join("intervista")).unwrap();
        assert_eq!(written, out.join("intervista.vtt"));
        assert_eq!(std::fs::read_to_string(written).unwrap(), vtt);
    }

    #[test]
    fn notes_export_text_but_never_subtitles() {
        let tmp = tempfile::tempdir().unwrap();
        let archive = tmp.path();
        let id = create_item(
            archive,
            &meta(ItemType::Note, "Idea"),
            &segs(&["Uno.", "Due."]),
        )
        .unwrap();
        // 1 s pause between segments: one paragraph.
        assert_eq!(
            export_item(archive, &id, ExportFormat::Txt).unwrap(),
            "Uno. Due.\n"
        );
        for f in [ExportFormat::Srt, ExportFormat::Vtt] {
            let err = export_item(archive, &id, f).unwrap_err();
            assert!(
                format!("{err:#}").contains("notes have no subtitles"),
                "{err:#}"
            );
        }
        let err = create_subtitles(archive, &id).unwrap_err();
        assert!(
            format!("{err:#}").contains("notes have no subtitles"),
            "{err:#}"
        );
        assert_eq!(
            refresh_subtitles(archive, &id).unwrap(),
            SidecarOutcome::NotApplicable
        );
        assert!(!srt_path(archive, &id).exists());
        let status = subtitles_status(archive, &id).unwrap();
        assert!(!status.applicable && !status.exists);
    }

    #[test]
    fn txt_of_a_transcript_edited_outside_is_its_markdown_body() {
        let tmp = tempfile::tempdir().unwrap();
        let archive = tmp.path();
        let id = create_item(
            archive,
            &meta(ItemType::Transcription, "T"),
            &segs(&["Uno."]),
        )
        .unwrap();
        let path = archive.join(&id).join("transcript.md");
        let doc = std::fs::read_to_string(&path)
            .unwrap()
            .replace("Uno.", "Uno, a mano.");
        std::fs::write(&path, doc).unwrap();
        let txt = export_item(archive, &id, ExportFormat::Txt).unwrap();
        assert!(txt.contains("Uno, a mano."), "{txt}");
    }

    #[test]
    fn subtitles_need_transcribed_lines() {
        let tmp = tempfile::tempdir().unwrap();
        let archive = tmp.path();
        let mut s = segs(&[""]);
        s.segments[0].stt_error = Some("boom".into());
        let id = create_item(archive, &meta(ItemType::Meeting, "Vuota"), &s).unwrap();
        assert!(export_item(archive, &id, ExportFormat::Srt).is_err());
        let err = create_subtitles(archive, &id).unwrap_err();
        assert!(
            format!("{err:#}").contains("no transcribed lines"),
            "{err:#}"
        );
        assert_eq!(
            refresh_subtitles(archive, &id).unwrap(),
            SidecarOutcome::NotApplicable
        );
    }

    #[test]
    fn create_writes_the_sidecar_and_tracks_its_hash() {
        let tmp = tempfile::tempdir().unwrap();
        let archive = tmp.path();
        let id = create_item(
            archive,
            &meta(ItemType::Transcription, "T"),
            &segs(&["Uno.", "Due."]),
        )
        .unwrap();
        let before = subtitles_status(archive, &id).unwrap();
        assert!(before.applicable && !before.exists && !before.edited_externally);

        let status = create_subtitles(archive, &id).unwrap();
        assert!(status.exists && !status.edited_externally);
        let srt = std::fs::read_to_string(srt_path(archive, &id)).unwrap();
        assert_eq!(srt, export_item(archive, &id, ExportFormat::Srt).unwrap());
        assert!(archive.join(&id).join(".sussurro/subtitles.json").is_file());
        // Again: fine, same content.
        create_subtitles(archive, &id).unwrap();
    }

    #[test]
    fn a_line_edit_with_always_updates_the_sidecar() {
        let tmp = tempfile::tempdir().unwrap();
        let archive = tmp.path();
        let id = create_item(
            archive,
            &meta(ItemType::Transcription, "T"),
            &segs(&["Uno.", "Due."]),
        )
        .unwrap();
        assert_eq!(
            refresh_subtitles(archive, &id).unwrap(),
            SidecarOutcome::Written
        );
        edit_segment(archive, &id, 1, SegmentEdit::Text("Due, corretto.".into())).unwrap();
        assert_eq!(
            refresh_subtitles(archive, &id).unwrap(),
            SidecarOutcome::Written
        );
        let srt = std::fs::read_to_string(srt_path(archive, &id)).unwrap();
        assert!(srt.contains("Due, corretto."), "{srt}");
    }

    #[test]
    fn a_sidecar_edited_by_the_user_is_never_overwritten() {
        let tmp = tempfile::tempdir().unwrap();
        let archive = tmp.path();
        let id = create_item(
            archive,
            &meta(ItemType::Transcription, "T"),
            &segs(&["Uno."]),
        )
        .unwrap();
        create_subtitles(archive, &id).unwrap();
        let path = srt_path(archive, &id);
        let mine = "1\n00:00:00,000 --> 00:00:02,000\nUno, sistemato a mano.\n\n";
        std::fs::write(&path, mine).unwrap();

        assert!(subtitles_status(archive, &id).unwrap().edited_externally);
        assert_eq!(
            refresh_subtitles(archive, &id).unwrap(),
            SidecarOutcome::KeptEdited
        );
        let err = create_subtitles(archive, &id).unwrap_err();
        assert!(
            format!("{err:#}").contains("edited outside Sussurro"),
            "{err:#}"
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), mine);

        // Deleting it hands the name back to the app.
        std::fs::remove_file(&path).unwrap();
        assert_eq!(
            refresh_subtitles(archive, &id).unwrap(),
            SidecarOutcome::Written
        );
        assert!(!subtitles_status(archive, &id).unwrap().edited_externally);
    }

    #[test]
    fn a_sidecar_the_app_never_wrote_is_kept() {
        let tmp = tempfile::tempdir().unwrap();
        let archive = tmp.path();
        let id = create_item(archive, &meta(ItemType::Meeting, "M"), &segs(&["Uno."])).unwrap();
        let path = srt_path(archive, &id);
        std::fs::write(&path, "fatto da me").unwrap();
        assert_eq!(
            refresh_subtitles(archive, &id).unwrap(),
            SidecarOutcome::KeptEdited
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "fatto da me");
        // A folder in its place is not ours either.
        std::fs::remove_file(&path).unwrap();
        std::fs::create_dir(&path).unwrap();
        assert_eq!(
            refresh_subtitles(archive, &id).unwrap(),
            SidecarOutcome::KeptEdited
        );
    }

    #[test]
    fn live_items_get_subtitles_when_the_session_ends() {
        let tmp = tempfile::tempdir().unwrap();
        let archive = tmp.path();
        let id =
            crate::archive::live::begin_session(archive, &meta(ItemType::Meeting, "Live")).unwrap();
        crate::archive::live::checkpoint(archive, &id, &segs(&["Uno."]), true).unwrap();
        assert_eq!(
            refresh_subtitles(archive, &id).unwrap(),
            SidecarOutcome::NotApplicable
        );
        let err = create_subtitles(archive, &id).unwrap_err();
        assert!(
            format!("{err:#}").contains("still being recorded"),
            "{err:#}"
        );
        assert!(!srt_path(archive, &id).exists());
    }
}
