//! The fixed corpus for speaker-aware recipes (#143, test only): small
//! synthetic multi-speaker transcripts in Italian and English
//! (`testdata/*.txt`), and the structural checks a recipe result on them
//! must pass — used with a fake model in unit tests and against a local
//! Ollama in the `#[ignore]` live test (`run::tests::live_meeting_recipes_on_ollama`).
//!
//! Fixture format: `#` comments; `key: value` headers (`type`, `title`,
//! `date`, `language`, `participant: Name <email>`, `speaker: id = Label`);
//! a `---` line; then one segment per line, `HH:MM:SS speaker-id | text`.

use crate::archive::{
    create_item, DocSpeaker, ItemMeta, ItemType, Participant, Segment, SegmentsFile,
};
use std::path::Path;

/// One transcript of the corpus.
pub struct Fixture {
    pub name: &'static str,
    pub meta: ItemMeta,
    pub segments: SegmentsFile,
}

impl Fixture {
    /// Speaker labels, in the order they are declared.
    pub fn speakers(&self) -> Vec<String> {
        self.segments
            .speakers
            .iter()
            .map(|s| s.label.clone())
            .collect()
    }

    /// Everyone a result may name: speakers and participants.
    pub fn people(&self) -> Vec<String> {
        let mut all = self.speakers();
        for p in &self.meta.participants {
            if !all.contains(&p.name) {
                all.push(p.name.clone());
            }
        }
        all
    }

    /// Store it as an archive item; returns the item id.
    pub fn create(&self, archive: &Path) -> String {
        create_item(archive, &self.meta, &self.segments).unwrap()
    }
}

const SOURCES: [(&str, &str); 3] = [
    ("it-standup", include_str!("testdata/it-standup.txt")),
    ("en-planning", include_str!("testdata/en-planning.txt")),
    ("it-intervista", include_str!("testdata/it-intervista.txt")),
];

/// The whole corpus.
pub fn all() -> Vec<Fixture> {
    SOURCES.iter().map(|(name, src)| parse(name, src)).collect()
}

/// One fixture by name.
pub fn fixture(name: &str) -> Fixture {
    all()
        .into_iter()
        .find(|f| f.name == name)
        .unwrap_or_else(|| panic!("no fixture {name}"))
}

fn parse_ts(ts: &str) -> u64 {
    let p: Vec<u64> = ts.split(':').map(|x| x.parse().unwrap()).collect();
    (p[0] * 3600 + p[1] * 60 + p[2]) * 1000
}

fn parse(name: &'static str, src: &str) -> Fixture {
    let mut meta = ItemMeta::default();
    let mut segs = SegmentsFile::default();
    let mut body = false;
    for line in src
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
    {
        if line == "---" {
            body = true;
            continue;
        }
        if body {
            let (head, text) = line
                .split_once(" | ")
                .unwrap_or_else(|| panic!("{name}: {line}"));
            let (ts, speaker) = head.split_once(' ').unwrap();
            let start = parse_ts(ts);
            assert!(
                segs.speakers.iter().any(|s| s.id == speaker),
                "{name}: unknown speaker {speaker}"
            );
            if let Some(prev) = segs.segments.last_mut() {
                prev.end_ms = start.saturating_sub(200).max(prev.start_ms);
            }
            segs.segments.push(Segment {
                id: segs.segments.len() as u32,
                start_ms: start,
                end_ms: start + 5_000,
                speaker_id: Some(speaker.to_string()),
                raw: text.to_string(),
                text: text.to_string(),
                ..Default::default()
            });
            continue;
        }
        let (key, value) = line
            .split_once(": ")
            .unwrap_or_else(|| panic!("{name}: {line}"));
        match key {
            "type" => {
                meta.item_type = match value {
                    "meeting" => ItemType::Meeting,
                    "transcription" => ItemType::Transcription,
                    _ => ItemType::Note,
                }
            }
            "title" => meta.title = value.to_string(),
            "date" => meta.date = value.to_string(),
            "language" => meta.language = value.to_string(),
            "participant" => {
                let (name, email) = match value.split_once(" <") {
                    Some((n, e)) => (n, Some(e.trim_end_matches('>').to_string())),
                    None => (value, None),
                };
                meta.participants.push(Participant {
                    name: name.to_string(),
                    email,
                });
            }
            "speaker" => {
                let (id, label) = value.split_once(" = ").unwrap();
                segs.speakers.push(DocSpeaker {
                    id: id.into(),
                    label: label.into(),
                    ..Default::default()
                });
            }
            other => panic!("{name}: unknown header {other}"),
        }
    }
    Fixture {
        name,
        meta,
        segments: segs,
    }
}

