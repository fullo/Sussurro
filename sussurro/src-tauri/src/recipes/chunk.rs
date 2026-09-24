//! The transcript as recipe input (pure): one text line per segment, with
//! its timestamp and speaker when the item has them, and greedy chunking of
//! those lines into pieces that fit a character budget.
//!
//! Sizes are counted in characters, and tokens are estimated at
//! [`CHARS_PER_TOKEN`] characters each — conservative for Italian and
//! English prose (typically 3.5–4.5), so a chunk rarely overflows a
//! model's window even for accented or mixed-language text.

use crate::archive::render::format_timestamp;
use crate::archive::{ItemMeta, ItemType, SegmentsFile};

/// Characters per token assumed when sizing chunks.
pub const CHARS_PER_TOKEN: usize = 3;
/// Share of the context window kept free for the model's answer…
const OUTPUT_SHARE: usize = 4; // 1/4
/// …but never less than this many tokens.
const MIN_OUTPUT_TOKENS: usize = 512;
/// Slack for chat-template tokens and estimation error.
const SAFETY_TOKENS: usize = 64;
/// Smallest input budget ever planned (tokens), however large the prompt.
const MIN_INPUT_TOKENS: usize = 256;

/// One line of input: `[00:12:03] Anna: text`, `[00:12:03] text` or `text`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct InputLine {
    pub start_ms: Option<u64>,
    pub speaker: Option<String>,
    pub text: String,
}

impl InputLine {
    pub fn format(&self) -> String {
        let text = self.text.trim();
        match (self.start_ms, self.speaker.as_deref()) {
            (Some(ms), Some(sp)) => format!("[{}] {sp}: {text}", format_timestamp(ms)),
            (Some(ms), None) => format!("[{}] {text}", format_timestamp(ms)),
            (None, Some(sp)) => format!("{sp}: {text}"),
            (None, None) => text.to_string(),
        }
    }
}

/// Input lines from an item's segments. Meetings and transcriptions keep
/// each segment's timestamp; speakers are named wherever the item has
/// them (any type). Notes are plain dictated text. Empty segments (an STT
/// failure) are skipped.
pub fn lines_from_segments(meta: &ItemMeta, segs: &SegmentsFile) -> Vec<InputLine> {
    let timed = meta.item_type != ItemType::Note;
    segs.segments
        .iter()
        .filter(|s| !s.text.trim().is_empty())
        .map(|s| {
            let speaker = s
                .speaker_id
                .as_deref()
                .and_then(|id| segs.speakers.iter().find(|sp| sp.id == id))
                .map(|sp| sp.label.split_whitespace().collect::<Vec<_>>().join(" "))
                .filter(|l| !l.is_empty());
            InputLine {
                start_ms: timed.then_some(s.start_ms),
                speaker,
                text: s.text.split_whitespace().collect::<Vec<_>>().join(" "),
            }
        })
        .collect()
}

/// Input lines from the markdown body (an item edited outside Sussurro —
/// the markdown wins — or one written by hand without segments): its
/// non-empty lines, minus the leading `# title`. Timestamps and speakers
/// the body carries (`**[00:01:02] Anna:** …`) stay in the text.
pub fn lines_from_body(body: &str) -> Vec<InputLine> {
    let mut lines = body.lines().map(str::trim).filter(|l| !l.is_empty()).peekable();
    if lines.peek().is_some_and(|l| l.starts_with("# ")) {
        lines.next();
    }
    lines
        .map(|l| InputLine {
            text: l.to_string(),
            ..Default::default()
        })
        .collect()
}

/// Whether any line names a speaker (the prompt then explains the format).
pub fn has_speakers(lines: &[InputLine]) -> bool {
    lines.iter().any(|l| l.speaker.is_some())
}

