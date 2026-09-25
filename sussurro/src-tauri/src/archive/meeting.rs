//! What the meeting page said while a browser session ran (0.9, #126):
//! who the page showed speaking and who was in the call, with timestamps.
//! The engine attributes remote lines from the same events as they arrive
//! (#131, `speakers::names`); this log keeps them for later passes and for
//! diagnosis.
//!
//! ```text
//! <item>/.sussurro/meeting-events.jsonl   one JSON event per line, appended
//! ```
//!
//! Append-only so an hour of speaker changes costs one short write each,
//! and a crash keeps everything written so far. A live session keeps the
//! file open ([`EventsWriter`], #217) instead of reopening it per event.

use super::store::{existing_item_dir, lock_items, META_DIR};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::{Path, PathBuf};

pub const MEETING_EVENTS_FILE: &str = "meeting-events.jsonl";

/// What saw a speaker (protocol 2, #131).
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum NameSource {
    /// The audio's RTP contributing sources: no indicator lag.
    Rtp,
    /// The page's speaking indicator (every protocol 1 event).
    #[default]
    Dom,
    /// The page's live captions (later than the indicator).
    Caption,
}

impl NameSource {
    fn is_dom(&self) -> bool {
        *self == NameSource::Dom
    }
}

/// One event. `at_ms` is the session position when the app received it
/// (the clock segments' `start_ms` use); `t_ms` is the client's own
/// timestamp, when it sent one. `id` / `source` / `speaker_idle` /
/// `speaker_name` / `observer_health` come from protocol 2 clients (#131);
/// lines written before them read the same as before.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MeetingEvent {
    SpeakerActive {
        at_ms: u64,
        t_ms: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        #[serde(default, skip_serializing_if = "NameSource::is_dom")]
        source: NameSource,
    },
    SpeakerIdle {
        at_ms: u64,
        t_ms: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
    },
    SpeakerName {
        at_ms: u64,
        id: String,
        #[serde(default)]
        name: Option<String>,
    },
    Participants {
        at_ms: u64,
        names: Vec<String>,
    },
    ObserverHealth {
        at_ms: u64,
        state: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        set: Option<String>,
        #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
        hooks: std::collections::BTreeMap<String, String>,
    },
}

fn events_path(dir: &Path) -> PathBuf {
    dir.join(META_DIR).join(MEETING_EVENTS_FILE)
}

fn to_lines(events: &[MeetingEvent]) -> Result<String> {
    let mut lines = String::new();
    for e in events {
        lines.push_str(&serde_json::to_string(e)?);
        lines.push('\n');
    }
    Ok(lines)
}

/// Item `id`'s log, opened for appending (created if missing).
fn open_events(archive: &Path, id: &str) -> Result<std::fs::File> {
    let _lock = lock_items();
    let dir = existing_item_dir(archive, id)?;
    std::fs::create_dir_all(dir.join(META_DIR))
        .with_context(|| format!("creating {}", dir.join(META_DIR).display()))?;
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(events_path(&dir))
        .context("opening the meeting events")
}

/// Append `events` to item `id`'s log.
pub fn append_events(archive: &Path, id: &str, events: &[MeetingEvent]) -> Result<()> {
    if events.is_empty() {
        return Ok(());
    }
    let lines = to_lines(events)?;
    open_events(archive, id)?
        .write_all(lines.as_bytes())
        .context("writing the meeting events")
}

/// Item `id`'s log held open by a live session: one open, then one write
/// per batch. The folder is resolved once, so a writer must be dropped
/// before the item can be renamed or deleted (Windows refuses to move a
/// folder with an open file) and reopened under the new id.
pub struct EventsWriter {
    id: String,
    file: std::fs::File,
}

impl EventsWriter {
    pub fn open(archive: &Path, id: &str) -> Result<Self> {
        Ok(Self {
            id: id.to_string(),
            file: open_events(archive, id)?,
        })
    }

    /// The item this writer appends to.
    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn append(&mut self, events: &[MeetingEvent]) -> Result<()> {
        if events.is_empty() {
            return Ok(());
        }
        let lines = to_lines(events)?;
        self.file
            .write_all(lines.as_bytes())
            .context("writing the meeting events")
    }
}

