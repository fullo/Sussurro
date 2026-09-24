//! What the meeting page said while a browser session ran (0.9, #126):
//! who the page showed speaking and who was in the call, with timestamps.
//! Stored for speaker attribution (#131), which is not done yet.
//!
//! ```text
//! <item>/.sussurro/meeting-events.jsonl   one JSON event per line, appended
//! ```
//!
//! Append-only so an hour of speaker changes costs one short write each,
//! and a crash keeps everything written so far.

use super::store::{existing_item_dir, lock_items, META_DIR};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::{Path, PathBuf};

pub const MEETING_EVENTS_FILE: &str = "meeting-events.jsonl";

/// One event. `at_ms` is the session position when the app received it
/// (the clock segments' `start_ms` use); `t_ms` is the client's own
/// timestamp, when it sent one.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MeetingEvent {
    SpeakerActive {
        at_ms: u64,
        t_ms: u64,
        name: String,
    },
    Participants {
        at_ms: u64,
        names: Vec<String>,
    },
}

fn events_path(dir: &Path) -> PathBuf {
    dir.join(META_DIR).join(MEETING_EVENTS_FILE)
}

/// Append `events` to item `id`'s log.
pub fn append_events(archive: &Path, id: &str, events: &[MeetingEvent]) -> Result<()> {
    if events.is_empty() {
        return Ok(());
    }
    let _lock = lock_items();
    let dir = existing_item_dir(archive, id)?;
    std::fs::create_dir_all(dir.join(META_DIR))
        .with_context(|| format!("creating {}", dir.join(META_DIR).display()))?;
    let mut lines = String::new();
    for e in events {
        lines.push_str(&serde_json::to_string(e)?);
        lines.push('\n');
    }
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(events_path(&dir))
        .context("opening the meeting events")?;
    f.write_all(lines.as_bytes())
        .context("writing the meeting events")
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
            name: "Anna".into(),
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
}
