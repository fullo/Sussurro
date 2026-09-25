//! Text preparation for read-aloud (0.12, #254, plan §4.5): an archive
//! item's markdown becomes speakable text, cut into sentence-bounded chunks
//! for the engine. Pure, deterministic, engine-independent.
//!
//! Three stages, each its own module:
//!
//! 1. [`markdown`] — markdown → blocks of plain text. Frontmatter, code
//!    blocks, HTML comments, link targets, images without alt text and
//!    footnote markers are dropped; headings, paragraphs, list items, table
//!    rows and transcript lines become blocks, each with the pause that
//!    follows it (a heading gets the longest). Tables are read row by row
//!    with their headers ("Name: Anna, Role: host.") or skipped
//!    ([`TableMode`]); transcript lines `**[00:01:02] Anna:** text` are read
//!    as "Anna: text" (the name only when the speaker changes) or without
//!    the name ([`SpeakerMode`]); the timestamp is never read.
//! 2. [`normalize`] — per-language expansion (Italian and English):
//!    numbers, decimals, years (English), ordinals, Roman numerals in
//!    context, dates, times, percentages, currencies, phone numbers, flight
//!    numbers and booking codes, units, ranges, common abbreviations and
//!    titles, URLs and email addresses, symbols. Any other language passes
//!    through with markup, emoji and URLs cleaned only.
//! 3. [`chunk`] — sentences packed greedily into chunks of at most
//!    [`PrepOptions::max_chunk_chars`] characters, never across blocks;
//!    an over-long sentence is cut at a clause (`,` `;` `:`), else at a
//!    space. The same input always gives the same chunks, so a chunk index
//!    is a stable address for streaming and per-chunk marking.
//!
//! ## Default chunk length: [`DEFAULT_MAX_CHUNK_CHARS`] = 160
//!
//! P18 picked Pocket TTS. Pocket conditions each generation on at most 50
//! tokens of text (`MAX_TOKEN_PER_CHUNK` in Kyutai's reference code, which
//! re-splits longer input itself) and caps a chunk's audio at
//! `(tokens / 3 + 2)` seconds. Its SentencePiece vocabulary has only 4,000
//! pieces, so Italian and English prose runs at roughly 3.2–3.5 characters
//! per token: 50 tokens ≈ 160–175 characters. 160 keeps a normal chunk
//! inside one Pocket generation, so the engine never re-splits a sentence
//! at a place we didn't choose, while still packing two or three short
//! sentences together for natural prosody. The engine (#255) should still
//! count tokens and may pass a smaller limit; a limit below
//! [`MIN_MAX_CHUNK_CHARS`] is raised to it.
//!
//! What it does not do: guess Italian gender for "1" before a noun (read
//! "uno"), expand every abbreviation (only the common ones listed in
//! [`normalize`]), or read file names and code; code blocks are skipped
//! and inline code is read as written.

mod chunk;
mod markdown;
mod normalize;
mod numbers;

pub use chunk::split_sentences;

/// Default hard limit on a chunk's length in characters (see the module
/// docs for why 160).
pub const DEFAULT_MAX_CHUNK_CHARS: usize = 160;
/// Smallest chunk limit accepted: below this a normal clause no longer
/// fits and sentences would be cut between words.
pub const MIN_MAX_CHUNK_CHARS: usize = 40;

/// Language of the text, from the item's `language` frontmatter code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    Italian,
    English,
    /// Anything else (or unknown): markdown is stripped and text cleaned,
    /// but numbers and abbreviations are left as written.
    Other,
}

impl Lang {
    /// `it`, `it-IT`, `ita`, `italian`, `Italiano` → Italian; `en`,
    /// `en-GB`, `eng`, `english` → English; anything else → Other.
    pub fn from_code(code: &str) -> Lang {
        let c = code.trim().to_ascii_lowercase();
        let base = c.split(['-', '_']).next().unwrap_or("");
        match base {
            "it" | "ita" | "italian" | "italiano" => Lang::Italian,
            "en" | "eng" | "english" | "inglese" => Lang::English,
            _ => Lang::Other,
        }
    }
}

/// How tables are read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TableMode {
    /// Row by row, each cell with its column header: "Name: Anna, Role:
    /// host."
    #[default]
    Read,
    /// Tables are left out.
    Skip,
}

/// How transcript speaker prefixes (`**[00:01:02] Anna:** text`) are read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SpeakerMode {
    /// "Anna: text", the name said again only when the speaker changes.
    #[default]
    Name,
    /// Only the text.
    Drop,
}

/// Options for [`prepare`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrepOptions {
    pub tables: TableMode,
    pub speakers: SpeakerMode,
    /// Hard limit on a chunk's length in characters (clamped to at least
    /// [`MIN_MAX_CHUNK_CHARS`]).
    pub max_chunk_chars: usize,
}

impl Default for PrepOptions {
    fn default() -> Self {
        PrepOptions {
            tables: TableMode::Read,
            speakers: SpeakerMode::Name,
            max_chunk_chars: DEFAULT_MAX_CHUNK_CHARS,
        }
    }
}

/// The pause the engine should leave after a chunk, shortest first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Pause {
    /// The chunk ends inside a sentence (an over-long sentence was cut):
    /// continue without a gap.
    None,
    /// Between two sentences of one paragraph.
    Sentence,
    /// After a list item, a table row or a line of the same speaker.
    Line,
    /// After a paragraph, or when the speaker changes.
    Paragraph,
    /// After a heading or a thematic break, and before a heading.
    Section,
}

impl Pause {
    /// Suggested silence in milliseconds. Engines that produce their own
    /// sentence-final silence may subtract it.
    pub fn suggested_ms(self) -> u32 {
        match self {
            Pause::None => 0,
            Pause::Sentence => 150,
            Pause::Line => 300,
            Pause::Paragraph => 600,
            Pause::Section => 1000,
        }
    }
}

/// One piece of text for the engine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chunk {
    /// Position in the document, from 0 (stable for the same input and
    /// options).
    pub index: usize,
    /// Speakable text: no markup, numbers and abbreviations in words.
    pub text: String,
    /// Pause after this chunk.
    pub pause_after: Pause,
}

/// Markdown document (frontmatter allowed) → speakable chunks.
pub fn prepare(markdown: &str, lang: Lang, opts: &PrepOptions) -> Vec<Chunk> {
    let max = opts.max_chunk_chars.max(MIN_MAX_CHUNK_CHARS);
    let mut chunks = Vec::new();
    for block in markdown::blocks(markdown, opts) {
        let text = normalize::normalize(&block.text, lang);
        let pieces = chunk::chunk_block(&text, max);
        let last = pieces.len().saturating_sub(1);
        for (i, (piece, inner_pause)) in pieces.into_iter().enumerate() {
            chunks.push(Chunk {
                index: chunks.len(),
                text: piece,
                pause_after: if i == last { block.pause } else { inner_pause },
            });
        }
    }
    chunks
}

/// The whole document as one speakable string (chunks joined by spaces,
/// blocks by newlines) — for previews, tests and debugging.
pub fn speakable_text(markdown: &str, lang: Lang, opts: &PrepOptions) -> String {
    let mut out = String::new();
    for c in prepare(markdown, lang, opts) {
        out.push_str(&c.text);
        out.push(if c.pause_after >= Pause::Line {
            '\n'
        } else {
            ' '
        });
    }
    out.trim_end().to_string()
}

/// Normalise one piece of plain text (no markdown) for `lang`.
pub fn normalize_text(text: &str, lang: Lang) -> String {
    normalize::normalize(text, lang)
}

#[cfg(test)]
mod tests;
