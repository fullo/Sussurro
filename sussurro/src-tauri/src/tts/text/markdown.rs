//! Markdown → blocks of plain text, each with the pause that follows it
//! (pure). A line-based reader for what archive items contain — the
//! CommonMark subset people write by hand plus the app's own transcript
//! lines — not a full CommonMark parser:
//!
//! - frontmatter, fenced and indented code blocks, HTML comments, link
//!   reference definitions and footnote markers are dropped;
//! - ATX (`## Title`) and setext headings become a block followed by a
//!   section pause, and the block before a heading also gets one;
//! - paragraphs join their lines; list items (`-`, `*`, `+`, `1.`, `1)`,
//!   task boxes) and block quotes lose their markers;
//! - tables are read row by row with their headers, or skipped;
//! - transcript lines (`**[00:01:02] Anna:** text`, `**[00:01:02]** text`,
//!   `[00:01:02] text`) lose the timestamp; the speaker name is kept when it
//!   changes, or dropped;
//! - inline: emphasis and strike-through marks, code-span backticks (the
//!   code is read), link targets (the text is read), images (the alt text
//!   is read), autolink brackets, HTML tags, escapes and the common
//!   entities.
//!
//! Headings, list items and table rows get a final full stop when they
//! have no closing punctuation, so the engine ends them with a falling
//! intonation.

use super::{Pause, PrepOptions, SpeakerMode, TableMode};
use regex::Regex;
use std::sync::LazyLock;

/// A block of plain text (inline markup removed, not yet normalised).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Block {
    pub text: String,
    pub pause: Pause,
}

