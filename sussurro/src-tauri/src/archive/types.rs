//! Archive data model (plan §5): the YAML frontmatter of `transcript.md`
//! ([`ItemMeta`]) and `.sussurro/segments.json` ([`SegmentsFile`]).
//!
//! The frontmatter is user-editable, so its deserialization is lenient: a
//! scalar where a list is expected, a number where a string is expected or a
//! differently-cased `type` all load instead of breaking the item. Keys the
//! app does not know about are kept in [`ItemMeta::extra`] and written back
//! unchanged.

use serde::{Deserialize, Deserializer, Serialize};
use std::collections::BTreeMap;

/// Current `segments.json` schema version.
pub const SEGMENTS_VERSION: u32 = 1;

/// What an item is, chosen by content, not by source (P10).
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq, Default, Hash)]
#[serde(rename_all = "lowercase")]
pub enum ItemType {
    /// The user's own voice dictating a note.
    #[default]
    Note,
    /// A live conversation among several people.
    Meeting,
    /// Audio recorded by others (podcast, interview, lecture, link).
    Transcription,
}

impl ItemType {
    pub fn as_str(self) -> &'static str {
        match self {
            ItemType::Note => "note",
            ItemType::Meeting => "meeting",
            ItemType::Transcription => "transcription",
        }
    }

    /// Case-insensitive parse; `None` for anything unknown.
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "note" => Some(ItemType::Note),
            "meeting" => Some(ItemType::Meeting),
            "transcription" => Some(ItemType::Transcription),
            _ => None,
        }
    }
}

impl<'de> Deserialize<'de> for ItemType {
    /// Unknown or hand-mangled values fall back to `note` rather than making
    /// the whole item unreadable.
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = lenient_string(d)?;
        Ok(ItemType::parse(&s).unwrap_or_default())
    }
}

/// A participant in a meeting or transcription.
#[derive(Debug, Clone, Serialize, PartialEq, Eq, Default)]
pub struct Participant {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
}

impl<'de> Deserialize<'de> for Participant {
    /// Accepts `{ name, email? }` or a bare string (`- Anna Rossi`).
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let v = serde_json::Value::deserialize(d)?;
        match v {
            serde_json::Value::Object(map) => {
                let name = map.get("name").map(value_to_string).unwrap_or_default();
                let email = map
                    .get("email")
                    .map(value_to_string)
                    .filter(|e| !e.trim().is_empty());
                Ok(Participant { name, email })
            }
            other => Ok(Participant {
                name: value_to_string(&other),
                email: None,
            }),
        }
    }
}

/// Frontmatter of `transcript.md` — the source of truth for item metadata.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct ItemMeta {
    #[serde(rename = "type", default)]
    pub item_type: ItemType,
    #[serde(default, deserialize_with = "lenient_string")]
    pub title: String,
    /// RFC 3339, e.g. `2026-09-24T10:00:00+02:00`.
    #[serde(default, deserialize_with = "lenient_string")]
    pub date: String,
    /// `HH:MM:SS`.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "lenient_opt_string"
    )]
    pub duration: Option<String>,
    /// `mic` | `file:<name>` | `url:<link>` | `browser:<host>` | `system`.
    #[serde(default, deserialize_with = "lenient_string")]
    pub source: String,
    #[serde(default, deserialize_with = "lenient_string")]
    pub language: String,
    #[serde(default, deserialize_with = "lenient_string")]
    pub engine: String,
    #[serde(default, deserialize_with = "lenient_string_list")]
    pub tags: Vec<String>,
    #[serde(default, deserialize_with = "lenient_string_list")]
    pub categories: Vec<String>,
    #[serde(default, deserialize_with = "lenient_participants")]
    pub participants: Vec<Participant>,
    /// Frontmatter keys written by the user (or another tool, e.g. Obsidian)
    /// that Sussurro does not model. Preserved across read → update → write.
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

/// Logical capture channel a segment came from.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum Channel {
    #[default]
    Mic,
    Remote,
    System,
    File,
}

/// A word with its timing, where the engine provides one.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct Word {
    pub w: String,
    pub start_ms: u64,
    pub end_ms: u64,
}

/// One transcribed segment (a VAD chunk of at most ~30 s).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct Segment {
    pub id: u32,
    #[serde(default)]
    pub channel: Channel,
    pub start_ms: u64,
    pub end_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speaker_id: Option<String>,
    /// Raw STT output.
    #[serde(default)]
    pub raw: String,
    /// Cleaned text (what the transcript shows).
    #[serde(default)]
    pub text: String,
    /// The user edited `text` by hand in the app.
    #[serde(default)]
    pub edited: bool,
    #[serde(default)]
    pub words: Vec<Word>,
    /// `words` were split proportionally over the segment (the engine gave
    /// no timings), not measured — consumers (SRT, playback highlight) may
    /// want to treat them as approximate. Omitted from JSON when false.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub words_estimated: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub embedding: Option<Vec<f32>>,
}

/// A speaker as known to one document.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct DocSpeaker {
    /// `"you"` | `"meet:<name>"` | `"voice:<n>"`.
    pub id: String,
    pub label: String,
    #[serde(default)]
    pub color: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub person_id: Option<String>,
}

