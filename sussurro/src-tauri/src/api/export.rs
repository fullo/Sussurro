//! `GET /items/{id}/export?format=…` (#126, plan §5 "Exports"): "Copy as
//! text" and downloads from the extension's side panel.
//!
//! - `md`: `transcript.md` exactly as stored (frontmatter included).
//! - `txt`: plain lines `[HH:MM:SS] Label: text` for meetings and
//!   transcriptions (the label when the segment has a speaker), paragraphs
//!   for notes; an item edited outside Sussurro exports its markdown body.
//! - `srt`, `vtt`: recognised, answered "not available yet" until the
//!   subtitle writers land (#133) — they plug in at [`render`].

use crate::archive::{self, ItemType};
use anyhow::Result;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportFormat {
    Md,
    Txt,
    Srt,
    Vtt,
}

impl ExportFormat {
    /// `md` | `txt` | `srt` | `vtt` (case-insensitive); missing = `md`.
    pub fn parse(s: Option<&str>) -> Option<Self> {
        match s.map(|s| s.trim().to_ascii_lowercase()).as_deref() {
            None | Some("") | Some("md") => Some(Self::Md),
            Some("txt") => Some(Self::Txt),
            Some("srt") => Some(Self::Srt),
            Some("vtt") => Some(Self::Vtt),
            _ => None,
        }
    }

    pub fn extension(self) -> &'static str {
        match self {
            Self::Md => "md",
            Self::Txt => "txt",
            Self::Srt => "srt",
            Self::Vtt => "vtt",
        }
    }

    pub fn content_type(self) -> &'static str {
        match self {
            Self::Md => "text/markdown; charset=utf-8",
            Self::Txt => "text/plain; charset=utf-8",
            Self::Srt => "application/x-subrip; charset=utf-8",
            Self::Vtt => "text/vtt; charset=utf-8",
        }
    }
}

/// An export ready to send.
#[derive(Debug, Clone, PartialEq)]
pub struct Export {
    pub body: String,
    pub content_type: &'static str,
    /// Suggested download name: the item's folder name plus the extension.
    pub filename: String,
}

#[derive(Debug)]
pub enum ExportError {
    /// The item does not exist (or the id is invalid): 404.
    NotFound(anyhow::Error),
    /// The format is known but has no writer yet (#133): 501.
    NotAvailable(ExportFormat),
    Failed(anyhow::Error),
}

/// Plain text of an item. Pure.
pub fn plain_text(item: &archive::Item) -> String {
    if item.edited_externally {
        return item.body.trim().to_string() + "\n";
    }
    let segs = &item.segments;
    let mut out = String::new();
    match item.meta.item_type {
        ItemType::Note => out = crate::engine::transcript_text(&segs.segments),
        ItemType::Meeting | ItemType::Transcription => {
            for s in segs.segments.iter().filter(|s| !s.text.trim().is_empty()) {
                let ts = archive::render::format_timestamp(s.start_ms);
                let label = s
                    .speaker_id
                    .as_deref()
                    .and_then(|id| segs.speakers.iter().find(|sp| sp.id == id))
                    .map(|sp| sp.label.split_whitespace().collect::<Vec<_>>().join(" "))
                    .filter(|l| !l.is_empty());
                let text = s.text.split_whitespace().collect::<Vec<_>>().join(" ");
                match label {
                    Some(l) => out.push_str(&format!("[{ts}] {l}: {text}\n")),
                    None => out.push_str(&format!("[{ts}] {text}\n")),
                }
            }
        }
    }
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
    out
}

/// The export of item `id` in `format`.
pub fn render(archive_dir: &Path, id: &str, format: ExportFormat) -> Result<Export, ExportError> {
    let item = archive::read_item(archive_dir, id).map_err(ExportError::NotFound)?;
    let body = match format {
        ExportFormat::Md => {
            let dir = archive::paths::item_dir(archive_dir, id).map_err(ExportError::NotFound)?;
            let bytes = std::fs::read(dir.join(archive::store::TRANSCRIPT_FILE))
                .map_err(|e| ExportError::Failed(e.into()))?;
            String::from_utf8_lossy(&bytes).into_owned()
        }
        ExportFormat::Txt => plain_text(&item),
        f @ (ExportFormat::Srt | ExportFormat::Vtt) => return Err(ExportError::NotAvailable(f)),
    };
    let folder = id.rsplit('/').next().unwrap_or("transcript");
    Ok(Export {
        body,
        content_type: format.content_type(),
        filename: format!("{folder}.{}", format.extension()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::archive::{create_item, Channel, DocSpeaker, ItemMeta, Segment, SegmentsFile};

    fn seg(id: u32, start_ms: u64, text: &str, speaker: Option<&str>) -> Segment {
        Segment {
            id,
            channel: Channel::Remote,
            start_ms,
            end_ms: start_ms + 1_000,
            raw: text.into(),
            text: text.into(),
            speaker_id: speaker.map(str::to_string),
            ..Default::default()
        }
    }

    fn meeting(archive: &Path) -> String {
        let meta = ItemMeta {
            item_type: ItemType::Meeting,
            title: "Weekly sync".into(),
            date: "2026-09-24T10:00:00+02:00".into(),
            source: "browser:meet.google.com".into(),
            ..Default::default()
        };
        let segs = SegmentsFile {
            speakers: vec![DocSpeaker {
                id: "meet:Anna".into(),
                label: "Anna".into(),
                ..Default::default()
            }],
            segments: vec![
                seg(0, 723_000, "Ciao a tutti.", Some("meet:Anna")),
                seg(1, 725_000, " ", None),
                seg(2, 726_500, "Buongiorno\nAnna.", None),
            ],
            ..Default::default()
        };
        create_item(archive, &meta, &segs).unwrap()
    }

    #[test]
    fn formats_parse() {
        assert_eq!(ExportFormat::parse(None), Some(ExportFormat::Md));
        assert_eq!(ExportFormat::parse(Some("TXT")), Some(ExportFormat::Txt));
        assert_eq!(ExportFormat::parse(Some("srt")), Some(ExportFormat::Srt));
        assert_eq!(ExportFormat::parse(Some("vtt")), Some(ExportFormat::Vtt));
        assert_eq!(ExportFormat::parse(Some("docx")), None);
    }

    #[test]
    fn exports_md_as_stored_and_txt_as_lines() {
        let tmp = tempfile::tempdir().unwrap();
        let archive = tmp.path().join("Sussurro");
        let id = meeting(&archive);
        let md = render(&archive, &id, ExportFormat::Md).unwrap();
        let stored = std::fs::read_to_string(archive.join(&id).join("transcript.md")).unwrap();
        assert_eq!(md.body, stored);
        assert!(md.filename.ends_with("weekly-sync.md"), "{}", md.filename);
        let txt = render(&archive, &id, ExportFormat::Txt).unwrap();
        assert_eq!(
            txt.body,
            "[00:12:03] Anna: Ciao a tutti.\n[00:12:06] Buongiorno Anna.\n"
        );
        assert!(txt.content_type.starts_with("text/plain"));
        assert!(matches!(
            render(&archive, &id, ExportFormat::Srt),
            Err(ExportError::NotAvailable(ExportFormat::Srt))
        ));
        assert!(matches!(
            render(&archive, "2026/09/nope", ExportFormat::Md),
            Err(ExportError::NotFound(_))
        ));
        assert!(matches!(
            render(&archive, "../etc", ExportFormat::Txt),
            Err(ExportError::NotFound(_))
        ));
    }
}