/// The events of item `id`, oldest first; unreadable lines are skipped.
pub fn read_events(archive: &Path, id: &str) -> Result<Vec<MeetingEvent>> {
    let dir = existing_item_dir(archive, id)?;
    let text = match std::fs::read_to_string(events_path(&dir)) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e.into()),
    };
    Ok(text
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::archive::store::create_item;
    use crate::archive::types::{ItemMeta, ItemType, SegmentsFile};

    #[test]
    fn events_append_and_read_back() {
        let tmp = tempfile::tempdir().unwrap();
        let archive = tmp.path().join("Sussurro");
        let meta = ItemMeta {
            item_type: ItemType::Meeting,
            title: "Sync".into(),
            date: "2026-09-24T10:00:00+02:00".into(),
            source: "browser:meet.google.com".into(),
            ..Default::default()
        };
        let id = create_item(&archive, &meta, &SegmentsFile::default()).unwrap();
        assert!(read_events(&archive, &id).unwrap().is_empty());
        let a = MeetingEvent::Participants {
            at_ms: 0,
            names: vec!["Anna".into(), "Bo".into()],
        };
        let b = MeetingEvent::SpeakerActive {
            at_ms: 1_500,
            t_ms: 1_480,
            name: Some("Anna".into()),
            id: None,
            source: NameSource::Dom,
        };
        append_events(&archive, &id, std::slice::from_ref(&a)).unwrap();
        append_events(&archive, &id, &[]).unwrap();
        append_events(&archive, &id, std::slice::from_ref(&b)).unwrap();
        assert_eq!(read_events(&archive, &id).unwrap(), vec![a, b.clone()]);
        let raw = std::fs::read_to_string(
            archive
                .join(&id)
                .join(META_DIR)
                .join(MEETING_EVENTS_FILE),
        )
        .unwrap();
        assert!(raw.lines().next().unwrap().contains(r#""kind":"participants""#));
        assert!(append_events(&archive, "2026/09/missing", std::slice::from_ref(&b)).is_err());
    }

    #[test]
    fn a_writer_keeps_the_file_open_and_appends() {
        let tmp = tempfile::tempdir().unwrap();
        let archive = tmp.path().join("Sussurro");
        let meta = ItemMeta {
            item_type: ItemType::Meeting,
            title: "Sync".into(),
            date: "2026-09-24T10:00:00+02:00".into(),
            source: "browser:meet.google.com".into(),
            ..Default::default()
        };
        let id = create_item(&archive, &meta, &SegmentsFile::default()).unwrap();
        let ev = |at_ms| MeetingEvent::SpeakerName {
            at_ms,
            id: "csrc:1".into(),
            name: Some("Anna".into()),
        };
        append_events(&archive, &id, &[ev(0)]).unwrap();
        let mut w = EventsWriter::open(&archive, &id).unwrap();
        assert_eq!(w.id(), id);
        w.append(&[ev(1)]).unwrap();
        w.append(&[]).unwrap();
        w.append(&[ev(2), ev(3)]).unwrap();
        assert_eq!(read_events(&archive, &id).unwrap(), vec![ev(0), ev(1), ev(2), ev(3)]);
        assert!(EventsWriter::open(&archive, "2026/09/missing").is_err());
    }

    #[test]
    fn protocol_one_lines_still_read_and_new_fields_are_optional() {
        let old = r#"{"kind":"speaker_active","at_ms":10,"t_ms":8,"name":"Anna"}"#;
        assert_eq!(
            serde_json::from_str::<MeetingEvent>(old).unwrap(),
            MeetingEvent::SpeakerActive {
                at_ms: 10,
                t_ms: 8,
                name: Some("Anna".into()),
                id: None,
                source: NameSource::Dom,
            }
        );
        let rtp = MeetingEvent::SpeakerActive {
            at_ms: 10,
            t_ms: 8,
            name: None,
            id: Some("csrc:42".into()),
            source: NameSource::Rtp,
        };
        let line = serde_json::to_string(&rtp).unwrap();
        assert_eq!(
            line,
            r#"{"kind":"speaker_active","at_ms":10,"t_ms":8,"id":"csrc:42","source":"rtp"}"#
        );
        assert_eq!(serde_json::from_str::<MeetingEvent>(&line).unwrap(), rtp);
        let health = MeetingEvent::ObserverHealth {
            at_ms: 1,
            state: "names_unavailable".into(),
            set: None,
            hooks: Default::default(),
        };
        assert_eq!(
            serde_json::to_string(&health).unwrap(),
            r#"{"kind":"observer_health","at_ms":1,"state":"names_unavailable"}"#
        );
    }
}
