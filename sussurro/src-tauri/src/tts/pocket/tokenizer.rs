//! Pocket TTS's text tokenizer: a SentencePiece **unigram** model, read
//! from the bundle's `tokenizer.model` and run in Rust (no `sentencepiece`
//! C++ library, no `tokenizers` crate).
//!
//! Only what the pinned Pocket tokenizers use is implemented, and a model
//! that needs more is refused at load rather than tokenized differently:
//!
//! - model type unigram, 4,000 pieces;
//! - normalizer `identity` with no precompiled character map (no NFKC),
//!   `add_dummy_prefix` on, `remove_extra_whitespaces` off,
//!   `escape_whitespaces` on (only U+0020 becomes `▁`);
//! - byte fallback on: a character no piece covers is written as its UTF-8
//!   bytes (`<0xC3>` `<0x9C>` …).
//!
//! Encoding is SentencePiece's own Viterbi search over the lattice of
//! pieces (`unigram_model.cc`, `EncodeOptimized`): the best total score
//! wins, a position no one-character piece covers gets an unknown node
//! scored `min score − 10`, and ties keep the first (shortest) candidate.
//! The live test checks the ids against the reference library on the
//! pinned files.

use anyhow::{bail, Context, Result};
use std::collections::HashMap;

/// SentencePiece's penalty for an unknown character (`kUnkPenalty`).
const UNK_PENALTY: f32 = 10.0;
/// The whitespace marker (U+2581).
const SPACE: char = '\u{2581}';

/// Piece types of `sentencepiece_model.proto`.
const TYPE_NORMAL: u64 = 1;
const TYPE_UNKNOWN: u64 = 2;
const TYPE_USER_DEFINED: u64 = 4;
const TYPE_BYTE: u64 = 6;
/// `TrainerSpec.model_type` for a unigram model.
const MODEL_UNIGRAM: u64 = 1;

/// A loaded unigram tokenizer.
pub struct Tokenizer {
    /// Matchable pieces (normal, user-defined) by text → (id, score).
    pieces: HashMap<String, (u32, f32)>,
    /// Longest matchable piece, in characters.
    max_piece_chars: usize,
    unk_id: u32,
    unk_score: f32,
    /// Id of `<0xNN>` for each byte.
    byte_ids: [u32; 256],
    add_dummy_prefix: bool,
    vocab_size: usize,
}

impl std::fmt::Debug for Tokenizer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Tokenizer")
            .field("vocab_size", &self.vocab_size)
            .finish_non_exhaustive()
    }
}

impl Tokenizer {
    /// Parse a `tokenizer.model` file.
    pub fn from_file(path: &std::path::Path) -> Result<Tokenizer> {
        let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
        Tokenizer::from_bytes(&bytes)
    }

    /// Parse a serialized `ModelProto`.
    pub fn from_bytes(bytes: &[u8]) -> Result<Tokenizer> {
        let model = parse_model(bytes)?;
        if model.model_type != MODEL_UNIGRAM {
            bail!(
                "unsupported tokenizer: model type {} (unigram only)",
                model.model_type
            );
        }
        if !model.byte_fallback {
            bail!("unsupported tokenizer: byte fallback is off");
        }
        if !model.charsmap_empty {
            bail!("unsupported tokenizer: it needs a precompiled normalizer");
        }
        if model.remove_extra_whitespaces || !model.escape_whitespaces {
            bail!("unsupported tokenizer: whitespace handling differs from Pocket's");
        }
        let mut pieces = HashMap::new();
        let mut max_piece_chars = 1;
        let mut min_score = f32::INFINITY;
        let mut unk_id = None;
        let mut byte_ids = [u32::MAX; 256];
        for (id, p) in model.pieces.iter().enumerate() {
            let id = id as u32;
            match p.kind {
                TYPE_NORMAL | TYPE_USER_DEFINED => {
                    if p.kind == TYPE_NORMAL {
                        min_score = min_score.min(p.score);
                    }
                    max_piece_chars = max_piece_chars.max(p.text.chars().count());
                    pieces.insert(p.text.clone(), (id, p.score));
                }
                TYPE_UNKNOWN => unk_id = Some(id),
                TYPE_BYTE => {
                    let hex = p
                        .text
                        .strip_prefix("<0x")
                        .and_then(|s| s.strip_suffix('>'))
                        .and_then(|h| u8::from_str_radix(h, 16).ok())
                        .with_context(|| format!("bad byte piece '{}'", p.text))?;
                    byte_ids[hex as usize] = id;
                }
                // Control (<s>, </s>) and unused pieces never match text.
                _ => {}
            }
        }
        let unk_id = unk_id.context("the tokenizer has no unknown piece")?;
        if byte_ids.contains(&u32::MAX) {
            bail!("the tokenizer lacks some of the 256 byte pieces");
        }
        if !min_score.is_finite() {
            bail!("the tokenizer has no pieces");
        }
        Ok(Tokenizer {
            pieces,
            max_piece_chars,
            unk_id,
            unk_score: min_score - UNK_PENALTY,
            byte_ids,
            add_dummy_prefix: model.add_dummy_prefix,
            vocab_size: model.pieces.len(),
        })
    }