static ATX_HEADING: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^#{1,6}(?:[ \t]+(.*?))?[ \t]*#*[ \t]*$").expect("valid regex"));
static FENCE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[ ]{0,3}(`{3,}|~{3,})").expect("valid regex"));
static LIST_ITEM: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^[ \t]*(?:[-*+]|\d{1,9}[.)])[ \t]+(?:\[[ xX]\][ \t]+)?(.*)$").expect("valid regex")
});
static THEMATIC_BREAK: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^[ ]{0,3}(?:(?:-[ \t]*){3,}|(?:\*[ \t]*){3,}|(?:_[ \t]*){3,})$")
        .expect("valid regex")
});
static SETEXT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[ ]{0,3}(?:=+|-+)[ \t]*$").expect("valid regex"));
static TABLE_SEPARATOR: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^[ \t]*\|?[ \t]*:?-+:?[ \t]*(?:\|[ \t]*:?-+:?[ \t]*)*\|?[ \t]*$")
        .expect("valid regex")
});
static LINK_DEFINITION: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[ ]{0,3}\[[^\]]+\]:[ \t]*\S+").expect("valid regex"));
/// `**[00:01:02] Anna:** text` / `**[00:01:02]** text`.
static TRANSCRIPT_BOLD: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\*\*\[\d{1,3}:\d{2}(?::\d{2})?\](?:[ \t]+(.+?):)?\*\*[ \t]*(.*)$")
        .expect("valid regex")
});
/// `[00:01:02] text` / `[00:01:02] Anna: text` (hand-written).
static TRANSCRIPT_PLAIN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\[\d{1,3}:\d{2}(?::\d{2})?\][ \t]*(.*)$").expect("valid regex"));
static HEADING_NUMBER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\s*(?:\d+(?:\.\d+)*[.)]|\d+(?:\.\d+)+)\s+").expect("valid regex")
});
static HTML_COMMENT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?s)<!--.*?-->").expect("valid regex"));

/// The document's blocks, in reading order.
pub fn blocks(doc: &str, opts: &PrepOptions) -> Vec<Block> {
    let body = crate::archive::frontmatter::split(doc)
        .map(|(_, body)| body)
        .unwrap_or(doc);
    let body = body.strip_prefix('\u{feff}').unwrap_or(body);
    let body = HTML_COMMENT.replace_all(body, "");
    Reader::new(opts).read(&body)
}

struct Reader<'o> {
    opts: &'o PrepOptions,
    out: Vec<Block>,
    paragraph: Vec<String>,
    /// Kind of the last block pushed, for indented-code detection.
    last_was_list: bool,
    last_speaker: Option<String>,
}

impl<'o> Reader<'o> {
    fn new(opts: &'o PrepOptions) -> Self {
        Reader {
            opts,
            out: Vec::new(),
            paragraph: Vec::new(),
            last_was_list: false,
            last_speaker: None,
        }
    }

    fn read(mut self, body: &str) -> Vec<Block> {
        let lines: Vec<String> = body
            .lines()
            .map(|l| strip_quote_markers(l.trim_end_matches('\r')))
            .collect();
        let mut i = 0;
        while i < lines.len() {
            let line = lines[i].as_str();
            let trimmed = line.trim();

            if trimmed.is_empty() {
                self.flush_paragraph();
                i += 1;
                continue;
            }
            // Fenced code: skip to the closing fence (or the end).
            if let Some(m) = FENCE.captures(line) {
                self.flush_paragraph();
                let fence = &m[1];
                let (ch, len) = (fence.chars().next().unwrap_or('`'), fence.len());
                i += 1;
                while i < lines.len() {
                    let t = lines[i].trim();
                    if t.len() >= len && t.chars().all(|c| c == ch) {
                        break;
                    }
                    i += 1;
                }
                i += 1;
                self.last_was_list = false;
                continue;
            }
            // Indented code: only when not continuing a paragraph or list.
            if self.paragraph.is_empty()
                && !self.last_was_list
                && (line.starts_with("    ") || line.starts_with('\t'))
            {
                i += 1;
                continue;
            }
            // Setext heading: the paragraph so far is the title.
            if !self.paragraph.is_empty() && SETEXT.is_match(line) {
                let title = std::mem::take(&mut self.paragraph).join(" ");
                self.push_heading(&title);
                i += 1;
                continue;
            }
            if THEMATIC_BREAK.is_match(line) {
                self.flush_paragraph();
                self.raise_last_pause(Pause::Section);
                i += 1;
                continue;
            }
            if let Some(m) = ATX_HEADING.captures(trimmed) {
                self.flush_paragraph();
                self.push_heading(m.get(1).map_or("", |g| g.as_str()));
                i += 1;
                continue;
            }
            if LINK_DEFINITION.is_match(line) {
                self.flush_paragraph();
                i += 1;
                continue;
            }
            // Table: a row of cells followed by a separator row.
            if trimmed.contains('|')
                && lines
                    .get(i + 1)
                    .is_some_and(|n| TABLE_SEPARATOR.is_match(n))
                && lines[i + 1].contains('-')
            {
                self.flush_paragraph();
                let header = split_cells(trimmed);
                let mut rows = Vec::new();
                i += 2;
                while i < lines.len() && lines[i].contains('|') && !lines[i].trim().is_empty() {
                    rows.push(split_cells(lines[i].trim()));
                    i += 1;
                }
                self.push_table(&header, &rows);
                continue;
            }
            if let Some((speaker, text)) = transcript_line(trimmed) {
                self.flush_paragraph();
                self.push_transcript_line(speaker, &text);
                i += 1;
                continue;
            }
            if let Some(m) = LIST_ITEM.captures(line) {
                self.flush_paragraph();
                let mut item = vec![m[1].to_string()];
                i += 1;
                // Continuation lines: indented, not a new item or block.
                while i < lines.len() {
                    let next = lines[i].as_str();
                    if next.trim().is_empty()
                        || LIST_ITEM.is_match(next)
                        || !(next.starts_with(' ') || next.starts_with('\t'))
                    {
                        break;
                    }
                    item.push(next.trim().to_string());
                    i += 1;
                }
                self.push_line_block(&item.join(" "), true);
                continue;
            }
            self.paragraph.push(trimmed.to_string());
            i += 1;
        }
        self.flush_paragraph();
        self.out
    }

    fn push(&mut self, text: String, pause: Pause, list: bool) {
        let text = collapse_ws(&text);
        if text.is_empty() {
            return;
        }
        self.out.push(Block { text, pause });
        self.last_was_list = list;
    }

    fn raise_last_pause(&mut self, pause: Pause) {
        if let Some(last) = self.out.last_mut() {
            last.pause = last.pause.max(pause);
        }
    }

    fn flush_paragraph(&mut self) {
        if self.paragraph.is_empty() {
            return;
        }
        let text = std::mem::take(&mut self.paragraph).join(" ");
        self.last_speaker = None;
        self.push(inline(&text), Pause::Paragraph, false);
    }

    fn push_heading(&mut self, title: &str) {
        let text = inline(title);
        // Section numbers (`1.`, `2.3`, `4)`) are not read; a heading that
        // starts with a year (`2025 in review`) keeps it.
        let text = match HEADING_NUMBER.find(&text) {
            Some(m) if m.end() < text.len() => text[m.end()..].to_string(),
            _ => text,
        };
        if collapse_ws(&text).is_empty() {
            return;
        }
        self.raise_last_pause(Pause::Section);
        self.last_speaker = None;
        self.push(end_with_stop(&text), Pause::Section, false);
    }

    /// List items: their own block, a line pause after.
    fn push_line_block(&mut self, raw: &str, list: bool) {
        self.last_speaker = None;
        let text = inline(raw);
        self.push(end_with_stop(&text), Pause::Line, list);
    }

    fn push_table(&mut self, header: &[String], rows: &[Vec<String>]) {
        if self.opts.tables == TableMode::Skip {
            return;
        }
        self.last_speaker = None;
        let header: Vec<String> = header.iter().map(|h| collapse_ws(&inline(h))).collect();
        if rows.is_empty() {
            let text = header
                .iter()
                .filter(|h| !h.is_empty())
                .cloned()
                .collect::<Vec<_>>()
                .join(", ");
            self.push(end_with_stop(&text), Pause::Paragraph, false);
            return;
        }
        for row in rows {
            let cells: Vec<String> = row
                .iter()
                .enumerate()
                .filter_map(|(c, cell)| {
                    let cell = collapse_ws(&inline(cell));
                    if cell.is_empty() {
                        return None;
                    }
                    match header.get(c).filter(|h| !h.is_empty()) {
                        Some(h) => Some(format!("{h}: {cell}")),
                        None => Some(cell),
                    }
                })
                .collect();
            self.push(end_with_stop(&cells.join(", ")), Pause::Line, false);
        }
        self.raise_last_pause(Pause::Paragraph);
    }

    fn push_transcript_line(&mut self, speaker: Option<String>, text: &str) {
        let text = inline(text);
        if collapse_ws(&text).is_empty() {
            return;
        }
        let changed = speaker != self.last_speaker;
        if changed {
            // A new voice: the previous line ends with a longer pause.
            self.raise_last_pause(Pause::Paragraph);
        }
        let line = match (&speaker, self.opts.speakers) {
            (Some(name), SpeakerMode::Name) if changed => format!("{name}: {text}"),
            _ => text,
        };
        self.last_speaker = speaker;
        self.push(line, Pause::Line, false);
    }
}

