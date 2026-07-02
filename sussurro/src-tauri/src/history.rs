use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HistoryEntry {
    pub timestamp: String, // RFC 3339
    pub raw: String,
    pub cleaned: String,
}

pub fn append(path: &Path, entry: &HistoryEntry) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut f = std::fs::OpenOptions::new().create(true).append(true).open(path)?;
    writeln!(f, "{}", serde_json::to_string(entry).expect("entry serialize"))
}

/// Newest first. Corrupt lines are skipped, not fatal.
pub fn read_last(path: &Path, n: usize) -> Vec<HistoryEntry> {
    let Ok(content) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let mut entries: Vec<HistoryEntry> = content
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect();
    entries.reverse();
    entries.truncate(n);
    entries
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(n: u32) -> HistoryEntry {
        HistoryEntry {
            timestamp: format!("2026-07-02T12:00:0{n}Z"),
            raw: format!("um raw {n}"),
            cleaned: format!("clean {n}"),
        }
    }

    #[test]
    fn append_then_read_roundtrips_in_reverse_order() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("history.jsonl");
        for n in 0..3 {
            append(&path, &entry(n)).unwrap();
        }
        let got = read_last(&path, 10);
        assert_eq!(got.len(), 3);
        assert_eq!(got[0], entry(2)); // newest first
        assert_eq!(got[2], entry(0));
    }

    #[test]
    fn read_last_limits_count() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("history.jsonl");
        for n in 0..5 {
            append(&path, &entry(n)).unwrap();
        }
        assert_eq!(read_last(&path, 2).len(), 2);
    }

    #[test]
    fn missing_file_reads_empty() {
        let dir = tempfile::tempdir().unwrap();
        assert!(read_last(&dir.path().join("nope.jsonl"), 10).is_empty());
    }
}
