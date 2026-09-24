//! Word timings (plan §4.2, #108). Engines that report timings (whisper
//! token timestamps, Parakeet TDT frames) give words relative to the
//! segment; [`place_words`] moves them onto the recording's timeline. When an
//! engine gives none, [`proportional_words`] splits the segment's duration
//! over its words by character count (marked estimated by the caller).

use crate::archive::Word;

/// Split `text` into whitespace-separated words and share `[start_ms,
/// end_ms)` between them in proportion to their length in characters
/// (at least one character each, so punctuation-only tokens still get a
/// sliver). Contiguous, ordered, and the last word ends at `end_ms`. Pure.
pub fn proportional_words(text: &str, start_ms: u64, end_ms: u64) -> Vec<Word> {
    let words: Vec<&str> = text.split_whitespace().collect();
    if words.is_empty() {
        return Vec::new();
    }
    let weights: Vec<u64> = words
        .iter()
        .map(|w| w.chars().count().max(1) as u64)
        .collect();
    let total: u64 = weights.iter().sum();
    let span = end_ms.saturating_sub(start_ms);
    let mut acc = 0u64;
    words
        .iter()
        .zip(&weights)
        .map(|(w, weight)| {
            let s = start_ms + span * acc / total;
            acc += weight;
            let e = start_ms + span * acc / total;
            Word {
                w: (*w).to_string(),
                start_ms: s,
                end_ms: e,
            }
        })
        .collect()
}

/// Shift segment-relative words by `offset_ms` and clamp them inside the
/// segment `[0, duration_ms]` (token timestamps can overshoot the audio by a
/// frame or two), keeping them ordered. Pure.
pub fn place_words(words: &[Word], offset_ms: u64, duration_ms: u64) -> Vec<Word> {
    let mut last_end = 0u64;
    words
        .iter()
        .map(|w| {
            let start = w.start_ms.min(duration_ms).max(last_end.min(duration_ms));
            let end = w.end_ms.min(duration_ms).max(start);
            last_end = end;
            Word {
                w: w.w.clone(),
                start_ms: offset_ms + start,
                end_ms: offset_ms + end,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn triple(words: &[Word]) -> Vec<(&str, u64, u64)> {
        words
            .iter()
            .map(|w| (w.w.as_str(), w.start_ms, w.end_ms))
            .collect()
    }

    #[test]
    fn proportional_split_follows_character_counts() {
        // 2 + 4 + 4 = 10 chars over 1000 ms.
        let words = proportional_words("hi four word", 1_000, 2_000);
        assert_eq!(
            triple(&words),
            vec![
                ("hi", 1_000, 1_200),
                ("four", 1_200, 1_600),
                ("word", 1_600, 2_000)
            ]
        );
    }

    #[test]
    fn proportional_split_is_contiguous_and_ends_on_time() {
        let words = proportional_words("  uno, due   tre... quattro! ", 0, 3_333);
        assert_eq!(words.len(), 4);
        assert_eq!(words[0].start_ms, 0);
        assert_eq!(words.last().unwrap().end_ms, 3_333);
        for w in words.windows(2) {
            assert_eq!(w[0].end_ms, w[1].start_ms);
            assert!(w[0].start_ms <= w[0].end_ms);
        }
    }

    #[test]
    fn proportional_split_edge_cases() {
        assert!(proportional_words("   ", 0, 1_000).is_empty());
        // Zero-length segment: every word collapses onto the start.
        let words = proportional_words("a b", 500, 500);
        assert_eq!(triple(&words), vec![("a", 500, 500), ("b", 500, 500)]);
        // Multibyte characters count as one each.
        let words = proportional_words("è ab", 0, 300);
        assert_eq!(triple(&words), vec![("è", 0, 100), ("ab", 100, 300)]);
    }

    #[test]
    fn placed_words_are_offset_clamped_and_ordered() {
        let rel = vec![
            Word {
                w: "a".into(),
                start_ms: 0,
                end_ms: 400,
            },
            Word {
                w: "b".into(),
                start_ms: 300,
                end_ms: 900,
            }, // overlaps a
            Word {
                w: "c".into(),
                start_ms: 950,
                end_ms: 1_200,
            }, // past the end
        ];
        let placed = place_words(&rel, 10_000, 1_000);
        assert_eq!(
            triple(&placed),
            vec![
                ("a", 10_000, 10_400),
                ("b", 10_400, 10_900),
                ("c", 10_950, 11_000)
            ]
        );
    }
}