/// `.sussurro/segments.json`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SegmentsFile {
    pub version: u32,
    #[serde(default)]
    pub speakers: Vec<DocSpeaker>,
    #[serde(default)]
    pub segments: Vec<Segment>,
}

impl Default for SegmentsFile {
    fn default() -> Self {
        Self {
            version: SEGMENTS_VERSION,
            speakers: Vec::new(),
            segments: Vec::new(),
        }
    }
}

// ---- lenient deserializers -------------------------------------------------

/// Any scalar as a string: YAML `title: 2026` or `language: no` must still
/// load as text. Null becomes empty; lists/maps become compact JSON.
fn value_to_string(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::Null => String::new(),
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Bool(b) => b.to_string(),
        serde_json::Value::Number(n) => n.to_string(),
        other => other.to_string(),
    }
}

fn lenient_string<'de, D: Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    Ok(value_to_string(&serde_json::Value::deserialize(d)?))
}

fn lenient_opt_string<'de, D: Deserializer<'de>>(d: D) -> Result<Option<String>, D::Error> {
    let s = lenient_string(d)?;
    Ok((!s.trim().is_empty()).then_some(s))
}

/// A list of strings, or a single scalar (`tags: roadmap`), or a
/// comma-separated string (`tags: a, b`). Empty entries are dropped.
fn lenient_string_list<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<String>, D::Error> {
    let v = serde_json::Value::deserialize(d)?;
    let items: Vec<String> = match v {
        serde_json::Value::Null => Vec::new(),
        serde_json::Value::Array(xs) => xs.iter().map(value_to_string).collect(),
        serde_json::Value::String(s) => s.split(',').map(str::to_string).collect(),
        other => vec![value_to_string(&other)],
    };
    Ok(items
        .into_iter()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect())
}

fn lenient_participants<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<Participant>, D::Error> {
    let v = serde_json::Value::deserialize(d)?;
    let xs = match v {
        serde_json::Value::Null => return Ok(Vec::new()),
        serde_json::Value::Array(xs) => xs,
        other => vec![other],
    };
    Ok(xs
        .into_iter()
        .filter_map(|x| serde_json::from_value::<Participant>(x).ok())
        .filter(|p| !p.name.trim().is_empty())
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn item_type_serializes_lowercase_and_parses_leniently() {
        assert_eq!(
            serde_json::to_string(&ItemType::Meeting).unwrap(),
            "\"meeting\""
        );
        let t: ItemType = serde_json::from_str("\"Transcription\"").unwrap();
        assert_eq!(t, ItemType::Transcription);
        let unknown: ItemType = serde_json::from_str("\"memo\"").unwrap();
        assert_eq!(unknown, ItemType::Note);
    }

    #[test]
    fn channel_serializes_lowercase() {
        assert_eq!(
            serde_json::to_string(&Channel::Remote).unwrap(),
            "\"remote\""
        );
        let c: Channel = serde_json::from_str("\"file\"").unwrap();
        assert_eq!(c, Channel::File);
    }

    #[test]
    fn segments_file_roundtrips_and_omits_empty_optionals() {
        let f = SegmentsFile {
            version: SEGMENTS_VERSION,
            speakers: vec![DocSpeaker {
                id: "you".into(),
                label: "You".into(),
                color: "#f00".into(),
                person_id: None,
            }],
            segments: vec![Segment {
                id: 1,
                channel: Channel::Mic,
                start_ms: 0,
                end_ms: 1500,
                speaker_id: Some("you".into()),
                raw: "ehm ciao".into(),
                text: "Ciao.".into(),
                edited: false,
                words: vec![Word {
                    w: "ciao".into(),
                    start_ms: 200,
                    end_ms: 600,
                }],
                words_estimated: false,
                embedding: None,
            }],
        };
        let json = serde_json::to_string(&f).unwrap();
        assert!(!json.contains("embedding"));
        assert!(!json.contains("words_estimated"));
        assert!(!json.contains("person_id"));
        assert_eq!(serde_json::from_str::<SegmentsFile>(&json).unwrap(), f);
    }

    #[test]
    fn segment_defaults_fill_missing_fields() {
        let s: Segment =
            serde_json::from_str(r#"{"id":3,"start_ms":10,"end_ms":20,"text":"x"}"#).unwrap();
        assert_eq!(s.channel, Channel::Mic);
        assert!(s.words.is_empty() && !s.edited && s.speaker_id.is_none());
    }

    #[test]
    fn meta_lenient_fields_and_extras() {
        let m: ItemMeta = serde_json::from_value(serde_json::json!({
            "type": "MEETING",
            "title": 2026,
            "tags": "release, roadmap",
            "categories": ["team", 7],
            "participants": ["Anna", {"name": "Bob", "email": "bob@example.com"}, {"email": "x"}],
            "aliases": ["weekly"],
        }))
        .unwrap();
        assert_eq!(m.item_type, ItemType::Meeting);
        assert_eq!(m.title, "2026");
        assert_eq!(m.tags, vec!["release", "roadmap"]);
        assert_eq!(m.categories, vec!["team", "7"]);
        assert_eq!(m.participants.len(), 2);
        assert_eq!(m.participants[1].email.as_deref(), Some("bob@example.com"));
        assert_eq!(m.extra["aliases"], serde_json::json!(["weekly"]));
    }
}