    pub fn vocab_size(&self) -> usize {
        self.vocab_size
    }

    /// Token ids of `text` (no BOS/EOS), as `SentencePieceProcessor.encode`.
    pub fn encode(&self, text: &str) -> Vec<u32> {
        if text.is_empty() {
            return Vec::new();
        }
        // Identity normalizer: only the dummy prefix and space escaping.
        let mut norm = String::with_capacity(text.len() + 3);
        if self.add_dummy_prefix {
            norm.push(SPACE);
        }
        norm.extend(text.chars().map(|c| if c == ' ' { SPACE } else { c }));
        let chars: Vec<char> = norm.chars().collect();
        let n = chars.len();

        // best[i] = (score, start of the last piece, id) of the best path
        // ending at char i; `usize::MAX` start = unreached.
        let mut best: Vec<(f32, usize, u32)> = vec![(0.0, usize::MAX, 0); n + 1];
        best[0] = (0.0, 0, 0);
        let mut buf = String::new();
        for start in 0..n {
            if best[start].1 == usize::MAX {
                continue;
            }
            let base = best[start].0;
            let mut has_single = false;
            buf.clear();
            for (len, &c) in chars[start..n.min(start + self.max_piece_chars)]
                .iter()
                .enumerate()
            {
                buf.push(c);
                let Some(&(id, score)) = self.pieces.get(buf.as_str()) else {
                    continue;
                };
                let end = start + len + 1;
                if len == 0 {
                    has_single = true;
                }
                let cand = base + score;
                if best[end].1 == usize::MAX || cand > best[end].0 {
                    best[end] = (cand, start, id);
                }
            }
            if !has_single {
                let end = start + 1;
                let cand = base + self.unk_score;
                if best[end].1 == usize::MAX || cand > best[end].0 {
                    best[end] = (cand, start, self.unk_id);
                }
            }
        }

        // Backtrack, then expand unknown characters into their bytes.
        let mut path = Vec::new();
        let mut end = n;
        while end > 0 {
            let (_, start, id) = best[end];
            path.push((start, end, id));
            end = start;
        }
        path.reverse();
        let mut ids = Vec::with_capacity(path.len());
        let mut utf8 = [0u8; 4];
        for (start, end, id) in path {
            if id == self.unk_id {
                for c in &chars[start..end] {
                    for b in c.encode_utf8(&mut utf8).bytes() {
                        ids.push(self.byte_ids[b as usize]);
                    }
                }
            } else {
                ids.push(id);
            }
        }
        ids
    }

    /// Number of tokens of `text`.
    pub fn count(&self, text: &str) -> usize {
        self.encode(text).len()
    }
}

// ---- A minimal protobuf reader for `sentencepiece_model.proto` ----

struct Piece {
    text: String,
    score: f32,
    kind: u64,
}

struct Model {
    pieces: Vec<Piece>,
    model_type: u64,
    byte_fallback: bool,
    add_dummy_prefix: bool,
    remove_extra_whitespaces: bool,
    escape_whitespaces: bool,
    charsmap_empty: bool,
}

/// One field of a protobuf message.
enum Field<'a> {
    Varint(u64),
    Fixed32(u32),
    Fixed64,
    Bytes(&'a [u8]),
}

struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn new(buf: &'a [u8]) -> Self {
        Reader { buf, pos: 0 }
    }

    fn varint(&mut self) -> Result<u64> {
        let mut v = 0u64;
        for shift in (0..64).step_by(7) {
            let b = *self.buf.get(self.pos).context("truncated varint")?;
            self.pos += 1;
            v |= u64::from(b & 0x7f) << shift;
            if b & 0x80 == 0 {
                return Ok(v);
            }
        }
        bail!("varint too long")
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self.pos.checked_add(n).context("length overflow")?;
        let s = self.buf.get(self.pos..end).context("truncated field")?;
        self.pos = end;
        Ok(s)
    }

    /// The next `(field number, value)`, or `None` at the end.
    fn next(&mut self) -> Result<Option<(u64, Field<'a>)>> {
        if self.pos >= self.buf.len() {
            return Ok(None);
        }
        let key = self.varint()?;
        let field = match key & 7 {
            0 => Field::Varint(self.varint()?),
            1 => {
                self.take(8)?;
                Field::Fixed64
            }
            2 => {
                let len = usize::try_from(self.varint()?).context("field too long")?;
                Field::Bytes(self.take(len)?)
            }
            5 => {
                let b = self.take(4)?;
                Field::Fixed32(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
            }
            w => bail!("unsupported protobuf wire type {w}"),
        };
        Ok(Some((key >> 3, field)))
    }
}