/// Characters of transcript input that fit in one call to a model with a
/// `context_tokens` window, when the rest of the prompt (instructions,
/// header) takes `overhead_chars`. A quarter of the window (at least 512
/// tokens) stays free for the answer.
pub fn input_budget_chars(context_tokens: u32, overhead_chars: usize) -> usize {
    let ctx = context_tokens as usize;
    let output = (ctx / OUTPUT_SHARE).max(MIN_OUTPUT_TOKENS).min(ctx / 2);
    let overhead = overhead_chars.div_ceil(CHARS_PER_TOKEN);
    let input = ctx
        .saturating_sub(output)
        .saturating_sub(overhead)
        .saturating_sub(SAFETY_TOKENS)
        .max(MIN_INPUT_TOKENS);
    input * CHARS_PER_TOKEN
}

fn char_len(s: &str) -> usize {
    s.chars().count()
}

/// Split one over-long line into pieces of at most `budget` characters,
/// preferring sentence ends, then spaces, then a hard cut.
pub fn split_long_line(line: &str, budget: usize) -> Vec<String> {
    let budget = budget.max(1);
    let mut out = Vec::new();
    let mut rest = line.trim();
    while char_len(rest) > budget {
        // Byte offset of the budget-th character.
        let limit = rest.char_indices().nth(budget).map(|(i, _)| i).unwrap_or(rest.len());
        let window = &rest[..limit];
        let cut = ['.', '?', '!', ';']
            .iter()
            .filter_map(|p| window.rfind(&format!("{p} ")).map(|i| i + 1))
            .max()
            .filter(|&i| i > limit / 3)
            .or_else(|| window.rfind(' ').filter(|&i| i > 0))
            .unwrap_or(limit);
        out.push(rest[..cut].trim().to_string());
        rest = rest[cut..].trim_start();
    }
    if !rest.is_empty() {
        out.push(rest.to_string());
    }
    out
}

