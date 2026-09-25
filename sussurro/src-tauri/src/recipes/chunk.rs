//! The transcript as recipe input (pure): one text line per segment, with
//! its timestamp and speaker when the item has them, and greedy chunking of
//! those lines into pieces that fit a character budget.
//!
//! With speakers (#143), chunks follow speaker turns: a turn (consecutive
//! lines of one speaker) that fits a chunk is never cut across two, and a
//! line too long for any chunk is split into pieces that each keep its
//! `[HH:MM:SS] Name:` prefix, so every piece a model reads says who spoke.
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
/// non-empty lines, minus the leading `# title`. Lines in the shape the app
/// renders (`**[00:01:02] Anna:** …`, `**[00:01:02]** …`, `[00:01:02] …`)
/// get their timestamp and speaker back, so a meeting edited by hand stays
/// speaker-aware (#143); anything else is kept as plain text.
pub fn lines_from_body(body: &str) -> Vec<InputLine> {
    let mut lines = body
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .peekable();
    if lines.peek().is_some_and(|l| l.starts_with("# ")) {
        lines.next();
    }
    lines.map(parse_body_line).collect()
}

/// `HH:MM:SS` (hours may grow past 99) → milliseconds.
fn parse_timestamp(ts: &str) -> Option<u64> {
    let parts: Vec<&str> = ts.split(':').collect();
    if parts.len() != 3
        || parts
            .iter()
            .any(|p| p.is_empty() || !p.bytes().all(|b| b.is_ascii_digit()))
    {
        return None;
    }
    let (h, m, s): (u64, u64, u64) = (
        parts[0].parse().ok()?,
        parts[1].parse().ok()?,
        parts[2].parse().ok()?,
    );
    (m < 60 && s < 60).then_some((h * 3600 + m * 60 + s) * 1000)
}

