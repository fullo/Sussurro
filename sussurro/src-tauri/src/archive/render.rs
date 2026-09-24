//! `transcript.md` rendering from metadata + segments (pure).

use super::frontmatter;
use super::types::{ItemMeta, ItemType, SegmentsFile};
use anyhow::Result;

/// A pause at least this long between two note segments starts a new
/// paragraph; shorter pauses join the segments with a space.
pub const PARAGRAPH_GAP_MS: u64 = 2_000;

/// `HH:MM:SS` for a millisecond offset (hours keep growing past 99).
pub fn format_timestamp(ms: u64) -> String {
    let s = ms / 1000;
    format!("{:02}:{:02}:{:02}", s / 3600, (s / 60) % 60, s % 60)
}

/// One line for headings: newlines would break the `# title` line.
fn single_line(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The body below the frontmatter: `# <title>`, a blank line, the text.
pub fn render_body(meta: &ItemMeta, segs: &SegmentsFile) -> String {
    let title = single_line(&meta.title);
    let mut parts = Vec::new();
    if !title.is_empty() {
        parts.push(format!("# {title}"));
    }
    parts.extend(match meta.item_type {
        ItemType::Note => note_paragraphs(segs),
        ItemType::Meeting | ItemType::Transcription => timestamped_lines(segs),
    });
    if parts.is_empty() {
        return String::new();
    }
    format!("{}\n", parts.join("\n\n"))
}

/// Full `transcript.md`: frontmatter, then a blank line, then the body.
pub fn render_transcript(meta: &ItemMeta, segs: &SegmentsFile) -> Result<String> {
    Ok(format!(
        "{}\n{}",
        frontmatter::render(meta)?,
        render_body(meta, segs)
    ))
}

/// Notes: cleaned text joined into paragraphs, broken on long pauses.
fn note_paragraphs(segs: &SegmentsFile) -> Vec<String> {
    let mut paras: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut last_end: Option<u64> = None;
    for seg in &segs.segments {
        let text = seg.text.trim();
        if text.is_empty() {
            continue;
        }
        let gap = last_end.map(|e| seg.start_ms.saturating_sub(e));
        if gap.is_some_and(|g| g >= PARAGRAPH_GAP_MS) && !current.is_empty() {
            paras.push(std::mem::take(&mut current));
        }
        if !current.is_empty() {
            current.push(' ');
        }
        current.push_str(text);
        last_end = Some(seg.end_ms);
    }
    if !current.is_empty() {
        paras.push(current);
    }
    paras
}

/// Meetings and transcriptions: one block per segment. With speakers,
/// `**[HH:MM:SS] Label:** text`; without, `[HH:MM:SS] text`.
fn timestamped_lines(segs: &SegmentsFile) -> Vec<String> {
    let with_speakers = !segs.speakers.is_empty();
    segs.segments
        .iter()
        .filter(|s| !s.text.trim().is_empty())
        .map(|seg| {
            let ts = format_timestamp(seg.start_ms);
            let text = seg.text.trim();
            if !with_speakers {
                return format!("[{ts}] {text}");
            }
            let label = seg
                .speaker_id
                .as_deref()
                .and_then(|id| segs.speakers.iter().find(|sp| sp.id == id))
                .map(|sp| single_line(&sp.label))
                .filter(|l| !l.is_empty());
            match label {
                Some(l) => format!("**[{ts}] {l}:** {text}"),
                None => format!("**[{ts}]** {text}"),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::archive::types::{DocSpeaker, Segment};

    fn seg(id: u32, start: u64, end: u64, speaker: Option<&str>, text: &str) -> Segment {
        Segment {
            id,
            start_ms: start,
            end_ms: end,
            speaker_id: speaker.map(str::to_string),
            raw: text.to_string(),
            text: text.to_string(),
            ..Default::default()
        }
    }

    fn meta(t: ItemType, title: &str) -> ItemMeta {
        ItemMeta {
            item_type: t,
            title: title.into(),
            ..Default::default()
        }
    }

    #[test]
    fn timestamps() {
        assert_eq!(format_timestamp(0), "00:00:00");
        assert_eq!(format_timestamp(61_999), "00:01:01");
        assert_eq!(format_timestamp(3_723_000), "01:02:03");
        assert_eq!(format_timestamp(100 * 3_600_000), "100:00:00");
    }

    #[test]
    fn note_joins_segments_and_breaks_on_pauses() {
        let segs = SegmentsFile {
            segments: vec![
                seg(1, 0, 1000, None, "Prima frase."),
                seg(2, 1500, 3000, None, " Seconda. "),
                seg(3, 3200, 3300, None, "   "),
                seg(4, 6000, 7000, None, "Nuovo paragrafo."),
            ],
            ..Default::default()
        };
        let body = render_body(&meta(ItemType::Note, "Idea\nlunga"), &segs);
        assert_eq!(
            body,
            "# Idea lunga\n\nPrima frase. Seconda.\n\nNuovo paragrafo.\n"
        );
    }

    #[test]
    fn meeting_with_speakers_labels_every_line() {
        let segs = SegmentsFile {
            speakers: vec![
                DocSpeaker {
                    id: "you".into(),
                    label: "You".into(),
                    ..Default::default()
                },
                DocSpeaker {
                    id: "voice:2".into(),
                    label: "Voice 2".into(),
                    ..Default::default()
                },
            ],
            segments: vec![
                seg(1, 0, 1000, Some("you"), "Ciao."),
                seg(2, 65_000, 66_000, Some("voice:2"), "Buongiorno."),
                seg(3, 3_700_000, 3_701_000, None, "Chi parla?"),
                seg(4, 3_702_000, 3_703_000, Some("ghost"), "Boh."),
            ],
            ..Default::default()
        };
        let body = render_body(&meta(ItemType::Meeting, "Sync"), &segs);
        assert_eq!(
            body,
            "# Sync\n\n**[00:00:00] You:** Ciao.\n\n**[00:01:05] Voice 2:** Buongiorno.\n\n\
             **[01:01:40]** Chi parla?\n\n**[01:01:42]** Boh.\n"
        );
    }

    #[test]
    fn transcription_without_speakers_uses_plain_timestamps() {
        let segs = SegmentsFile {
            segments: vec![
                seg(1, 0, 1000, None, "Uno."),
                seg(2, 2000, 3000, None, "Due."),
            ],
            ..Default::default()
        };
        let body = render_body(&meta(ItemType::Transcription, "Podcast"), &segs);
        assert_eq!(body, "# Podcast\n\n[00:00:00] Uno.\n\n[00:00:02] Due.\n");
    }

    #[test]
    fn empty_item_renders_title_only() {
        let body = render_body(&meta(ItemType::Note, "Vuota"), &SegmentsFile::default());
        assert_eq!(body, "# Vuota\n");
        assert_eq!(
            render_body(&meta(ItemType::Note, " "), &SegmentsFile::default()),
            ""
        );
        let doc =
            render_transcript(&meta(ItemType::Note, "Vuota"), &SegmentsFile::default()).unwrap();
        assert!(doc.starts_with("---\ntype: note\n"));
        assert!(doc.ends_with("---\n\n# Vuota\n"));
    }
}