/// `(speaker, text)` of a transcript line, or `None` for any other line.
fn transcript_line(line: &str) -> Option<(Option<String>, String)> {
    if let Some(m) = TRANSCRIPT_BOLD.captures(line) {
        let speaker = m
            .get(1)
            .map(|s| collapse_ws(s.as_str()))
            .filter(|s| !s.is_empty());
        return Some((speaker, m[2].to_string()));
    }
    TRANSCRIPT_PLAIN
        .captures(line)
        .map(|m| (None, m[1].to_string()))
}

/// Drop leading `>` block-quote markers.
fn strip_quote_markers(line: &str) -> String {
    let mut rest = line;
    loop {
        let t = rest.trim_start_matches([' ', '\t']);
        match t.strip_prefix('>') {
            Some(after) if rest.len() - t.len() <= 3 => {
                rest = after.strip_prefix(' ').unwrap_or(after);
            }
            _ => return rest.to_string(),
        }
    }
}

/// Cells of a table row; `\|` is a literal pipe.
fn split_cells(row: &str) -> Vec<String> {
    let row = row.trim();
    let row = row.strip_prefix('|').unwrap_or(row);
    let row = if row.ends_with('|') && !row.ends_with("\\|") {
        &row[..row.len() - 1]
    } else {
        row
    };
    let mut cells = Vec::new();
    let mut cur = String::new();
    let mut chars = row.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' if chars.peek() == Some(&'|') => {
                cur.push('|');
                chars.next();
            }
            '|' => cells.push(std::mem::take(&mut cur).trim().to_string()),
            c => cur.push(c),
        }
    }
    cells.push(cur.trim().to_string());
    cells
}

