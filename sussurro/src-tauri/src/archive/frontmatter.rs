//! YAML frontmatter of `transcript.md`: `---\n<yaml>\n---\n` at the top of the
//! file. Parsing and emitting go through `serde-saphyr` (pure Rust, YAML 1.2,
//! actively maintained; `serde_yaml` is deprecated).

use super::types::ItemMeta;
use anyhow::{Context, Result};

/// Split a markdown document into `(frontmatter yaml, body)`. `None` when the
/// file does not start with a `---` line or the block is never closed. A
/// leading BOM and CRLF line endings are tolerated; the closing fence may be
/// `---` or `...`.
pub fn split(doc: &str) -> Option<(&str, &str)> {
    let doc = doc.strip_prefix('\u{feff}').unwrap_or(doc);
    let first_end = doc.find('\n')?;
    if doc[..first_end].trim_end_matches('\r') != "---" {
        return None;
    }
    let yaml_start = first_end + 1;
    let mut pos = yaml_start;
    loop {
        let line_end = doc[pos..].find('\n').map(|i| pos + i);
        let line = &doc[pos..line_end.unwrap_or(doc.len())];
        let trimmed = line.trim_end_matches('\r').trim_end();
        if trimmed == "---" || trimmed == "..." {
            let yaml = &doc[yaml_start..pos];
            let body = match line_end {
                Some(e) => &doc[e + 1..],
                None => "",
            };
            return Some((yaml, body));
        }
        pos = line_end? + 1;
    }
}

/// Parse the frontmatter of `doc` into metadata plus the body that follows it.
/// A document without frontmatter yields default metadata and the whole text
/// as body; malformed YAML is an error.
pub fn parse(doc: &str) -> Result<(ItemMeta, String)> {
    match split(doc) {
        Some((yaml, body)) => {
            let meta = if yaml.trim().is_empty() {
                ItemMeta::default()
            } else {
                serde_saphyr::from_str::<ItemMeta>(yaml).context("invalid YAML frontmatter")?
            };
            Ok((meta, body.to_string()))
        }
        None => Ok((ItemMeta::default(), doc.to_string())),
    }
}

/// The `---\n<yaml>---\n` block for `meta`.
pub fn render(meta: &ItemMeta) -> Result<String> {
    let mut yaml = serde_saphyr::to_string(meta).context("serializing frontmatter")?;
    if !yaml.ends_with('\n') {
        yaml.push('\n');
    }
    Ok(format!("---\n{yaml}---\n"))
}

/// Replace the frontmatter of `doc` with `meta`, keeping the body verbatim.
pub fn replace(doc: &str, meta: &ItemMeta) -> Result<String> {
    let body = match split(doc) {
        Some((_, body)) => body,
        None => doc,
    };
    Ok(format!("{}{body}", render(meta)?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::archive::types::{ItemType, Participant};

    fn sample() -> ItemMeta {
        ItemMeta {
            item_type: ItemType::Meeting,
            title: "Weekly sync — release 0.7: \"final\"".into(),
            date: "2026-09-24T10:00:00+02:00".into(),
            duration: Some("00:42:10".into()),
            source: "browser:meet.google.com".into(),
            language: "no".into(),
            engine: "whisper-small".into(),
            tags: vec!["release".into(), "2026".into(), "yes".into()],
            categories: vec!["team".into()],
            participants: vec![
                Participant {
                    name: "Anna Rossi".into(),
                    email: Some("anna@example.com".into()),
                },
                Participant {
                    name: "Voice 2".into(),
                    email: None,
                },
            ],
            extra: Default::default(),
        }
    }

    #[test]
    fn render_then_parse_roundtrips_tricky_scalars() {
        let meta = sample();
        let doc = format!("{}\n# Title\n\nBody text.\n", render(&meta).unwrap());
        assert!(doc.starts_with("---\ntype: meeting\n"), "{doc}");
        let (back, body) = parse(&doc).unwrap();
        assert_eq!(back, meta);
        assert_eq!(body, "\n# Title\n\nBody text.\n");
    }

    #[test]
    fn parses_hand_written_frontmatter_like_the_plan_example() {
        let doc = "---\r\ntype: meeting\r\ntitle: Weekly sync\r\ndate: 2026-09-24T10:00:00+02:00\r\n\
                   duration: 00:42:10\r\ntags: [release, roadmap]\r\n\
                   participants:\r\n  - { name: Anna Rossi, email: anna@example.com }\r\n  - { name: Voice 2 }\r\n\
                   ---\r\nbody\r\n";
        let (m, body) = parse(doc).unwrap();
        assert_eq!(m.item_type, ItemType::Meeting);
        assert_eq!(m.date, "2026-09-24T10:00:00+02:00");
        assert_eq!(m.duration.as_deref(), Some("00:42:10"));
        assert_eq!(m.tags, vec!["release", "roadmap"]);
        assert_eq!(m.participants[0].email.as_deref(), Some("anna@example.com"));
        assert_eq!(m.participants[1].email, None);
        assert_eq!(body, "body\r\n");
    }

    #[test]
    fn unknown_keys_survive_parse_update_render() {
        let doc = "---\ntitle: Old\naliases: [weekly]\ncssclass: wide\nrating: 4\nnested:\n  a: 1\n---\n# Old\n";
        let (mut m, _) = parse(doc).unwrap();
        m.title = "New".into();
        let out = replace(doc, &m).unwrap();
        let (again, body) = parse(&out).unwrap();
        assert_eq!(again.title, "New");
        assert_eq!(again.extra["aliases"], serde_json::json!(["weekly"]));
        assert_eq!(again.extra["cssclass"], serde_json::json!("wide"));
        assert_eq!(again.extra["rating"], serde_json::json!(4));
        assert_eq!(again.extra["nested"], serde_json::json!({"a": 1}));
        assert_eq!(body, "# Old\n");
    }

    #[test]
    fn no_frontmatter_means_defaults_and_whole_body() {
        let (m, body) = parse("# Just markdown\n").unwrap();
        assert_eq!(m, ItemMeta::default());
        assert_eq!(body, "# Just markdown\n");
        // Unclosed block: not frontmatter.
        assert!(split("---\ntitle: x\n").is_none());
        // Replace on a file without frontmatter prepends one.
        let out = replace("hello\n", &sample()).unwrap();
        assert!(out.starts_with("---\n") && out.ends_with("---\nhello\n"));
    }

    #[test]
    fn empty_frontmatter_and_bom_are_accepted() {
        let (m, body) = parse("\u{feff}---\n---\nx").unwrap();
        assert_eq!(m, ItemMeta::default());
        assert_eq!(body, "x");
    }

    #[test]
    fn malformed_yaml_is_an_error() {
        assert!(parse("---\ntitle: [unclosed\n---\n").is_err());
    }
}
