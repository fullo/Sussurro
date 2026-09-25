use anyhow::{Context, Result};
use std::path::Path;
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

/// Loaded whisper.cpp model. Loading takes seconds — load once, keep in AppState.
pub struct Transcriber {
    ctx: WhisperContext,
}

impl Transcriber {
    pub fn load(model_path: &Path) -> Result<Self> {
        let path = model_path
            .to_str()
            .context("model path is not valid UTF-8")?;
        let ctx = WhisperContext::new_with_params(path, WhisperContextParameters::default())
            .context("failed to load whisper model")?;
        Ok(Self { ctx })
    }

    /// samples: 16 kHz mono f32. initial_prompt biases vocabulary (personal
    /// dictionary). language: "auto" or an ISO 639-1 code like "it".
    pub fn transcribe(
        &self,
        samples: &[f32],
        initial_prompt: Option<&str>,
        language: &str,
    ) -> Result<String> {
        let mut state = self.ctx.create_state().context("create whisper state")?;
        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
        params.set_language(Some(language));
        params.set_print_progress(false);
        params.set_print_special(false);
        params.set_print_realtime(false);
        params.set_print_timestamps(false);
        if let Some(p) = initial_prompt {
            params.set_initial_prompt(p);
        }
        state
            .full(params, samples)
            .context("whisper inference failed")?;

        let n = state.full_n_segments();
        let mut text = String::new();
        for i in 0..n {
            if let Some(segment) = state.get_segment(i) {
                text.push_str(segment.to_str().context("segment text")?);
            }
        }
        Ok(text.trim().to_string())
    }

    /// Long-form variant of [`Self::transcribe`] (engine, #113): same
    /// decoding, plus whisper.cpp token timestamps grouped into words (ms
    /// relative to the start of `samples`) and the detected language. The
    /// dictation path keeps calling `transcribe`, unchanged.
    pub fn transcribe_timed(
        &self,
        samples: &[f32],
        initial_prompt: Option<&str>,
        language: &str,
    ) -> Result<super::TimedTranscript> {
        let mut state = self.ctx.create_state().context("create whisper state")?;
        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
        params.set_language(Some(language));
        params.set_print_progress(false);
        params.set_print_special(false);
        params.set_print_realtime(false);
        params.set_print_timestamps(false);
        params.set_token_timestamps(true);
        if let Some(p) = initial_prompt {
            params.set_initial_prompt(p);
        }
        state
            .full(params, samples)
            .context("whisper inference failed")?;

        let eot = self.ctx.token_eot();
        let mut text = String::new();
        let mut pieces = Vec::new();
        for segment in state.as_iter() {
            text.push_str(&segment.to_str_lossy().context("segment text")?);
            for i in 0..segment.n_tokens() {
                let Some(token) = segment.get_token(i) else {
                    continue;
                };
                let data = token.token_data();
                pieces.push(TokenPiece {
                    bytes: token.to_bytes().map(<[u8]>::to_vec).unwrap_or_default(),
                    // whisper.cpp times are centiseconds.
                    start_ms: (data.t0.max(0) as u64) * 10,
                    end_ms: (data.t1.max(0) as u64) * 10,
                    special: token.token_id() >= eot,
                });
            }
        }
        let language =
            whisper_rs::get_lang_str(state.full_lang_id_from_state()).map(str::to_string);
        Ok(super::TimedTranscript {
            text: text.trim().to_string(),
            words: words_from_tokens(&pieces),
            words_estimated: false,
            language,
        })
    }
}

/// One decoded whisper token with its timestamps (ms).
#[derive(Debug, Clone, PartialEq)]
pub struct TokenPiece {
    /// Raw token bytes: a multi-byte UTF-8 character can span two tokens, so
    /// words are assembled from bytes and decoded once complete.
    pub bytes: Vec<u8>,
    pub start_ms: u64,
    pub end_ms: u64,
    /// Control tokens (`[_BEG_]`, `[_TT_…]`, end of text…): never text.
    pub special: bool,
}