fn collapse_ws(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Add a full stop when the text has no closing punctuation.
fn end_with_stop(text: &str) -> String {
    let t = collapse_ws(text);
    let last = t
        .trim_end_matches(['"', '\'', '”', '’', '»', ')', ']'])
        .chars()
        .last();
    match last {
        None => t,
        Some('.' | '!' | '?' | '…' | ':' | ';') => t,
        Some(',') => format!("{}.", &t[..t.len() - 1]),
        Some(_) => format!("{t}."),
    }
}

static CODE_SPAN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(`+)(.+?)(`+)").expect("valid regex"));
static IMAGE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"!\[([^\]]*)\]\([^)]*\)").expect("valid regex"));
static LINK_INLINE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\[([^\]]+)\]\((?:[^()]|\([^)]*\))*\)").expect("valid regex"));
static LINK_REF: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\[([^\]]+)\]\[[^\]]*\]").expect("valid regex"));
static FOOTNOTE_REF: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\[\^[^\]]+\]").expect("valid regex"));
static AUTOLINK: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"<((?:https?|ftp)://[^>\s]+|[^@>\s]+@[^>\s]+)>").expect("valid regex")
});
static HTML_TAG: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"</?[A-Za-z][A-Za-z0-9-]*(?:\s[^<>]*)?/?>").expect("valid regex"));
static EMPH_OPEN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(^|[\s\p{P}])(?:[*_]+|~~)(\S)").expect("valid regex"));
static EMPH_CLOSE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(\S)(?:[*_]+|~~)($|[\s\p{P}])").expect("valid regex"));

/// Private-use stand-ins for escaped markdown characters, restored at the
/// end so the escape survives the markup removal.
const ESCAPE_BASE: u32 = 0xF0000;
const ESCAPABLE: &str = "\\`*_{}[]()#+-.!|~<>";

/// Inline markup → plain text.
pub(super) fn inline(s: &str) -> String {
    // Escapes first, so `\*` is never emphasis.
    let mut protected = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' {
            if let Some(&n) = chars.peek() {
                if let Some(pos) = ESCAPABLE.find(n) {
                    chars.next();
                    protected.push(char::from_u32(ESCAPE_BASE + pos as u32).unwrap_or(n));
                    continue;
                }
            }
        }
        protected.push(c);
    }
    let mut t = CODE_SPAN
        .replace_all(&protected, |c: &regex::Captures| {
            if c[1].len() == c[3].len() {
                c[2].trim().to_string()
            } else {
                c[0].to_string()
            }
        })
        .into_owned();
    t = IMAGE.replace_all(&t, "$1").into_owned();
    t = LINK_INLINE.replace_all(&t, "$1").into_owned();
    t = LINK_REF.replace_all(&t, "$1").into_owned();
    t = FOOTNOTE_REF.replace_all(&t, "").into_owned();
    t = AUTOLINK.replace_all(&t, "$1").into_owned();
    t = HTML_TAG.replace_all(&t, " ").into_owned();
    // Emphasis marks: twice, since one pass can't see a mark whose
    // neighbour was another mark (`***x***`).
    for _ in 0..2 {
        t = EMPH_OPEN.replace_all(&t, "$1$2").into_owned();
        t = EMPH_CLOSE.replace_all(&t, "$1$2").into_owned();
    }
    let t = decode_entities(&t);
    t.chars()
        .map(|c| {
            let code = c as u32;
            if (ESCAPE_BASE..ESCAPE_BASE + ESCAPABLE.len() as u32).contains(&code) {
                ESCAPABLE
                    .chars()
                    .nth((code - ESCAPE_BASE) as usize)
                    .unwrap_or(c)
            } else {
                c
            }
        })
        .collect()
}