fn collapse(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// One body line back as an input line (see [`lines_from_body`]).
fn parse_body_line(line: &str) -> InputLine {
    let plain = || InputLine {
        text: line.to_string(),
        ..Default::default()
    };
    if let Some(rest) = line.strip_prefix("**[") {
        let Some((ts, rest)) = rest.split_once(']') else {
            return plain();
        };
        let Some(ms) = parse_timestamp(ts) else {
            return plain();
        };
        // `**[ts]** text`: a line without a speaker.
        if let Some(text) = rest.strip_prefix("**") {
            return InputLine {
                start_ms: Some(ms),
                speaker: None,
                text: collapse(text),
            };
        }
        // `**[ts] Label:** text`
        let Some((label, text)) = rest.split_once(":**") else {
            return plain();
        };
        let label = collapse(label);
        if label.is_empty() || label.contains('*') {
            return plain();
        }
        return InputLine {
            start_ms: Some(ms),
            speaker: Some(label),
            text: collapse(text),
        };
    }
    if let Some((ts, text)) = line.strip_prefix('[').and_then(|r| r.split_once("] ")) {
        if let Some(ms) = parse_timestamp(ts) {
            return InputLine {
                start_ms: Some(ms),
                speaker: None,
                text: collapse(text),
            };
        }
    }
    plain()
}

/// Whether any line names a speaker (the prompt then explains the format).
pub fn has_speakers(lines: &[InputLine]) -> bool {
    lines.iter().any(|l| l.speaker.is_some())
}

/// The speakers the lines name, each once, in order of first appearance.
pub fn speaker_names(lines: &[InputLine]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for sp in lines.iter().filter_map(|l| l.speaker.as_deref()) {
        if !out.iter().any(|o| o == sp) {
            out.push(sp.to_string());
        }
    }
    out
}

/// A speaker nobody identified: the generic "Voice N" label clustering
/// gives (`speakers::doc::voice_label`). Passed to the model as-is, with
/// the instruction not to guess who it is (#143).
pub fn is_generic_voice(label: &str) -> bool {
    label
        .trim()
        .strip_prefix("Voice ")
        .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
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
        let limit = rest
            .char_indices()
            .nth(budget)
            .map(|(i, _)| i)
            .unwrap_or(rest.len());
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

/// Pieces of `line`, formatted, of at most `budget` characters each. A
/// line over budget is split on its text (see [`split_long_line`]) and
/// every piece keeps the line's `[HH:MM:SS] Name:` prefix, so attribution
/// survives the cut. (A prefix taking half the budget or more — never on
/// real windows — falls back to cutting the formatted line.)
pub fn format_split(line: &InputLine, budget: usize) -> Vec<String> {
    let budget = budget.max(1);
    let full = line.format();
    if char_len(&full) <= budget {
        return vec![full];
    }
    let prefix = InputLine {
        text: String::new(),
        ..line.clone()
    }
    .format();
    let plen = char_len(&prefix);
    if plen == 0 || plen * 2 > budget {
        return split_long_line(&full, budget);
    }
    split_long_line(line.text.trim(), budget - plen)
        .into_iter()
        .map(|piece| format!("{prefix}{piece}"))
        .collect()
}

/// Speaker turns: runs of consecutive lines with the same speaker (lines
/// without a speaker form runs of their own).
pub fn speaker_turns(lines: &[InputLine]) -> Vec<&[InputLine]> {
    let mut turns = Vec::new();
    let mut start = 0;
    for i in 1..=lines.len() {
        if i == lines.len() || lines[i].speaker != lines[start].speaker {
            turns.push(&lines[start..i]);
            start = i;
        }
    }
    turns
}

/// Chunks being filled: formatted lines joined by `\n`.
struct Packer {
    budget: usize,
    chunks: Vec<String>,
    current: String,
    len: usize,
}

impl Packer {
    fn flush(&mut self) {
        if !self.current.is_empty() {
            self.chunks.push(std::mem::take(&mut self.current));
            self.len = 0;
        }
    }

    /// Whether `len` more characters fit the current chunk.
    fn fits(&self, len: usize) -> bool {
        self.current.is_empty() || self.len + 1 + len <= self.budget
    }

    fn push(&mut self, piece: &str) {
        let len = char_len(piece);
        if !self.fits(len) {
            self.flush();
        }
        if !self.current.is_empty() {
            self.current.push('\n');
            self.len += 1;
        }
        self.current.push_str(piece);
        self.len += len;
    }
}

/// Pack input lines, in order, into chunks of at most `budget` characters
/// (formatted lines joined by `\n`), on speaker-turn boundaries: a turn
/// that fits a chunk starts a new chunk rather than being cut across two;
/// only a turn longer than a whole chunk is split, between its lines (and
/// an over-long line with [`format_split`], each piece keeping its
/// speaker). Without speakers this is [`chunk_lines`] on the formatted
/// lines. Every character of every line ends up in exactly one chunk, in
/// the original order.
pub fn chunk_turns(lines: &[InputLine], budget: usize) -> Vec<String> {
    let budget = budget.max(1);
    let mut p = Packer {
        budget,
        chunks: Vec::new(),
        current: String::new(),
        len: 0,
    };
    for turn in speaker_turns(lines) {
        let pieces: Vec<String> = turn.iter().flat_map(|l| format_split(l, budget)).collect();
        let turn_len =
            pieces.iter().map(|x| char_len(x)).sum::<usize>() + pieces.len().saturating_sub(1);
        // A turn that won't fit what is left of this chunk but fits one of
        // its own is kept together. (Lines without a speaker are no turn.)
        if turn[0].speaker.is_some() && !p.fits(turn_len) && turn_len <= budget {
            p.flush();
        }
        for piece in &pieces {
            p.push(piece);
        }
    }
    p.flush();
    p.chunks
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
        ItemMeta {
            item_type: t,
            title: "T".into(),
            ..Default::default()
        }
    }

    #[test]
    fn meeting_lines_carry_timestamps_and_speaker_labels() {
        let segs = SegmentsFile {
            speakers: vec![DocSpeaker {
                id: "meet:anna".into(),
                label: "Anna  Rossi".into(),
                ..Default::default()
            }],
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
        assert_eq!(
            lines,
            [
                "[01:02:03] Anna Rossi: Partiamo dalla 0.7.",
                "[01:02:10] Ok."
            ]
        );
    }

    #[test]
    fn note_lines_are_plain_text_but_keep_speakers() {
        let segs = SegmentsFile {
            speakers: vec![DocSpeaker {
                id: "you".into(),
                label: "You".into(),
                ..Default::default()
            }],
            segments: vec![
                seg(0, 5000, None, "Prima idea."),
                seg(1, 9000, Some("you"), "Seconda."),
            ],
            ..Default::default()
        };
        let lines: Vec<_> = lines_from_segments(&meta(ItemType::Note), &segs)
            .iter()
            .map(InputLine::format)
            .collect();
        assert_eq!(lines, ["Prima idea.", "You: Seconda."]);
        assert!(has_speakers(&lines_from_segments(
            &meta(ItemType::Note),
            &segs
        )));
    }

    #[test]
    fn body_lines_drop_the_title_heading_and_blank_lines() {
        let body = "\n# Weekly\n\n**[00:00:01] Anna:** ciao\n\n**[00:00:05] Bob:** hi\n";
        let lines: Vec<_> = lines_from_body(body)
            .iter()
            .map(InputLine::format)
            .collect();
        assert_eq!(lines, ["[00:00:01] Anna: ciao", "[00:00:05] Bob: hi"]);
    }

    #[test]
    fn body_lines_get_their_speakers_and_timestamps_back() {
        let body = "# T\n**[01:02:03] Voice 2:** Buongiorno  a tutti.\n**[00:00:09]** senza voce\n\
                    [00:00:10] plain timed\nJust text: with a colon\n**[xx:00:01] Anna:** bad ts\n\
                    **[00:00:11] **Bold**:** odd\n[00:61:00] bad minutes";
        let lines = lines_from_body(body);
        assert_eq!(
            lines[0],
            InputLine {
                start_ms: Some(3_723_000),
                speaker: Some("Voice 2".into()),
                text: "Buongiorno a tutti.".into()
            }
        );
        assert_eq!(
            lines[1],
            InputLine {
                start_ms: Some(9_000),
                speaker: None,
                text: "senza voce".into()
            }
        );
        assert_eq!(
            lines[2],
            InputLine {
                start_ms: Some(10_000),
                speaker: None,
                text: "plain timed".into()
            }
        );
        // Anything else stays plain text, untouched.
        let rest = [
            "Just text: with a colon",
            "**[xx:00:01] Anna:** bad ts",
            "**[00:00:11] **Bold**:** odd",
            "[00:61:00] bad minutes",
        ];
        for (i, raw) in rest.iter().enumerate() {
            assert_eq!(
                lines[3 + i],
                InputLine {
                    text: raw.to_string(),
                    ..Default::default()
                },
                "{raw}"
            );
        }
        assert_eq!(speaker_names(&lines), ["Voice 2"]);
    }

    #[test]
    fn speaker_names_in_order_and_generic_voices() {
        let l = |sp: Option<&str>| InputLine {
            speaker: sp.map(str::to_string),
            text: "x".into(),
            ..Default::default()
        };
        let lines = [
            l(Some("Anna")),
            l(None),
            l(Some("Voice 1")),
            l(Some("Anna")),
            l(Some("Bob")),
        ];
        assert_eq!(speaker_names(&lines), ["Anna", "Voice 1", "Bob"]);
        assert!(is_generic_voice("Voice 1") && is_generic_voice(" Voice 12 "));
        for named in [
            "Anna",
            "Voice",
            "Voice one",
            "Voice 2b",
            "voice 2",
            "My Voice 2",
        ] {
            assert!(!is_generic_voice(named), "{named}");
        }
    }

    fn turn_line(sec: u64, sp: &str, text: &str) -> InputLine {
        InputLine {
            start_ms: Some(sec * 1000),
            speaker: Some(sp.into()),
            text: text.into(),
        }
    }

    #[test]
    fn turns_group_consecutive_lines_of_one_speaker() {
        let lines = [
            turn_line(1, "Anna", "a"),
            turn_line(2, "Anna", "b"),
            turn_line(3, "Bob", "c"),
            InputLine {
                text: "d".into(),
                ..Default::default()
            },
            turn_line(5, "Anna", "e"),
        ];
        let sizes: Vec<_> = speaker_turns(&lines).iter().map(|t| t.len()).collect();
        assert_eq!(sizes, [2, 1, 1, 1]);
        assert!(speaker_turns(&[]).is_empty());
    }

    #[test]
    fn chunks_follow_speaker_turns() {
        // Turns of three lines, alternating speakers.
        let mut lines = Vec::new();
        for t in 0..12u64 {
            let sp = ["Anna", "Bob", "Voice 1"][t as usize % 3];
            for k in 0..3 {
                lines.push(turn_line(t * 10 + k, sp, &format!("turn {t} line {k}")));
            }
        }
        let budget = 150;
        let chunks = chunk_turns(&lines, budget);
        assert!(chunks.len() > 2);
        let formatted: Vec<String> = lines.iter().map(InputLine::format).collect();
        // Order and content preserved, budget respected.
        assert_eq!(chunks.join("\n"), formatted.join("\n"));
        assert!(chunks.iter().all(|c| c.chars().count() <= budget));
        // No turn is cut: every chunk starts with the first line of a turn.
        for c in &chunks {
            assert!(
                c.lines().next().unwrap().ends_with("line 0"),
                "chunk starts mid-turn: {c:?}"
            );
        }
        // Plain line packing would have cut turns at this budget.
        let plain = chunk_lines(&formatted, budget);
        assert!(plain
            .iter()
            .any(|c| !c.lines().next().unwrap().ends_with("line 0")));
    }

    #[test]
    fn a_turn_over_budget_is_split_and_every_piece_keeps_its_speaker() {
        let long = "Questa è una frase lunga. ".repeat(12);
        let lines = [
            turn_line(1, "Anna", "breve"),
            turn_line(2, "Voice 2", long.trim()),
            turn_line(3, "Voice 2", "coda"),
            turn_line(4, "Bob", "fine"),
        ];
        let budget = 120;
        let chunks = chunk_turns(&lines, budget);
        assert!(
            chunks.iter().all(|c| c.chars().count() <= budget),
            "{chunks:?}"
        );
        let all: Vec<&str> = chunks.iter().flat_map(|c| c.lines()).collect();
        assert!(all.iter().all(|l| l.starts_with("[00:00:0")), "{all:?}");
        let voice: Vec<&str> = all
            .iter()
            .copied()
            .filter(|l| l.contains("frase"))
            .collect();
        assert!(voice.len() > 1, "the long line was split");
        assert!(
            voice.iter().all(|l| l.starts_with("[00:00:02] Voice 2: ")),
            "{voice:?}"
        );
        // The text survives the split.
        let text: Vec<&str> = voice
            .iter()
            .map(|l| l.trim_start_matches("[00:00:02] Voice 2: "))
            .collect();
        assert_eq!(text.join(" "), long.trim());
        // Without speakers it is plain line packing.
        let plain: Vec<InputLine> = (0..20)
            .map(|i| InputLine {
                text: format!("line {i}"),
                ..Default::default()
            })
            .collect();
        let formatted: Vec<String> = plain.iter().map(InputLine::format).collect();
        assert_eq!(chunk_turns(&plain, 40), chunk_lines(&formatted, 40));
    }

    #[test]
    fn format_split_keeps_short_lines_whole() {
        let l = turn_line(61, "Anna", "ciao");
        assert_eq!(format_split(&l, 100), ["[00:01:01] Anna: ciao"]);
        // A prefix longer than half the budget: plain cut.
        let pieces = format_split(&l, 20);
        assert!(pieces.iter().all(|p| p.chars().count() <= 20));
        assert_eq!(pieces.join(" "), "[00:01:01] Anna: ciao");
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
        let lines: Vec<String> = (0..20)
            .map(|i| format!("[00:00:{i:02}] line number {i}"))
            .collect();
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
        assert_eq!(
            parts.iter().map(|p| p.chars().count()).collect::<Vec<_>>(),
            [10, 10, 5]
        );
        // Chunking uses the split too.
        let chunks = chunk_lines(&[long.to_string(), "fine".to_string()], 30);
        assert!(chunks.iter().all(|c| c.chars().count() <= 30), "{chunks:?}");
        assert!(chunks.last().unwrap().ends_with("fine"));
    }
}