/// Pack formatted lines, in order, into chunks of at most `budget`
/// characters (lines joined by `\n`). A chunk never splits a line unless
/// the line alone is over budget, in which case it is split with
/// [`split_long_line`]. Every character of every line ends up in exactly
/// one chunk, in the original order.
pub fn chunk_lines(lines: &[String], budget: usize) -> Vec<String> {
    let budget = budget.max(1);
    let mut chunks = Vec::new();
    let mut current = String::new();
    let mut current_len = 0;
    let pieces = lines.iter().flat_map(|l| {
        if char_len(l) > budget {
            split_long_line(l, budget)
        } else {
            vec![l.clone()]
        }
    });
    for piece in pieces {
        let len = char_len(&piece);
        let sep = usize::from(!current.is_empty());
        if current_len + sep + len > budget && !current.is_empty() {
            chunks.push(std::mem::take(&mut current));
            current_len = 0;
        }
        if !current.is_empty() {
            current.push('\n');
            current_len += 1;
        }
        current.push_str(&piece);
        current_len += len;
    }
    if !current.is_empty() {
        chunks.push(current);
    }
    chunks
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::archive::{DocSpeaker, Segment};

    fn seg(id: u32, start: u64, speaker: Option<&str>, text: &str) -> Segment {
        Segment {
            id,
            start_ms: start,
            end_ms: start + 1000,
            speaker_id: speaker.map(str::to_string),
            text: text.into(),
            raw: text.into(),
            ..Default::default()
        }
    }

    fn meta(t: ItemType) -> ItemMeta {
        ItemMeta { item_type: t, title: "T".into(), ..Default::default() }
    }

    #[test]
    fn meeting_lines_carry_timestamps_and_speaker_labels() {
        let segs = SegmentsFile {
            speakers: vec![DocSpeaker { id: "meet:anna".into(), label: "Anna  Rossi".into(), ..Default::default() }],
            segments: vec![
                seg(0, 3_723_000, Some("meet:anna"), " Partiamo  dalla 0.7. "),
                seg(1, 3_730_000, Some("voice:9"), "Ok."),
                seg(2, 3_731_000, None, "   "),
            ],
            ..Default::default()
        };
        let lines: Vec<_> = lines_from_segments(&meta(ItemType::Meeting), &segs)
            .iter()
            .map(InputLine::format)
            .collect();
        assert_eq!(lines, ["[01:02:03] Anna Rossi: Partiamo dalla 0.7.", "[01:02:10] Ok."]);
    }

    #[test]
    fn note_lines_are_plain_text_but_keep_speakers() {
        let segs = SegmentsFile {
            speakers: vec![DocSpeaker { id: "you".into(), label: "You".into(), ..Default::default() }],
            segments: vec![seg(0, 5000, None, "Prima idea."), seg(1, 9000, Some("you"), "Seconda.")],
            ..Default::default()
        };
        let lines: Vec<_> = lines_from_segments(&meta(ItemType::Note), &segs)
            .iter()
            .map(InputLine::format)
            .collect();
        assert_eq!(lines, ["Prima idea.", "You: Seconda."]);
        assert!(has_speakers(&lines_from_segments(&meta(ItemType::Note), &segs)));
    }

    #[test]
    fn body_lines_drop_the_title_heading_and_blank_lines() {
        let body = "\n# Weekly\n\n**[00:00:01] Anna:** ciao\n\n**[00:00:05] Bob:** hi\n";
        let lines: Vec<_> = lines_from_body(body).iter().map(InputLine::format).collect();
        assert_eq!(lines, ["**[00:00:01] Anna:** ciao", "**[00:00:05] Bob:** hi"]);
    }

    #[test]
    fn budget_scales_with_context_and_reserves_output() {
        // 4096 tokens: 1024 for the answer, 64 slack, 300 chars → 100 tokens of prompt.
        assert_eq!(input_budget_chars(4096, 300), (4096 - 1024 - 100 - 64) * 3);
        // Small windows keep at least 512 tokens for the answer (at most half).
        assert_eq!(input_budget_chars(1024, 0), (1024 - 512 - 64) * 3);
        // Big windows scale.
        assert!(input_budget_chars(32_768, 3000) > 60_000);
        // A huge prompt never drives the budget to zero.
        assert_eq!(input_budget_chars(1024, 100_000), 256 * 3);
    }

    #[test]
    fn chunks_respect_the_budget_and_line_boundaries() {
        let lines: Vec<String> = (0..20).map(|i| format!("[00:00:{i:02}] line number {i}")).collect();
        let budget = 100;
        let chunks = chunk_lines(&lines, budget);
        assert!(chunks.len() > 1);
        for c in &chunks {
            assert!(c.chars().count() <= budget, "{c:?}");
            // Never cut inside a line: every chunk line is a whole input line.
            assert!(c.lines().all(|l| lines.contains(&l.to_string())), "{c:?}");
        }
        // Order and content preserved.
        assert_eq!(chunks.join("\n"), lines.join("\n"));
    }

    #[test]
    fn everything_fits_in_one_chunk_when_small() {
        let lines = vec!["a".to_string(), "b".to_string()];
        assert_eq!(chunk_lines(&lines, 100), vec!["a\nb".to_string()]);
        assert!(chunk_lines(&[], 100).is_empty());
    }

    #[test]
    fn an_over_long_line_is_split_on_sentences_then_words() {
        let long = "Prima frase qui. Seconda frase più lunga di così. Terza.";
        let parts = split_long_line(long, 30);
        assert_eq!(parts[0], "Prima frase qui.");
        assert!(parts.iter().all(|p| p.chars().count() <= 30), "{parts:?}");
        assert_eq!(parts.join(" "), long);
        // No punctuation or spaces: hard cut on characters (multi-byte safe).
        let word = "è".repeat(25);
        let parts = split_long_line(&word, 10);
        assert_eq!(parts.iter().map(|p| p.chars().count()).collect::<Vec<_>>(), [10, 10, 5]);
        // Chunking uses the split too.
        let chunks = chunk_lines(&[long.to_string(), "fine".to_string()], 30);
        assert!(chunks.iter().all(|c| c.chars().count() <= 30), "{chunks:?}");
        assert!(chunks.last().unwrap().ends_with("fine"));
    }
}
