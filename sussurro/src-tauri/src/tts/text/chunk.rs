//! Sentence splitting and chunking of one normalised block (pure).
//!
//! A sentence ends at `.`, `!`, `?` or `...` (plus any closing quotes or
//! brackets) followed by a space and a capital letter, a digit or an
//! opening quote; a lowercase word after the stop means the stop was not a
//! sentence end. Normalisation has already expanded abbreviations and
//! numbers, so these rules see few false stops.
//!
//! Sentences are packed greedily into chunks of at most `max` characters.
//! A sentence longer than `max` is cut at the last clause mark (`,` `;`
//! `:`) in its first `max` characters, if that leaves a piece of at least a
//! third of `max`; else at the last space; a single word longer than `max`
//! is cut at `max`. Lengths are counted in characters, not bytes.

use super::Pause;

const CLOSERS: &[char] = &['"', '\'', '»', ')', ']', '”', '’'];
const OPENERS: &[char] = &['"', '\'', '«', '(', '[', '“', '‘', '¿', '¡'];

/// Split normalised text into sentences (each trimmed, none empty).
pub fn split_sentences(text: &str) -> Vec<String> {
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let mut out = Vec::new();
    let mut start = 0usize;
    let mut i = 0usize;
    while i < chars.len() {
        let (_, c) = chars[i];
        if !matches!(c, '.' | '!' | '?' | '…') {
            i += 1;
            continue;
        }
        // The whole run of stops and closers.
        let mut j = i + 1;
        while j < chars.len()
            && (matches!(chars[j].1, '.' | '!' | '?' | '…') || CLOSERS.contains(&chars[j].1))
        {
            j += 1;
        }
        let end_byte = chars.get(j).map_or(text.len(), |(b, _)| *b);
        if j >= chars.len() {
            break;
        }
        if !chars[j].1.is_whitespace() {
            i = j;
            continue;
        }
        let mut k = j;
        while k < chars.len() && chars[k].1.is_whitespace() {
            k += 1;
        }
        let next = chars.get(k).map(|(_, c)| *c);
        let boundary = match next {
            None => false,
            Some(n) => n.is_uppercase() || n.is_ascii_digit() || OPENERS.contains(&n),
        };
        if boundary {
            let s = text[start..end_byte].trim();
            if !s.is_empty() {
                out.push(s.to_string());
            }
            start = chars[k].0;
        }
        i = k;
    }
    let rest = text[start..].trim();
    if !rest.is_empty() {
        out.push(rest.to_string());
    }
    out
}

fn char_len(s: &str) -> usize {
    s.chars().count()
}

/// Byte offset of the `n`th character (or the end).
fn byte_at(s: &str, n: usize) -> usize {
    s.char_indices().nth(n).map_or(s.len(), |(b, _)| b)
}

/// Cut one over-long sentence into pieces of at most `max` characters.
fn split_long(sentence: &str, max: usize) -> Vec<String> {
    let mut pieces = Vec::new();
    let mut rest = sentence.trim();
    while char_len(rest) > max {
        let window_end = byte_at(rest, max);
        let window = &rest[..window_end];
        let min_piece = byte_at(rest, max / 3);
        // After a clause mark followed by a space, if the piece isn't tiny.
        let clause = window
            .char_indices()
            .filter(|(b, c)| {
                matches!(c, ',' | ';' | ':')
                    && *b >= min_piece
                    && rest[b + c.len_utf8()..].starts_with(' ')
            })
            .map(|(b, c)| b + c.len_utf8())
            .next_back();
        let cut = clause
            .or_else(|| {
                // The space right after the window counts: the window then
                // ends exactly on a word.
                if rest[window_end..].starts_with(' ') {
                    Some(window_end)
                } else {
                    window.rfind(' ').filter(|b| *b > 0)
                }
            })
            .unwrap_or(window_end);
        let piece = rest[..cut].trim();
        if !piece.is_empty() {
            pieces.push(piece.to_string());
        }
        rest = rest[cut..].trim_start();
    }
    if !rest.is_empty() {
        pieces.push(rest.to_string());
    }
    pieces
}

