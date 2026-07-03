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

/// Entries whose raw or cleaned text contains `query` (case-insensitive),
/// newest first, capped at `n`. Empty query behaves like read_last.
pub fn search(path: &Path, query: &str, n: usize) -> Vec<HistoryEntry> {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return read_last(path, n);
    }
    let Ok(content) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let mut hits: Vec<HistoryEntry> = content
        .lines()
        .filter_map(|l| serde_json::from_str::<HistoryEntry>(l).ok())
        .filter(|e| {
            e.raw.to_lowercase().contains(&q) || e.cleaned.to_lowercase().contains(&q)
        })
        .collect();
    hits.reverse();
    hits.truncate(n);
    hits
}

/// Drop entries older than `days` (0 = keep everything). Rewrites the file
/// only when something was actually removed. Returns the removed count.
pub fn prune_older_than(path: &Path, days: u32) -> std::io::Result<usize> {
    if days == 0 {
        return Ok(0);
    }
    let Ok(content) = std::fs::read_to_string(path) else {
        return Ok(0); // no file, nothing to prune
    };
    let cutoff = chrono::Utc::now() - chrono::Duration::days(days as i64);
    let mut kept: Vec<&str> = Vec::new();
    let mut removed = 0usize;
    for line in content.lines() {
        let old = serde_json::from_str::<HistoryEntry>(line)
            .ok()
            .and_then(|e| chrono::DateTime::parse_from_rfc3339(&e.timestamp).ok())
            .map(|t| t.with_timezone(&chrono::Utc) < cutoff)
            .unwrap_or(false); // unparseable lines are kept, not silently dropped
        if old {
            removed += 1;
        } else {
            kept.push(line);
        }
    }
    if removed > 0 {
        let mut out = kept.join("\n");
        if !out.is_empty() {
            out.push('\n');
        }
        std::fs::write(path, out)?;
    }
    Ok(removed)
}

/// Delete all history. A missing file already counts as cleared.
pub fn clear(path: &Path) -> std::io::Result<()> {
    match std::fs::remove_file(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        r => r,
    }
}

/// Render entries (given oldest-first) as a Markdown document for export.
pub fn to_markdown(entries: &[HistoryEntry]) -> String {
    let mut out = String::from("# Sussurro — dictation history\n");
    for e in entries {
        let when = chrono::DateTime::parse_from_rfc3339(&e.timestamp)
            .map(|t| t.format("%Y-%m-%d %H:%M").to_string())
            .unwrap_or_else(|_| e.timestamp.clone());
        out.push_str(&format!("\n## {when}\n\n{}\n", e.cleaned));
        if e.raw != e.cleaned {
            out.push_str(&format!("\n> raw: {}\n", e.raw.replace('\n', "\n> ")));
        }
    }
    out
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

    #[test]
    fn search_matches_raw_and_cleaned_case_insensitive() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("history.jsonl");
        append(&path, &HistoryEntry {
            timestamp: "2026-07-01T10:00:00Z".into(),
            raw: "um parliamo di SUSSURRO".into(),
            cleaned: "Parliamo del progetto.".into(),
        }).unwrap();
        append(&path, &HistoryEntry {
            timestamp: "2026-07-02T10:00:00Z".into(),
            raw: "altro testo".into(),
            cleaned: "Altro testo.".into(),
        }).unwrap();
        assert_eq!(search(&path, "sussurro", 10).len(), 1); // matches raw
        assert_eq!(search(&path, "progetto", 10).len(), 1); // matches cleaned
        assert_eq!(search(&path, "niente", 10).len(), 0);
        assert_eq!(search(&path, "  ", 10).len(), 2); // blank = read_last
    }

    #[test]
    fn prune_drops_only_old_entries() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("history.jsonl");
        let old = (chrono::Utc::now() - chrono::Duration::days(40)).to_rfc3339();
        let recent = chrono::Utc::now().to_rfc3339();
        append(&path, &HistoryEntry { timestamp: old, raw: "old".into(), cleaned: "old".into() }).unwrap();
        append(&path, &HistoryEntry { timestamp: recent, raw: "new".into(), cleaned: "new".into() }).unwrap();
        assert_eq!(prune_older_than(&path, 30).unwrap(), 1);
        let left = read_last(&path, 10);
        assert_eq!(left.len(), 1);
        assert_eq!(left[0].raw, "new");
        // 0 = keep forever
        assert_eq!(prune_older_than(&path, 0).unwrap(), 0);
        // missing file is fine
        assert_eq!(prune_older_than(&dir.path().join("nope.jsonl"), 30).unwrap(), 0);
    }

    #[test]
    fn markdown_export_has_headers_and_raw_quotes() {
        let md = to_markdown(&[
            entry(1),
            HistoryEntry {
                timestamp: "2026-07-03T09:30:00Z".into(),
                raw: "same text".into(),
                cleaned: "same text".into(),
            },
        ]);
        assert!(md.starts_with("# Sussurro"));
        assert!(md.contains("## 2026-07-02 12:00"));
        assert!(md.contains("clean 1"));
        assert!(md.contains("> raw: um raw 1"));
        // raw == cleaned: no redundant quote block
        assert_eq!(md.matches("> raw:").count(), 1);
    }

    #[test]
    fn clear_removes_all_entries_and_tolerates_missing_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("history.jsonl");
        append(&path, &entry(1)).unwrap();
        clear(&path).unwrap();
        assert!(read_last(&path, 10).is_empty());
        clear(&path).unwrap(); // second clear: file already gone, still Ok
    }
}