/// Group BPE tokens into words: a token starting with whitespace opens a new
/// word, anything else continues the current one. A word spans from its first
/// token's start to its last token's end. Pure.
pub fn words_from_tokens(pieces: &[TokenPiece]) -> Vec<crate::archive::Word> {
    let mut words = Vec::new();
    let mut cur: Option<(Vec<u8>, u64, u64)> = None;
    let flush = |cur: &mut Option<(Vec<u8>, u64, u64)>, words: &mut Vec<crate::archive::Word>| {
        if let Some((bytes, start, end)) = cur.take() {
            let w = String::from_utf8_lossy(&bytes).trim().to_string();
            if !w.is_empty() {
                words.push(crate::archive::Word {
                    w,
                    start_ms: start,
                    end_ms: end.max(start),
                });
            }
        }
    };
    for p in pieces.iter().filter(|p| !p.special && !p.bytes.is_empty()) {
        let starts_word = p.bytes[0].is_ascii_whitespace();
        match cur.as_mut() {
            Some((bytes, _, end)) if !starts_word => {
                bytes.extend_from_slice(&p.bytes);
                *end = p.end_ms;
            }
            _ => {
                flush(&mut cur, &mut words);
                cur = Some((p.bytes.clone(), p.start_ms, p.end_ms));
            }
        }
    }
    flush(&mut cur, &mut words);
    words
}

#[cfg(test)]
mod tests {
    use super::*;

    fn piece(s: &[u8], start_ms: u64, end_ms: u64) -> TokenPiece {
        TokenPiece {
            bytes: s.to_vec(),
            start_ms,
            end_ms,
            special: false,
        }
    }

    #[test]
    fn tokens_group_into_words_on_leading_space() {
        let pieces = vec![
            TokenPiece {
                special: true,
                ..piece(b"[_BEG_]", 0, 0)
            },
            piece(b" Hel", 0, 200),
            piece(b"lo", 200, 400),
            piece(b" world", 450, 900),
            piece(b".", 900, 950),
        ];
        let words = words_from_tokens(&pieces);
        let got: Vec<(&str, u64, u64)> = words
            .iter()
            .map(|w| (w.w.as_str(), w.start_ms, w.end_ms))
            .collect();
        assert_eq!(got, vec![("Hello", 0, 400), ("world.", 450, 950)]);
    }

    #[test]
    fn multibyte_characters_split_across_tokens_are_reassembled() {
        // "è" is 0xC3 0xA8: whisper can emit the two bytes as two tokens.
        let pieces = vec![piece(b" perch\xC3", 0, 300), piece(b"\xA8", 300, 350)];
        let words = words_from_tokens(&pieces);
        assert_eq!(words.len(), 1);
        assert_eq!(words[0].w, "perchè");
        assert_eq!((words[0].start_ms, words[0].end_ms), (0, 350));
    }

    #[test]
    fn empty_and_special_only_input_gives_no_words() {
        assert!(words_from_tokens(&[]).is_empty());
        let only_special = vec![TokenPiece {
            special: true,
            ..piece(b"[_TT_50]", 0, 500)
        }];
        assert!(words_from_tokens(&only_special).is_empty());
    }

    /// Needs a downloaded model. Run manually after downloading ggml-base.en.bin:
    /// $env:SUSSURRO_TEST_MODEL="C:\path\to\ggml-base.en.bin"; cargo test transcribe_silence -- --ignored --nocapture
    #[test]
    #[ignore]
    fn transcribe_silence_returns_without_panicking() {
        let model = std::env::var("SUSSURRO_TEST_MODEL").expect("set SUSSURRO_TEST_MODEL");
        let t = Transcriber::load(std::path::Path::new(&model)).unwrap();
        let silence = vec![0.0f32; 16_000]; // 1 s of silence
        let text = t.transcribe(&silence, None, "auto").unwrap();
        println!("transcript of silence: {text:?}");
    }
}
