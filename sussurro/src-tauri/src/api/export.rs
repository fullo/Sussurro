//! `GET /items/{id}/export?format=md|txt|srt|vtt` (#126, the HTTP side of
//! #133): "Copy as text" and downloads from the extension's side panel.
//! The content comes from [`archive::export::export_item`], the same code as
//! the app's Export menu — `md` as stored, `txt` as timed lines, `srt`/`vtt`
//! from the subtitle writers, refused for notes (P10).

use crate::archive::{self, export::ExportFormat};
use std::path::Path;

/// `format` query value → format; missing or empty = `md`.
pub fn parse_format(s: Option<&str>) -> Option<ExportFormat> {
    match s.map(str::trim) {
        None | Some("") => Some(ExportFormat::Md),
        Some(f) => ExportFormat::parse(f),
    }
}

pub fn content_type(format: ExportFormat) -> &'static str {
    match format {
        ExportFormat::Md => "text/markdown; charset=utf-8",
        ExportFormat::Txt => "text/plain; charset=utf-8",
        ExportFormat::Srt => "application/x-subrip; charset=utf-8",
        ExportFormat::Vtt => "text/vtt; charset=utf-8",
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
    /// No such item (or an invalid id): 404.
    NotFound(anyhow::Error),
    /// The item can't be exported that way (subtitles of a note, nothing
    /// transcribed yet) or could not be read: 422.
    Refused(anyhow::Error),
    /// Not recorded by the extension (#215): 403 with
    /// `code: "not_extension_item"`, which the extension explains.
    NotExtensionItem,
}

/// The `code` of a refusal for an item the extension didn't record.
pub const NOT_EXTENSION_ITEM: &str = "not_extension_item";

/// The extension reaches only the items it recorded (`source:
/// browser:<host>`, #215): a leaked pairing code must not expose the notes,
/// dictations and transcriptions of the rest of the archive.
pub fn extension_may_access(source: &str) -> bool {
    source.starts_with("browser:")
}

/// Item `id` exists and the extension may open or export it.
pub fn check_access(archive_dir: &Path, id: &str) -> Result<(), ExportError> {
    let item = archive::read_item(archive_dir, id).map_err(ExportError::NotFound)?;
    if extension_may_access(&item.meta.source) {
        Ok(())
    } else {
        Err(ExportError::NotExtensionItem)
    }
}

/// The export of item `id` in `format`, for the extension ([`check_access`]).
pub fn render(archive_dir: &Path, id: &str, format: ExportFormat) -> Result<Export, ExportError> {
    check_access(archive_dir, id)?;
    let body =
        archive::export::export_item(archive_dir, id, format).map_err(ExportError::Refused)?;
    let folder = id.rsplit('/').next().unwrap_or("transcript");
    Ok(Export {
        body,
        content_type: content_type(format),
        filename: format!("{folder}.{}", format.extension()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::archive::{create_item, Channel, ItemMeta, ItemType, Segment, SegmentsFile};

    fn item(archive: &Path, item_type: ItemType) -> String {
        item_from(archive, item_type, "browser:meet.google.com")
    }

    fn item_from(archive: &Path, item_type: ItemType, source: &str) -> String {
        let meta = ItemMeta {
            item_type,
            title: "Weekly sync".into(),
            date: "2026-09-24T10:00:00+02:00".into(),
            source: source.into(),
            ..Default::default()
        };
        let segs = SegmentsFile {
            segments: vec![Segment {
                id: 0,
                channel: Channel::Remote,
                start_ms: 723_000,
                end_ms: 725_000,
                raw: "ciao".into(),
                text: "Ciao a tutti.".into(),
                ..Default::default()
            }],
            ..Default::default()
        };
        create_item(archive, &meta, &segs).unwrap()
    }

    #[test]
    fn formats_parse_with_md_as_default() {
        assert_eq!(parse_format(None), Some(ExportFormat::Md));
        assert_eq!(parse_format(Some("")), Some(ExportFormat::Md));
        assert_eq!(parse_format(Some("TXT")), Some(ExportFormat::Txt));
        assert_eq!(parse_format(Some("vtt")), Some(ExportFormat::Vtt));
        assert_eq!(parse_format(Some("docx")), None);
    }

    #[test]
    fn exports_every_format_and_refuses_subtitles_for_notes() {
        let tmp = tempfile::tempdir().unwrap();
        let archive = tmp.path().join("Sussurro");
        let id = item(&archive, ItemType::Meeting);
        let md = render(&archive, &id, ExportFormat::Md).unwrap();
        let stored = std::fs::read_to_string(archive.join(&id).join("transcript.md")).unwrap();
        assert_eq!(md.body, stored);
        assert!(md.filename.ends_with("weekly-sync.md"), "{}", md.filename);
        let txt = render(&archive, &id, ExportFormat::Txt).unwrap();
        assert_eq!(txt.body, "[00:12:03] Ciao a tutti.\n");
        assert!(txt.content_type.starts_with("text/plain"));
        let srt = render(&archive, &id, ExportFormat::Srt).unwrap();
        assert!(srt.body.contains("00:12:03,000 --> "), "{}", srt.body);
        let vtt = render(&archive, &id, ExportFormat::Vtt).unwrap();
        assert!(vtt.body.starts_with("WEBVTT"));
        assert_eq!(vtt.content_type, "text/vtt; charset=utf-8");

        let note = item(&archive, ItemType::Note);
        assert!(matches!(
            render(&archive, &note, ExportFormat::Srt),
            Err(ExportError::Refused(_))
        ));
        assert!(render(&archive, &note, ExportFormat::Txt).is_ok());
        assert!(matches!(
            render(&archive, "2026/09/nope", ExportFormat::Md),
            Err(ExportError::NotFound(_))
        ));
        assert!(matches!(
            render(&archive, "../etc", ExportFormat::Txt),
            Err(ExportError::NotFound(_))
        ));
    }

    /// #215: only what the extension recorded; the rest of the archive
    /// (notes, dictations, files, links, system audio) is refused.
    #[test]
    fn only_items_the_extension_recorded_are_reachable() {
        assert!(extension_may_access("browser:meet.google.com"));
        assert!(extension_may_access("browser:teams.microsoft.com"));
        for s in [
            "mic",
            "file:/Users/x/a.mp3",
            "url:https://x.example",
            "system",
            "",
            "Browser:x",
            " browser:x",
        ] {
            assert!(!extension_may_access(s), "{s}");
        }
        let tmp = tempfile::tempdir().unwrap();
        let archive = tmp.path().join("Sussurro");
        let meeting = item(&archive, ItemType::Meeting);
        assert!(check_access(&archive, &meeting).is_ok());
        for source in ["mic", "file:/tmp/a.wav", "system"] {
            let other = item_from(&archive, ItemType::Note, source);
            assert!(matches!(
                check_access(&archive, &other),
                Err(ExportError::NotExtensionItem)
            ));
            assert!(matches!(
                render(&archive, &other, ExportFormat::Md),
                Err(ExportError::NotExtensionItem)
            ));
        }
        assert!(matches!(
            check_access(&archive, "2026/09/nope"),
            Err(ExportError::NotFound(_))
        ));
    }
}