/// One block → `(chunk text, pause after it)`; the caller replaces the
/// last pause with the block's own.
pub fn chunk_block(text: &str, max: usize) -> Vec<(String, Pause)> {
    let mut out: Vec<(String, Pause)> = Vec::new();
    let mut current = String::new();
    for sentence in split_sentences(text) {
        if char_len(&sentence) > max {
            if !current.is_empty() {
                out.push((std::mem::take(&mut current), Pause::Sentence));
            }
            let pieces = split_long(&sentence, max);
            let last = pieces.len().saturating_sub(1);
            for (i, p) in pieces.into_iter().enumerate() {
                let pause = if i == last {
                    Pause::Sentence
                } else {
                    Pause::None
                };
                out.push((p, pause));
            }
            continue;
        }
        if current.is_empty() {
            current = sentence;
        } else if char_len(&current) + 1 + char_len(&sentence) <= max {
            current.push(' ');
            current.push_str(&sentence);
        } else {
            out.push((std::mem::replace(&mut current, sentence), Pause::Sentence));
        }
    }
    if !current.is_empty() {
        out.push((current, Pause::Sentence));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sentences_split_on_stops_before_capitals() {
        assert_eq!(
            split_sentences("One. Two! Three? «Four», she said... Five."),
            ["One.", "Two!", "Three?", "«Four», she said...", "Five."]
        );
    }

    #[test]
    fn a_stop_before_lowercase_is_not_a_sentence_end() {
        assert_eq!(
            split_sentences("It costs about 3 p. per page. Then stop."),
            ["It costs about 3 p. per page.", "Then stop."]
        );
    }

    #[test]
    fn closing_quotes_stay_with_their_sentence() {
        assert_eq!(
            split_sentences("\"It's a first step.\" Then \"but now.\""),
            ["\"It's a first step.\"", "Then \"but now.\""]
        );
        assert_eq!(split_sentences("Ok.) Next."), ["Ok.)", "Next."]);
    }

    #[test]
    fn decimals_and_trailing_text() {
        assert_eq!(
            split_sentences("Version 2.0 is out"),
            ["Version 2.0 is out"]
        );
        assert_eq!(split_sentences("  "), Vec::<String>::new());
        assert_eq!(split_sentences("End."), ["End."]);
    }

    #[test]
    fn short_sentences_are_packed_up_to_the_limit() {
        let text = "One two. Three four. Five six. Seven eight.";
        let chunks = chunk_block(text, 20);
        assert_eq!(
            chunks,
            [
                ("One two. Three four.".to_string(), Pause::Sentence),
                ("Five six.".to_string(), Pause::Sentence),
                ("Seven eight.".to_string(), Pause::Sentence),
            ]
        );
        // "Five six. Seven eight." is 22 characters: over the limit.
        assert!(chunks.iter().all(|(c, _)| c.chars().count() <= 20));
    }

    #[test]
    fn a_long_sentence_is_cut_at_a_clause() {
        let s = "This sentence has a first clause, then a second clause that goes on, and a third one to finish.";
        let chunks = chunk_block(s, 60);
        assert_eq!(
            chunks,
            [
                ("This sentence has a first clause,".to_string(), Pause::None),
                (
                    "then a second clause that goes on,".to_string(),
                    Pause::None
                ),
                ("and a third one to finish.".to_string(), Pause::Sentence),
            ]
        );
    }

    #[test]
    fn without_clauses_the_cut_is_at_a_space() {
        let s = "aaaa bbbb cccc dddd eeee ffff gggg";
        let pieces = split_long(s, 12);
        assert_eq!(pieces, ["aaaa bbbb", "cccc dddd", "eeee ffff", "gggg"]);
        assert!(pieces.iter().all(|p| p.chars().count() <= 12));
    }

    #[test]
    fn a_word_longer_than_the_limit_is_cut_hard() {
        let pieces = split_long("abcdefghij klm", 4);
        assert_eq!(pieces, ["abcd", "efgh", "ij", "klm"]);
    }

    #[test]
    fn multibyte_characters_count_as_one() {
        let s = "àèìòù àèìòù àèìòù";
        let pieces = split_long(s, 11);
        assert_eq!(pieces, ["àèìòù àèìòù", "àèìòù"]);
    }

    #[test]
    fn same_input_same_chunks() {
        let text = "Alpha beta gamma. Delta epsilon, zeta eta theta iota kappa lambda mu nu. Xi omicron pi.";
        assert_eq!(chunk_block(text, 40), chunk_block(text, 40));
    }
}