// ---- Structural checks on a recipe result ----

fn heading_text(line: &str) -> Option<(usize, String)> {
    let level = line.chars().take_while(|c| *c == '#').count();
    (level > 0 && line[level..].starts_with(' '))
        .then(|| (level, line[level..].trim().to_lowercase()))
}

/// The lines under the first heading whose text contains one of `names`
/// (lowercase), up to the next heading of the same or a higher level.
pub fn section<'a>(doc: &'a str, names: &[&str]) -> Option<Vec<&'a str>> {
    let lines: Vec<&str> = doc.lines().collect();
    let (start, level) = lines.iter().enumerate().find_map(|(i, l)| {
        let (level, text) = heading_text(l.trim())?;
        names.iter().any(|n| text.contains(n)).then_some((i, level))
    })?;
    let body = lines[start + 1..]
        .iter()
        .take_while(|l| heading_text(l.trim()).is_none_or(|(lv, _)| lv > level))
        .copied()
        .collect();
    Some(body)
}

/// Headings of the attendees section, in the languages of the corpus.
pub const ATTENDEES: [&str; 4] = ["attendees", "partecipanti", "presenti", "participants"];
/// Headings of the action items section.
pub const ACTIONS: [&str; 5] = ["action", "azioni", "attività", "compiti", "da fare"];

/// The owner column of the action-item table (rows after the header and
/// the `---` separator). `None` when the section or its table is missing.
pub fn action_owners(doc: &str) -> Option<Vec<String>> {
    let body = section(doc, &ACTIONS)?;
    let rows: Vec<Vec<String>> = body
        .iter()
        .map(|l| l.trim())
        .filter(|l| l.starts_with('|'))
        .map(|l| {
            l.trim_matches('|')
                .split('|')
                .map(|c| c.trim().to_string())
                .collect()
        })
        .collect();
    if rows.len() < 2 {
        return None;
    }
    Some(
        rows[1..]
            .iter()
            .filter(|r| {
                !r.iter()
                    .all(|c| c.chars().all(|ch| matches!(ch, '-' | ':' | ' ')))
            })
            .map(|r| r.get(1).cloned().unwrap_or_default())
            .collect(),
    )
}

fn words(s: &str) -> Vec<String> {
    s.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_string)
        .collect()
}

