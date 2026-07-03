//! Persistent usage counters, independent of history retention: pruning or
//! clearing the dictation history never touches these numbers.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

/// How many daily buckets to keep. Totals are forever; per-day detail only
/// needs to cover "this week / this month" style summaries.
const KEEP_DAYS: usize = 90;

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct DayStats {
    pub dictations: u64,
    pub words: u64,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Stats {
    pub total_dictations: u64,
    pub total_words: u64,
    /// Per-day buckets keyed "YYYY-MM-DD" (BTreeMap: keys sort by date).
    #[serde(default)]
    pub days: BTreeMap<String, DayStats>,
}

pub fn load(path: &Path) -> Stats {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save(path: &Path, stats: &Stats) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, serde_json::to_string(stats).unwrap_or_default())
}

/// Count one dictation of `words` words under `day` ("YYYY-MM-DD"), keeping
/// only the newest KEEP_DAYS daily buckets. Best-effort: IO errors are the
/// caller's to ignore — stats must never break the dictation pipeline.
pub fn record(path: &Path, day: &str, words: u64) -> std::io::Result<()> {
    let mut stats = load(path);
    stats.total_dictations += 1;
    stats.total_words += words;
    let bucket = stats.days.entry(day.to_string()).or_default();
    bucket.dictations += 1;
    bucket.words += words;
    while stats.days.len() > KEEP_DAYS {
        let oldest = stats.days.keys().next().cloned().expect("non-empty");
        stats.days.remove(&oldest);
    }
    save(path, &stats)
}

/// Whitespace-separated word count of a transcript.
pub fn word_count(text: &str) -> u64 {
    text.split_whitespace().count() as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp() -> std::path::PathBuf {
        std::env::temp_dir().join(format!("sussurro-stats-{}.json", std::process::id()))
    }

    #[test]
    fn missing_file_loads_default() {
        let s = load(Path::new("does-not-exist-anywhere.json"));
        assert_eq!(s.total_dictations, 0);
        assert!(s.days.is_empty());
    }

    #[test]
    fn record_accumulates_totals_and_day_buckets() {
        let path = tmp();
        let _ = std::fs::remove_file(&path);
        record(&path, "2026-07-03", 10).unwrap();
        record(&path, "2026-07-03", 5).unwrap();
        record(&path, "2026-07-04", 7).unwrap();
        let s = load(&path);
        assert_eq!(s.total_dictations, 3);
        assert_eq!(s.total_words, 22);
        assert_eq!(s.days["2026-07-03"].dictations, 2);
        assert_eq!(s.days["2026-07-03"].words, 15);
        assert_eq!(s.days["2026-07-04"].words, 7);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn old_day_buckets_are_pruned_but_totals_stay() {
        let path = tmp().with_extension("prune.json");
        let _ = std::fs::remove_file(&path);
        for i in 0..KEEP_DAYS + 10 {
            // Fake but sortable ISO-like keys: 2026-001, 2026-002, …
            record(&path, &format!("2026-{i:03}"), 1).unwrap();
        }
        let s = load(&path);
        assert_eq!(s.days.len(), KEEP_DAYS);
        assert_eq!(s.total_dictations, (KEEP_DAYS + 10) as u64);
        // The oldest keys are the ones dropped.
        assert!(!s.days.contains_key("2026-000"));
        assert!(s.days.contains_key(&format!("2026-{:03}", KEEP_DAYS + 9)));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn words_are_counted_by_whitespace() {
        assert_eq!(word_count(""), 0);
        assert_eq!(word_count("ciao"), 1);
        assert_eq!(word_count("  ciao   mondo \n come va  "), 4);
    }
}
