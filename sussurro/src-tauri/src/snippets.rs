use crate::settings::Snippet;

/// Case/punctuation-insensitive form used to compare a transcript to cues:
/// "Firma email!" and "firma  email" both become "firma email".
fn normalize(s: &str) -> String {
    s.to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { ' ' })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// The snippet whose cue matches the whole transcript, if any.
pub fn find<'a>(snippets: &'a [Snippet], transcript: &str) -> Option<&'a Snippet> {
    let spoken = normalize(transcript);
    if spoken.is_empty() {
        return None;
    }
    snippets
        .iter()
        .filter(|s| !s.cue.trim().is_empty())
        .find(|s| normalize(&s.cue) == spoken)
}

/// Dictionary auto-learning: words the user introduced when correcting a
/// transcript, worth remembering. Skips short words, words already present in
/// the original text, and words already in the dictionary (case-insensitive).
pub fn learned_words(original: &str, corrected: &str, dictionary: &[String]) -> Vec<String> {
    use std::collections::HashSet;

    fn words(s: &str) -> Vec<String> {
        s.split(|c: char| !(c.is_alphanumeric() || c == '\''))
            .filter(|w| !w.is_empty())
            .map(str::to_string)
            .collect()
    }

    let original_words: HashSet<String> =
        words(original).into_iter().map(|w| w.to_lowercase()).collect();
    let dictionary_words: HashSet<String> =
        dictionary.iter().map(|w| w.to_lowercase()).collect();
    let mut seen = HashSet::new();

    words(corrected)
        .into_iter()
        .filter(|w| w.chars().count() >= 3)
        .filter(|w| !original_words.contains(&w.to_lowercase()))
        .filter(|w| !dictionary_words.contains(&w.to_lowercase()))
        .filter(|w| seen.insert(w.to_lowercase()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snip(cue: &str, text: &str) -> Snippet {
        Snippet { cue: cue.into(), text: text.into() }
    }

    #[test]
    fn cue_matches_ignoring_case_and_punctuation() {
        let snippets = vec![snip("firma email", "Cordiali saluti,\nFrancesco")];
        assert!(find(&snippets, "Firma email.").is_some());
        assert!(find(&snippets, "firma  EMAIL!").is_some());
        assert!(find(&snippets, "firma").is_none());
        assert!(find(&snippets, "manda la firma email a Luca").is_none());
    }

    #[test]
    fn empty_cue_or_transcript_never_matches() {
        let snippets = vec![snip("", "boom"), snip("   ", "boom")];
        assert!(find(&snippets, "boom").is_none());
        assert!(find(&[snip("ciao", "x")], "").is_none());
    }

    #[test]
    fn learns_new_words_from_corrections() {
        let learned = learned_words(
            "Ciao, sto usando susurro con taury.",
            "Ciao, sto usando Sussurro con Tauri.",
            &[],
        );
        assert_eq!(learned, vec!["Sussurro".to_string(), "Tauri".to_string()]);
    }

    #[test]
    fn case_only_changes_are_not_learned() {
        // Capitalization fixes (sentence starts, proper nouns already spelled
        // right) must not pollute the dictionary.
        assert!(learned_words("ciao da tauri", "Ciao da Tauri", &[]).is_empty());
    }

    #[test]
    fn unchanged_text_learns_nothing() {
        assert!(learned_words("stesso testo", "stesso testo", &[]).is_empty());
    }

    #[test]
    fn skips_dictionary_words_short_words_and_duplicates() {
        let learned = learned_words(
            "il gatto",
            "il Sussurro di Sussurro va qui",
            &["sussurro".into()],
        );
        // "Sussurro" already in dictionary (case-insensitive), "di"/"va" too short.
        assert_eq!(learned, vec!["qui".to_string()]);
    }
}
