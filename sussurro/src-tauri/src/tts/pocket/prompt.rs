//! Text → Pocket prompts: Kyutai's `prepare_text_prompt` and a token cap.
//!
//! [`crate::tts::text`] already cuts documents into ~160-character chunks,
//! which is about one Pocket generation. Pocket conditions a generation on
//! at most [`MAX_TOKENS`] tokens (`MAX_TOKEN_PER_CHUNK` upstream), so a
//! chunk the tokenizer counts longer is cut again here: at sentence ends,
//! then clause marks, then between words — never inside a word.

/// Most tokens one generation is conditioned on.
pub const MAX_TOKENS: usize = 50;

/// Characters the spike's driver (#236) drops or replaces before
/// tokenizing: quotes and brackets carry nothing the model can say, and
/// typographic apostrophes are not in the vocabulary.
fn replace_chars(text: &str) -> String {
    text.chars()
        .filter_map(|c| match c {
            '"' | '“' | '”' | '„' | '«' | '»' | '(' | ')' | '[' | ']' => None,
            '’' | '‘' => Some('\''),
            c => Some(c),
        })
        .collect()
}

/// Kyutai's `prepare_text_prompt`: one line, a capital first letter, final
/// punctuation, optional padding for very short inputs. Returns the text
/// and how many frames to keep generating after the end-of-speech signal
/// (the upstream guess + 2). `None` for empty text.
pub fn prepare(text: &str, pad_short: bool, remove_semicolons: bool) -> Option<(String, usize)> {
    let mut t = replace_chars(text).trim().to_string();
    if t.is_empty() {
        return None;
    }
    t = t.replace(['\n', '\r'], " ").replace("  ", " ");
    if remove_semicolons {
        t = t.replace(';', ",");
    }
    let words = t.split_whitespace().count();
    let frames_after_eos = if words <= 4 { 3 } else { 1 } + 2;
    let mut chars = t.chars();
    if let Some(first) = chars.next() {
        if !first.is_uppercase() {
            t = first.to_uppercase().chain(chars).collect();
        }
    }
    if t.chars().last().is_some_and(char::is_alphanumeric) {
        t.push('.');
    }
    if pad_short && t.split_whitespace().count() < 5 {
        t = format!("{}{t}", " ".repeat(8));
    }
    Some((t, frames_after_eos))
}

/// `text` cut into pieces of at most `max` tokens as `count` measures them.
/// A piece that can't be cut further (one very long word) is kept whole.
pub fn split_to_tokens(text: &str, max: usize, count: &dyn Fn(&str) -> usize) -> Vec<String> {
    let text = text.trim();
    if text.is_empty() {
        return Vec::new();
    }
    if count(text) <= max {
        return vec![text.to_string()];
    }
    // Sentence ends first, then clause marks, then single words.
    for marks in [&['.', '!', '?', '…'][..], &[',', ';', ':'][..]] {
        let parts = split_after(text, marks);
        if parts.len() > 1 {
            return pack(parts, max, count);
        }
    }
    pack(
        text.split_whitespace().map(str::to_string).collect(),
        max,
        count,
    )
}

/// Split after every run of `marks` that is followed by a space.
fn split_after(text: &str, marks: &[char]) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        cur.push(c);
        if marks.contains(&c) && chars.peek().is_some_and(|n| n.is_whitespace()) {
            out.push(cur.trim().to_string());
            cur.clear();
        }
    }
    if !cur.trim().is_empty() {
        out.push(cur.trim().to_string());
    }
    out
}

/// Join `parts` greedily into pieces of at most `max` tokens; a part over
/// the limit is split again (recursively) before it is packed.
fn pack(parts: Vec<String>, max: usize, count: &dyn Fn(&str) -> usize) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut cur = String::new();
    for part in parts {
        let pieces = if count(&part) > max && part.contains(char::is_whitespace) {
            split_to_tokens(&part, max, count)
        } else {
            vec![part]
        };
        for p in pieces {
            let joined = if cur.is_empty() {
                p.clone()
            } else {
                format!("{cur} {p}")
            };
            if cur.is_empty() || count(&joined) <= max {
                cur = joined;
            } else {
                out.push(std::mem::replace(&mut cur, p));
            }
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(s: &str) -> usize {
        s.split_whitespace().count()
    }

    #[test]
    fn prompts_are_prepared_like_kyutai() {
        assert_eq!(prepare("  ", false, false), None);
        assert_eq!(prepare("ciao", false, false), Some(("Ciao.".into(), 5)));
        assert_eq!(
            prepare("è una frase\ncon più di quattro parole!", false, false),
            Some(("È una frase con più di quattro parole!".into(), 3))
        );
        assert_eq!(
            prepare("a; b", false, true),
            Some(("A, b.".into(), 5)),
            "semicolons become commas when the bundle asks"
        );
        assert_eq!(
            prepare("hi there", true, false),
            Some(("        Hi there.".into(), 5)),
            "short inputs padded when the bundle asks"
        );
        assert_eq!(
            prepare("«Lui» disse: “sì” (forse) [nota] l’altro", false, false),
            Some(("Lui disse: sì forse nota l'altro.".into(), 3))
        );
    }

    #[test]
    fn short_text_is_one_piece() {
        assert_eq!(
            split_to_tokens("Una frase breve.", 50, &words),
            ["Una frase breve."]
        );
        assert!(split_to_tokens("   ", 50, &words).is_empty());
    }

    #[test]
    fn long_text_is_cut_at_sentences_then_clauses_then_words() {
        let t = "Uno due tre. Quattro cinque sei. Sette otto.";
        assert_eq!(
            split_to_tokens(t, 6, &words),
            ["Uno due tre. Quattro cinque sei.", "Sette otto."]
        );
        let t = "uno due, tre quattro, cinque sei";
        assert_eq!(
            split_to_tokens(t, 4, &words),
            ["uno due, tre quattro,", "cinque sei"]
        );
        let t = "a b c d e f g";
        assert_eq!(split_to_tokens(t, 3, &words), ["a b c", "d e f", "g"]);
        // A sentence over the limit inside a text is cut on its own.
        let t = "Corta. uno due tre quattro cinque. Fine.";
        let out = split_to_tokens(t, 3, &words);
        assert!(out.iter().all(|p| words(p) <= 3), "{out:?}");
        assert_eq!(out.join(" "), t);
        // One word longer than the limit stays whole.
        assert_eq!(split_to_tokens("x", 0, &words), ["x"]);
    }
}
