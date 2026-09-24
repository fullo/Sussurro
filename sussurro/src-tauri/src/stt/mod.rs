pub mod models;
pub mod parakeet;
pub mod sidecar;
pub mod whisper;

/// The loaded engine, whichever it is. Kept in AppState behind a Mutex.
pub enum AnyTranscriber {
    Whisper(whisper::Transcriber),
    Parakeet(parakeet::ParakeetTranscriber),
}

impl AnyTranscriber {
    /// Unified transcription: whisper honours the dictionary prompt and
    /// language hint, parakeet auto-detects and ignores both.
    pub fn transcribe(
        &mut self,
        samples: &[f32],
        initial_prompt: Option<&str>,
        language: &str,
    ) -> anyhow::Result<String> {
        match self {
            AnyTranscriber::Whisper(t) => t.transcribe(samples, initial_prompt, language),
            AnyTranscriber::Parakeet(t) => t.transcribe(samples),
        }
    }

    /// Long-form transcription with word timings (engine, #113). Both
    /// engines report real timings; `words` can still come back empty (e.g.
    /// no tokens), and the engine then falls back to a proportional split.
    pub fn transcribe_timed(
        &mut self,
        samples: &[f32],
        initial_prompt: Option<&str>,
        language: &str,
    ) -> anyhow::Result<TimedTranscript> {
        match self {
            AnyTranscriber::Whisper(t) => t.transcribe_timed(samples, initial_prompt, language),
            AnyTranscriber::Parakeet(t) => t.transcribe_timed(samples),
        }
    }
}

/// One segment's transcript with word timings in ms relative to the start
/// of the audio passed in.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TimedTranscript {
    pub text: String,
    pub words: Vec<crate::archive::Word>,
    /// True when `words` were not measured by the engine but split
    /// proportionally over the segment (`engine::timing`).
    pub words_estimated: bool,
    /// Language detected by the engine (whisper), `None` if unknown.
    pub language: Option<String>,
}

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