fn decode_entities(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = s.to_string();
    for (from, to) in [
        ("&nbsp;", " "),
        ("&lt;", "<"),
        ("&gt;", ">"),
        ("&quot;", "\""),
        ("&#39;", "'"),
        ("&apos;", "'"),
        ("&rsquo;", "’"),
        ("&lsquo;", "‘"),
        ("&ldquo;", "“"),
        ("&rdquo;", "”"),
        ("&laquo;", "«"),
        ("&raquo;", "»"),
        ("&hellip;", "…"),
        ("&mdash;", "—"),
        ("&ndash;", "–"),
        ("&euro;", "€"),
        ("&amp;", "&"),
    ] {
        out = out.replace(from, to);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(doc: &str) -> Vec<String> {
        blocks(doc, &PrepOptions::default())
            .into_iter()
            .map(|b| b.text)
            .collect()
    }

    fn with(doc: &str, opts: PrepOptions) -> Vec<(String, Pause)> {
        blocks(doc, &opts)
            .into_iter()
            .map(|b| (b.text, b.pause))
            .collect()
    }

    #[test]
    fn frontmatter_is_skipped() {
        let doc = "---\ntitle: Test\nlanguage: it\n---\n# Test\n\nCiao.\n";
        assert_eq!(texts(doc), ["Test.", "Ciao."]);
    }

    #[test]
    fn headings_get_a_section_pause_before_and_after() {
        let doc = "Intro text.\n\n## Second part ##\n\nBody.\n\nSetext\n======\n\nMore.";
        assert_eq!(
            with(doc, PrepOptions::default()),
            [
                ("Intro text.".to_string(), Pause::Section),
                ("Second part.".to_string(), Pause::Section),
                ("Body.".to_string(), Pause::Section),
                ("Setext.".to_string(), Pause::Section),
                ("More.".to_string(), Pause::Paragraph),
            ]
        );
    }

    #[test]
    fn heading_section_numbers_are_not_read() {
        assert_eq!(
            texts("# 1. Introduzione\n\n## 2.3 Metodo\n\n## 4) Fine\n\n# 2025 in review\n\n# 3."),
            ["Introduzione.", "Metodo.", "Fine.", "2025 in review.", "3."]
        );
    }

    #[test]
    fn heading_keeps_its_own_question_mark() {
        assert_eq!(texts("# Why now?"), ["Why now?"]);
    }

    #[test]
    fn paragraph_lines_are_joined() {
        assert_eq!(
            texts("one line\nsecond line\n\nnext"),
            ["one line second line", "next"]
        );
    }

    #[test]
    fn thematic_break_is_a_section_pause() {
        assert_eq!(
            with("a\n\n---\n\nb", PrepOptions::default()),
            [
                ("a".to_string(), Pause::Section),
                ("b".to_string(), Pause::Paragraph)
            ]
        );
    }

    #[test]
    fn code_blocks_are_skipped() {
        let doc =
            "Before.\n\n```rust\nfn main() {}\n```\n\n~~~~\nx\n~~~~\n\n    indented code\n\nAfter.";
        assert_eq!(texts(doc), ["Before.", "After."]);
    }

    #[test]
    fn unclosed_fence_skips_to_the_end() {
        assert_eq!(texts("Text.\n\n```\ncode\nmore"), ["Text."]);
    }

    #[test]
    fn inline_code_is_read_as_written() {
        assert_eq!(
            texts("Open `settings.json` now."),
            ["Open settings.json now."]
        );
    }

    #[test]
    fn lists_lose_their_markers() {
        let doc = "- first item\n* second, with more\n  continued here\n+ third!\n1. numbered\n2) other\n- [x] done task\n- [ ] open task";
        assert_eq!(
            texts(doc),
            [
                "first item.",
                "second, with more continued here.",
                "third!",
                "numbered.",
                "other.",
                "done task.",
                "open task."
            ]
        );
        assert!(blocks(doc, &PrepOptions::default())
            .iter()
            .all(|b| b.pause == Pause::Line));
    }

    #[test]
    fn indented_line_after_a_list_is_not_code() {
        assert_eq!(
            texts("- item\n\n    more of the item"),
            ["item.", "more of the item"]
        );
    }

    #[test]
    fn links_images_and_emphasis() {
        let doc = "Read **the [guide](https://example.com/a_(b))** and *this* ![a chart](c.png) \
                   or ![](x.png) __now__ ~~old~~ [ref][1][^2].\n\n[1]: https://example.com";
        assert_eq!(
            texts(doc),
            ["Read the guide and this a chart or now old ref."]
        );
    }

    #[test]
    fn intraword_underscores_and_escapes_stay() {
        assert_eq!(
            texts(r"snake_case and 2\*3 and \_x\_"),
            ["snake_case and 2*3 and _x_"]
        );
    }

    #[test]
    fn autolinks_html_and_entities() {
        let doc = "See <https://example.com> or <anna@example.com>. <br/>A&amp;B &laquo;ok&raquo;\n<!-- hidden\ncomment -->";
        assert_eq!(
            texts(doc),
            ["See https://example.com or anna@example.com. A&B «ok»"]
        );
    }

    #[test]
    fn block_quotes_are_read() {
        assert_eq!(
            texts("> quoted\n> text\n>\n> - item"),
            ["quoted text", "item."]
        );
    }

    #[test]
    fn tables_are_read_row_by_row_with_headers() {
        let doc = "| Name | Role | Note |\n|:-----|:----:|-----:|\n| Anna | host | |\n| Marco | guest | late |\n\nAfter.";
        assert_eq!(
            with(doc, PrepOptions::default()),
            [
                ("Name: Anna, Role: host.".to_string(), Pause::Line),
                (
                    "Name: Marco, Role: guest, Note: late.".to_string(),
                    Pause::Paragraph
                ),
                ("After.".to_string(), Pause::Paragraph),
            ]
        );
    }

    #[test]
    fn tables_can_be_skipped() {
        let doc = "Before.\n\n| a | b |\n|---|---|\n| 1 | 2 |\n\nAfter.";
        let opts = PrepOptions {
            tables: TableMode::Skip,
            ..Default::default()
        };
        assert_eq!(
            with(doc, opts)
                .into_iter()
                .map(|(t, _)| t)
                .collect::<Vec<_>>(),
            ["Before.", "After."]
        );
    }

    #[test]
    fn header_only_table_and_escaped_pipes() {
        assert_eq!(texts("a | b\n--|--"), ["a, b."]);
        assert_eq!(
            texts("| x | y |\n|---|---|\n| a \\| b | c |"),
            ["x: a | b, y: c."]
        );
    }

    #[test]
    fn a_pipe_in_prose_is_not_a_table() {
        assert_eq!(texts("either a | b\nor not"), ["either a | b or not"]);
    }

    #[test]
    fn transcript_lines_name_the_speaker_when_it_changes() {
        let doc = "# Meeting\n\n**[00:00:01] Anna:** Hello.\n\n**[00:00:04] Anna:** Second line.\n\n**[00:00:09] Marco:** Hi.\n\n**[00:00:12]** Nobody.\n\n[00:00:15] Plain line.";
        assert_eq!(
            with(doc, PrepOptions::default()),
            [
                ("Meeting.".to_string(), Pause::Section),
                ("Anna: Hello.".to_string(), Pause::Line),
                ("Second line.".to_string(), Pause::Paragraph),
                ("Marco: Hi.".to_string(), Pause::Paragraph),
                ("Nobody.".to_string(), Pause::Line),
                ("Plain line.".to_string(), Pause::Line),
            ]
        );
    }

    #[test]
    fn transcript_speakers_can_be_dropped() {
        let doc = "**[00:00:01] Anna:** Hello.\n\n**[00:00:09] Voice 2:** Hi.";
        let opts = PrepOptions {
            speakers: SpeakerMode::Drop,
            ..Default::default()
        };
        assert_eq!(
            with(doc, opts),
            [
                ("Hello.".to_string(), Pause::Paragraph),
                ("Hi.".to_string(), Pause::Line),
            ]
        );
    }

    #[test]
    fn empty_and_markup_only_documents_give_nothing() {
        assert!(texts("").is_empty());
        assert!(texts("---\ntitle: x\n---\n").is_empty());
        assert!(texts("#\n\n---\n\n```\ncode\n```\n![](a.png)").is_empty());
    }

    #[test]
    fn crlf_and_bom() {
        assert_eq!(
            texts("\u{feff}# Title\r\n\r\nText.\r\n"),
            ["Title.", "Text."]
        );
    }
}
