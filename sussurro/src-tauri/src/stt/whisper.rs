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

    /// samples: 16 kHz mono f32. initial_prompt biases vocabulary (personal dictionary).
    pub fn transcribe(&self, samples: &[f32], initial_prompt: Option<&str>) -> Result<String> {
        let mut state = self.ctx.create_state().context("create whisper state")?;
        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
        params.set_language(Some("auto"));
        params.set_print_progress(false);
        params.set_print_special(false);
        params.set_print_realtime(false);
        params.set_print_timestamps(false);
        if let Some(p) = initial_prompt {
            params.set_initial_prompt(p);
        }
        state.full(params, samples).context("whisper inference failed")?;

        let n = state.full_n_segments();
        let mut text = String::new();
        for i in 0..n {
            if let Some(segment) = state.get_segment(i) {
                text.push_str(segment.to_str().context("segment text")?);
            }
        }
        Ok(text.trim().to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Needs a downloaded model. Run manually after downloading ggml-base.en.bin:
    /// $env:SUSSURRO_TEST_MODEL="C:\path\to\ggml-base.en.bin"; cargo test transcribe_silence -- --ignored --nocapture
    #[test]
    #[ignore]
    fn transcribe_silence_returns_without_panicking() {
        let model = std::env::var("SUSSURRO_TEST_MODEL").expect("set SUSSURRO_TEST_MODEL");
        let t = Transcriber::load(std::path::Path::new(&model)).unwrap();
        let silence = vec![0.0f32; 16_000]; // 1 s of silence
        let text = t.transcribe(&silence, None).unwrap();
        println!("transcript of silence: {text:?}");
    }
}
