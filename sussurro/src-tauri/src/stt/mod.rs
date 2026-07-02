pub mod models;
pub mod whisper;

/// Whisper "initial prompt" that biases recognition toward personal-dictionary
/// words (names, jargon). None when the dictionary is empty.
pub fn dictionary_prompt(words: &[String]) -> Option<String> {
    if words.is_empty() {
        return None;
    }
    Some(format!("Glossary of terms that may appear: {}.", words.join(", ")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_dictionary_gives_no_prompt() {
        assert_eq!(dictionary_prompt(&[]), None);
    }

    #[test]
    fn dictionary_words_appear_in_prompt() {
        let p = dictionary_prompt(&["Sussurro".into(), "Ollama".into()]).unwrap();
        assert!(p.contains("Sussurro"));
        assert!(p.contains("Ollama"));
    }
}