/// Whether `owner` (a table cell) names only people in `known`, or says
/// no owner was stated. Several owners (`Anna, Ben`, `Anna e Ben`) are
/// each checked; a first name alone matches its person; emails and
/// parenthesised notes are ignored.
pub fn owner_known(owner: &str, known: &[String]) -> bool {
    let mut o = owner.replace(['*', '_'], "");
    for (open, close) in [('(', ')'), ('<', '>')] {
        while let (Some(a), Some(b)) = (o.find(open), o.find(close)) {
            if b < a {
                break;
            }
            o.replace_range(a..=b, "");
        }
    }
    let none = [
        "",
        "—",
        "-",
        "–",
        "n/a",
        "tbd",
        "none",
        "nessuno",
        "non specificato",
        "not stated",
        "non indicato",
    ];
    if none.contains(&o.trim().to_lowercase().as_str()) {
        return true;
    }
    let known: Vec<Vec<String>> = known.iter().map(|k| words(k)).collect();
    o.split([',', '/', '&', ';'])
        .flat_map(|p| {
            p.split(" e ")
                .flat_map(|q| q.split(" and "))
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .map(|p| words(&p))
        .filter(|w| !w.is_empty())
        .all(|w| known.iter().any(|k| w.iter().all(|x| k.contains(x))))
}

/// Names of the `##` headings (the per-speaker sections of *Who said what*).
pub fn h2_names(doc: &str) -> Vec<String> {
    doc.lines()
        .filter_map(|l| l.trim().strip_prefix("## "))
        .map(|h| h.replace(['*', '_'], "").trim().to_string())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_corpus_parses() {
        let all = all();
        assert_eq!(all.len(), 3);
        for f in &all {
            assert!(
                !f.meta.title.is_empty() && !f.meta.language.is_empty(),
                "{}",
                f.name
            );
            assert!(f.segments.segments.len() >= 14, "{}", f.name);
            assert!(f.segments.speakers.len() >= 2, "{}: multi-speaker", f.name);
            assert!(
                f.speakers().iter().any(|s| s.starts_with("Voice ")),
                "{}: has a generic voice",
                f.name
            );
            let used: std::collections::HashSet<_> = f
                .segments
                .segments
                .iter()
                .filter_map(|s| s.speaker_id.as_deref())
                .collect();
            assert_eq!(
                used.len(),
                f.segments.speakers.len(),
                "{}: every speaker speaks",
                f.name
            );
        }
        let it = fixture("it-standup");
        assert_eq!(it.meta.item_type, ItemType::Meeting);
        assert_eq!(
            it.meta.participants[0].email.as_deref(),
            Some("marco.bianchi@example.com")
        );
        assert_eq!(it.segments.segments[1].start_ms, 15_000);
        assert_eq!(fixture("en-planning").meta.participants[2].email, None);
        assert_eq!(
            fixture("it-intervista").meta.item_type,
            ItemType::Transcription
        );
        assert!(fixture("it-intervista").meta.participants.is_empty());
    }

    const MINUTES: &str = "\
## Partecipanti
- Marco Bianchi
- Giulia Verdi
- Voice 1

## Azioni
| Azione | Responsabile | Scadenza |
|---|---|---|
| Preparare la demo | **Giulia Verdi** | giovedì |
| Aggiornare il budget | Marco | fine mese |
| Contattare il fornitore | Voice 1 | domani |
| Rivedere la documentazione | — | — |

## Altro
| x | Paolo Neri | y |";

    #[test]
    fn sections_and_action_owners_are_read_from_markdown() {
        let att = section(MINUTES, &ATTENDEES).unwrap();
        assert!(att.iter().any(|l| l.contains("Marco Bianchi")));
        assert!(
            !att.iter().any(|l| l.contains("Azione")),
            "stops at the next heading"
        );
        let owners = action_owners(MINUTES).unwrap();
        assert_eq!(owners, ["**Giulia Verdi**", "Marco", "Voice 1", "—"]);
        assert!(action_owners("## Summary\nnothing").is_none());
        assert!(action_owners("## Action items\nNone.").is_none());
    }

    #[test]
    fn owners_must_be_known_people() {
        let known = fixture("it-standup").people();
        for ok in [
            "Giulia Verdi",
            "**Giulia**",
            "Marco, Giulia",
            "Marco e Giulia",
            "Voice 1",
            "—",
            "",
            "Paolo Neri (paolo.neri@example.org)",
        ] {
            assert!(owner_known(ok, &known), "{ok}");
        }
        for bad in ["Luca", "Voice 3", "Giulia Rossi", "Marco, Luca", "il team"] {
            assert!(!owner_known(bad, &known), "{bad}");
        }
        assert_eq!(
            h2_names("## Anna Rossi\n- a\n## **Voice 2**\n### x"),
            ["Anna Rossi", "Voice 2"]
        );
    }
}