fn parse_model(bytes: &[u8]) -> Result<Model> {
    let mut m = Model {
        pieces: Vec::new(),
        model_type: MODEL_UNIGRAM,
        byte_fallback: false,
        // proto2 defaults of NormalizerSpec.
        add_dummy_prefix: true,
        remove_extra_whitespaces: true,
        escape_whitespaces: true,
        charsmap_empty: true,
    };
    let mut r = Reader::new(bytes);
    while let Some((num, f)) = r.next()? {
        match (num, f) {
            (1, Field::Bytes(b)) => m.pieces.push(parse_piece(b)?),
            (2, Field::Bytes(b)) => {
                let mut t = Reader::new(b);
                while let Some((n, f)) = t.next()? {
                    match (n, f) {
                        (3, Field::Varint(v)) => m.model_type = v,
                        (35, Field::Varint(v)) => m.byte_fallback = v != 0,
                        _ => {}
                    }
                }
            }
            (3, Field::Bytes(b)) => {
                let mut t = Reader::new(b);
                while let Some((n, f)) = t.next()? {
                    match (n, f) {
                        (2, Field::Bytes(c)) => m.charsmap_empty = c.is_empty(),
                        (3, Field::Varint(v)) => m.add_dummy_prefix = v != 0,
                        (4, Field::Varint(v)) => m.remove_extra_whitespaces = v != 0,
                        (5, Field::Varint(v)) => m.escape_whitespaces = v != 0,
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
    if m.pieces.is_empty() {
        bail!("not a SentencePiece model (no pieces)");
    }
    Ok(m)
}

fn parse_piece(bytes: &[u8]) -> Result<Piece> {
    let mut p = Piece {
        text: String::new(),
        score: 0.0,
        kind: TYPE_NORMAL,
    };
    let mut r = Reader::new(bytes);
    while let Some((n, f)) = r.next()? {
        match (n, f) {
            (1, Field::Bytes(b)) => {
                p.text = String::from_utf8(b.to_vec()).context("piece is not UTF-8")?
            }
            (2, Field::Fixed32(v)) => p.score = f32::from_bits(v),
            (3, Field::Varint(v)) => p.kind = v,
            _ => {}
        }
    }
    Ok(p)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Serialize a tiny `ModelProto` (tests only).
    pub(crate) fn model_bytes(pieces: &[(&str, f32, u64)], dummy_prefix: bool) -> Vec<u8> {
        fn varint(out: &mut Vec<u8>, mut v: u64) {
            loop {
                let b = (v & 0x7f) as u8;
                v >>= 7;
                if v == 0 {
                    out.push(b);
                    return;
                }
                out.push(b | 0x80);
            }
        }
        fn bytes_field(out: &mut Vec<u8>, num: u64, b: &[u8]) {
            varint(out, num << 3 | 2);
            varint(out, b.len() as u64);
            out.extend_from_slice(b);
        }
        fn varint_field(out: &mut Vec<u8>, num: u64, v: u64) {
            varint(out, num << 3);
            varint(out, v);
        }
        let mut out = Vec::new();
        let mut all: Vec<(String, f32, u64)> = vec![
            ("<unk>".into(), 0.0, TYPE_UNKNOWN),
            ("<s>".into(), 0.0, 3),
            ("</s>".into(), 0.0, 3),
        ];
        for b in 0..=255u32 {
            all.push((format!("<0x{b:02X}>"), 0.0, TYPE_BYTE));
        }
        all.extend(pieces.iter().map(|(t, s, k)| (t.to_string(), *s, *k)));
        for (t, s, k) in &all {
            let mut p = Vec::new();
            bytes_field(&mut p, 1, t.as_bytes());
            varint(&mut p, 2 << 3 | 5);
            p.extend_from_slice(&s.to_le_bytes());
            varint_field(&mut p, 3, *k);
            bytes_field(&mut out, 1, &p);
        }
        let mut trainer = Vec::new();
        varint_field(&mut trainer, 3, MODEL_UNIGRAM);
        varint_field(&mut trainer, 35, 1);
        bytes_field(&mut out, 2, &trainer);
        let mut norm = Vec::new();
        bytes_field(&mut norm, 1, b"identity");
        varint_field(&mut norm, 3, u64::from(dummy_prefix));
        varint_field(&mut norm, 4, 0);
        varint_field(&mut norm, 5, 1);
        bytes_field(&mut out, 3, &norm);
        out
    }

    /// Ids of the test pieces: 3 specials + 256 bytes come first.
    const FIRST: u32 = 3 + 256;

    fn tok() -> Tokenizer {
        Tokenizer::from_bytes(&model_bytes(
            &[
                ("▁", -2.0, TYPE_NORMAL),     // FIRST
                ("▁ciao", -3.0, TYPE_NORMAL), // +1
                ("▁c", -4.0, TYPE_NORMAL),    // +2
                ("iao", -4.0, TYPE_NORMAL),   // +3
                ("a", -5.0, TYPE_NORMAL),     // +4
                ("b", -5.0, TYPE_NORMAL),     // +5
                ("ab", -9.5, TYPE_NORMAL),    // +6: worse than a + b
                ("é", -6.0, TYPE_NORMAL),     // +7
                (".", -1.0, TYPE_NORMAL),     // +8
                ("unused", -1.0, 5),          // +9
            ],
            true,
        ))
        .unwrap()
    }

    #[test]
    fn viterbi_picks_the_best_total_score() {
        let t = tok();
        // "▁ciao" (−3) beats "▁c" + "iao" (−8).
        assert_eq!(t.encode("ciao"), vec![FIRST + 1]);
        // "ab" (−9.5) beats "a" + "b" (−10).
        assert_eq!(t.encode("ab"), vec![FIRST, FIRST + 6]);
        assert_eq!(t.encode("ciao."), vec![FIRST + 1, FIRST + 8]);
    }

    #[test]
    fn spaces_become_the_marker_and_a_dummy_prefix_is_added() {
        let t = tok();
        assert_eq!(t.encode("ciao ciao"), vec![FIRST + 1, FIRST + 1]);
        // Two spaces: the second is a lone "▁".
        assert_eq!(t.encode("ciao  ciao"), vec![FIRST + 1, FIRST, FIRST + 1]);
        assert!(t.encode("").is_empty());
    }

    #[test]
    fn unknown_characters_fall_back_to_their_utf8_bytes() {
        let t = tok();
        // 'Ü' = C3 9C, '!' = 21: no piece covers them.
        assert_eq!(t.encode("Ü!"), vec![FIRST, 3 + 0xC3, 3 + 0x9C, 3 + 0x21]);
        // A known multi-byte character is one piece.
        assert_eq!(t.encode("é"), vec![FIRST, FIRST + 7]);
        // An unused piece never matches: bytes instead.
        let ids = t.encode("unused");
        assert!(!ids.contains(&(FIRST + 9)), "{ids:?}");
        // Control pieces are never produced from text.
        assert!(!t.encode("<s>").contains(&1));
        assert_eq!(t.count("ciao."), 2);
    }

    #[test]
    fn without_the_dummy_prefix_text_starts_bare() {
        let t = Tokenizer::from_bytes(&model_bytes(
            &[("a", -1.0, TYPE_NORMAL), ("▁a", -1.0, TYPE_NORMAL)],
            false,
        ))
        .unwrap();
        assert_eq!(t.encode("a a"), vec![FIRST, FIRST + 1]);
    }

    #[test]
    fn unsupported_or_broken_models_are_refused() {
        assert!(Tokenizer::from_bytes(b"").is_err());
        assert!(Tokenizer::from_bytes(&[0x0a, 0xff]).is_err(), "truncated");
        // A BPE model (type 2) is refused, not tokenized differently.
        let mut bpe = model_bytes(&[("a", -1.0, TYPE_NORMAL)], true);
        // Append a second trainer spec that overrides the model type.
        bpe.extend_from_slice(&[0x12, 0x02, 0x18, 0x02]);
        let err = Tokenizer::from_bytes(&bpe).unwrap_err();
        assert!(err.to_string().contains("model type 2"), "{err}");
    }
}
