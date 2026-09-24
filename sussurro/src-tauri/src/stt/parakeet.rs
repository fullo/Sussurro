use anyhow::{anyhow, Result};
use std::path::Path;
use transcribe_rs::onnx::parakeet::{ParakeetModel, ParakeetParams, TimestampGranularity};
use transcribe_rs::onnx::Quantization;

use super::pauses::{self, PauseSplit};

/// Directory (inside the app models dir) holding the extracted ONNX files.
pub const PARAKEET_DIR: &str = "parakeet-tdt-0.6b-v3-int8";
/// Same archive Handy ships: encoder/decoder int8 ONNX + nemo128 preprocessor.
pub const PARAKEET_URL: &str = "https://blob.handy.computer/parakeet-v3-int8.tar.gz";
pub const PARAKEET_SHA256: &str =
    "43d37191602727524a7d8c6da0eef11c4ba24320f5b4730f1a2497befc2efa77";

/// NVIDIA Parakeet TDT 0.6B v3 via transcribe-rs (ONNX Runtime, CPU).
/// Auto-detects 25 European languages; ignores prompts and language hints.
///
/// Input longer than a few seconds is transcribed in pieces cut at pauses
/// ([`super::pauses`], #194): unsplit, the model can drop every sentence
/// after a pause. Every caller (hotkey dictation, its live preview, the
/// local API, engine segments) gets the protection; whisper never splits.
pub struct ParakeetTranscriber {
    /// Boxed: keeps [`super::AnyTranscriber`]'s variants close in size.
    model: Box<ParakeetModel>,
    /// `None` only in tests and the #194 benchmark (the unsplit baseline).
    split: Option<PauseSplit>,
    /// Model calls so far (the #194 benchmark reports them).
    #[cfg(test)]
    decodes: usize,
}

impl ParakeetTranscriber {
    pub fn load(model_dir: &Path) -> Result<Self> {
        let model = ParakeetModel::load(model_dir, &Quantization::Int8)
            .map_err(|e| anyhow!("failed to load parakeet model: {e}"))?;
        Ok(Self {
            model: Box::new(model),
            split: Some(pauses::PARAKEET),
            #[cfg(test)]
            decodes: 0,
        })
    }

    /// samples: 16 kHz mono f32 — same contract as the whisper path.
    pub fn transcribe(&mut self, samples: &[f32]) -> Result<String> {
        Ok(self.transcribe_timed(samples)?.text)
    }

    /// Long-form variant (engine, #113): the same inference asking
    /// transcribe-rs for word-level timestamps (TDT frame times, seconds,
    /// already shifted back past its 250 ms leading pad) → words in ms
    /// relative to the start of `samples`.
    ///
    /// #194: the input is decoded in pieces cut at pauses, and a piece whose
    /// transcript stops while speech follows is decoded again from there.
    pub fn transcribe_timed(&mut self, samples: &[f32]) -> Result<super::TimedTranscript> {
        let Some(p) = self.split else {
            return self.timed_piece(samples);
        };
        let energy = pauses::Energy::of(samples);
        let mut decoded = Vec::new();
        for piece in energy.split(&p) {
            let mut from = piece.start;
            for _ in 0..MAX_DECODES_PER_PIECE {
                let t = self.timed_piece(&samples[from..piece.end])?;
                let heard_to = t
                    .words
                    .last()
                    .map_or(from, |w| from + ms_to_samples(w.end_ms));
                decoded.push((samples_to_ms(from), t));
                match energy.resume_at(heard_to, piece.end, &p) {
                    Some(next) if next > from => from = next,
                    _ => break,
                }
            }
        }
        Ok(join_timed(decoded))
    }

    fn timed_piece(&mut self, samples: &[f32]) -> Result<super::TimedTranscript> {
        #[cfg(test)]
        {
            self.decodes += 1;
        }
        let params = ParakeetParams {
            timestamp_granularity: Some(TimestampGranularity::Word),
            ..Default::default()
        };
        let result = self
            .model
            .transcribe_with(samples, &params)
            .map_err(|e| anyhow!("parakeet inference failed: {e}"))?;
        let words = result
            .segments
            .unwrap_or_default()
            .into_iter()
            .filter_map(|s| {
                let w = s.text.trim().to_string();
                (!w.is_empty()).then(|| crate::archive::Word {
                    w,
                    start_ms: secs_to_ms(s.start),
                    end_ms: secs_to_ms(s.end.max(s.start)),
                })
            })
            .collect();
        Ok(super::TimedTranscript {
            text: result.text.trim().to_string(),
            words,
            words_estimated: false,
            language: None,
        })
    }
}

/// The pieces' texts, trimmed, empty ones skipped, joined by one space.
fn join_texts(texts: &[String]) -> String {
    texts
        .iter()
        .map(|t| t.trim())
        .filter(|t| !t.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// One transcript from consecutive pieces, each with its start (ms) in the
/// whole input: texts joined, word times shifted onto the input's clock.
fn join_timed(pieces: Vec<(u64, super::TimedTranscript)>) -> super::TimedTranscript {
    let texts: Vec<String> = pieces.iter().map(|(_, t)| t.text.clone()).collect();
    let words = pieces
        .into_iter()
        .flat_map(|(offset, t)| {
            t.words.into_iter().map(move |w| crate::archive::Word {
                start_ms: w.start_ms + offset,
                end_ms: w.end_ms + offset,
                ..w
            })
        })
        .collect();
    super::TimedTranscript {
        text: join_texts(&texts),
        words,
        words_estimated: false,
        language: None,
    }
}

fn secs_to_ms(s: f32) -> u64 {
    (s.max(0.0) * 1000.0).round() as u64
}

/// Decodes of one piece at most: the first plus re-decodes of its rest
/// ([`pauses::Energy::resume_at`]); each one starts later in the piece.
const MAX_DECODES_PER_PIECE: usize = 8;

fn samples_to_ms(samples: usize) -> u64 {
    samples as u64 * 1000 / pauses::RATE as u64
}

fn ms_to_samples(ms: u64) -> usize {
    (ms as usize).saturating_mul(pauses::RATE) / 1000
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Lowercase words, punctuation dropped (the #109 WER normalisation).
    fn words(s: &str) -> String {
        s.to_lowercase()
            .split(|c: char| !c.is_alphanumeric())
            .filter(|w| !w.is_empty())
            .collect::<Vec<_>>()
            .join(" ")
    }

    fn word(w: &str, start_ms: u64, end_ms: u64) -> crate::archive::Word {
        crate::archive::Word {
            w: w.into(),
            start_ms,
            end_ms,
        }
    }

    #[test]
    fn pieces_join_with_one_space_and_skip_empty_ones() {
        let texts = [" Hello there. ", "", "  ", "How are you?"].map(String::from);
        assert_eq!(join_texts(&texts), "Hello there. How are you?");
        assert_eq!(join_texts(&[]), "");
    }

    #[test]
    fn timed_pieces_are_shifted_onto_the_input_clock() {
        let piece = |text: &str, words| super::super::TimedTranscript {
            text: text.into(),
            words,
            words_estimated: false,
            language: None,
        };
        let joined = join_timed(vec![
            (
                0,
                piece(
                    "One two.",
                    vec![word("One", 100, 300), word("two.", 400, 700)],
                ),
            ),
            (9_500, piece("", vec![])),
            (12_000, piece("Three.", vec![word("Three.", 50, 400)])),
        ]);
        assert_eq!(joined.text, "One two. Three.");
        assert_eq!(
            joined.words,
            vec![
                word("One", 100, 300),
                word("two.", 400, 700),
                word("Three.", 12_050, 12_400)
            ]
        );
        assert!(!joined.words_estimated);
    }

    /// #194 regression, needs the real model and a FLEURS chunk that lost
    /// its second sentence (bench-109 `corpus/chunks/en_us/03.wav`: two
    /// utterances with a ~1.4 s pause; unsplit Parakeet stops after the
    /// first):
    /// SUSSURRO_TEST_PARAKEET_DIR=/path/parakeet (the ONNX files)
    /// SUSSURRO_TEST_WAV=/path/03.wav
    /// SUSSURRO_TEST_TAIL="remote data and voice needs" (words of the last sentence)
    /// cargo test parakeet_keeps_sentences_after_a_pause -- --ignored --nocapture
    #[test]
    #[ignore]
    fn parakeet_keeps_sentences_after_a_pause() {
        let dir =
            std::env::var("SUSSURRO_TEST_PARAKEET_DIR").expect("set SUSSURRO_TEST_PARAKEET_DIR");
        let wav = std::env::var("SUSSURRO_TEST_WAV").expect("set SUSSURRO_TEST_WAV");
        let tail = std::env::var("SUSSURRO_TEST_TAIL").expect("set SUSSURRO_TEST_TAIL");
        let audio = crate::audio::decode::decode_to_16k_mono(Path::new(&wav)).unwrap();
        let mut t = ParakeetTranscriber::load(Path::new(&dir)).unwrap();
        let text = t.transcribe(&audio).unwrap();
        println!("transcribe: {text}");
        assert!(
            words(&text).contains(&words(&tail)),
            "lost the last sentence: {text}"
        );
        let timed = t.transcribe_timed(&audio).unwrap();
        println!("transcribe_timed: {}", timed.text);
        assert!(
            words(&timed.text).contains(&words(&tail)),
            "lost the last sentence: {}",
            timed.text
        );
    }

    /// #194 benchmark on the #109 corpus (FLEURS, CC-BY-4.0), with the
    /// app's own segmentation. For each packed ≤ 30 s chunk of one language:
    /// - `unsplit`: the whole chunk in one call (the old hotkey path);
    /// - `dictation`: [`ParakeetTranscriber::transcribe`] (the hotkey path);
    /// - `engine-before`: engine segments with the default segmenter, each
    ///   unsplit (the old long-form path);
    /// - `engine`: engine segments with Parakeet's segmenter, each through
    ///   [`ParakeetTranscriber::transcribe_timed`] (the long-form path).
    ///
    /// Engine modes use Silero when SUSSURRO_TEST_VAD_MODEL is set, else
    /// the energy detector (the app's fallback). Writes one JSON per mode
    /// in the #109 result format (score with its `score.py`):
    /// SUSSURRO_TEST_PARAKEET_DIR=… SUSSURRO_BENCH_ROOT=/path/bench-109
    /// SUSSURRO_BENCH_LANG=en|it SUSSURRO_BENCH_OUT=/path/results-dir
    /// cargo test --release parakeet_fleurs_benchmark -- --ignored --nocapture
    #[test]
    #[ignore]
    fn parakeet_fleurs_benchmark() {
        use crate::engine::segmenter::{
            EnergyDetector, Segmenter, SegmenterParams, SpeechDetector, FRAME,
        };
        let env = |k: &str| std::env::var(k).unwrap_or_else(|_| panic!("set {k}"));
        let dir = env("SUSSURRO_TEST_PARAKEET_DIR");
        let root = std::path::PathBuf::from(env("SUSSURRO_BENCH_ROOT"));
        let lang = env("SUSSURRO_BENCH_LANG");
        let out = std::path::PathBuf::from(env("SUSSURRO_BENCH_OUT"));
        let vad = std::env::var("SUSSURRO_TEST_VAD_MODEL").ok();
        // Optional: another manifest under the root (paths relative to it),
        // a tag for the output names, a subset of the modes.
        let manifest_path = std::env::var("SUSSURRO_BENCH_MANIFEST")
            .unwrap_or_else(|_| "corpus/manifest.json".into());
        let tag = std::env::var("SUSSURRO_BENCH_TAG").unwrap_or_default();
        let modes = std::env::var("SUSSURRO_BENCH_MODES")
            .unwrap_or_else(|_| "unsplit,dictation,engine-before,engine".into());
        let manifest: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(root.join(manifest_path)).unwrap())
                .unwrap();
        let chunks: Vec<(String, f64, Vec<f32>)> = manifest
            .as_array()
            .unwrap()
            .iter()
            .filter(|c| c["lang"] == lang.as_str())
            .map(|c| {
                let path = c["path"].as_str().unwrap().to_string();
                let audio = crate::audio::decode::decode_to_16k_mono(&root.join(&path)).unwrap();
                (path, c["dur"].as_f64().unwrap(), audio)
            })
            .collect();
        let started = std::time::Instant::now();
        let mut t = ParakeetTranscriber::load(Path::new(&dir)).unwrap();
        let load_s = started.elapsed().as_secs_f64();
        let detector = || -> Box<dyn SpeechDetector> {
            match &vad {
                Some(m) => {
                    Box::new(crate::engine::vad::SileroDetector::load(Path::new(m)).unwrap())
                }
                None => Box::new(EnergyDetector::default()),
            }
        };
        let vad_name = detector().name();
        // The engine's segments of one chunk, fed in 1 s pushes.
        let segments = |audio: &[f32], p: SegmenterParams| -> Vec<Vec<f32>> {
            let mut det = detector();
            let mut seg = Segmenter::new(p);
            let mut out = Vec::new();
            let mut padded = audio.to_vec();
            padded.resize(padded.len().div_ceil(FRAME) * FRAME, 0.0);
            for push in padded.chunks(32 * FRAME) {
                let probs = det.probabilities(push).unwrap();
                for (frame, prob) in push.chunks(FRAME).zip(probs) {
                    out.extend(seg.push(frame, prob));
                }
            }
            out.extend(seg.finish());
            out.into_iter().map(|s| s.samples).collect()
        };
        // SUSSURRO_BENCH_SPLIT="soft,pause,tail_speech" (ms)
        // overrides the split for a parameter sweep.
        let split = match std::env::var("SUSSURRO_BENCH_SPLIT") {
            Ok(v) => {
                let v: Vec<u32> = v.split(',').map(|x| x.parse().unwrap()).collect();
                PauseSplit {
                    soft_max_ms: v[0],
                    min_pause_ms: v[1],
                    tail_speech_ms: v[2],
                    ..pauses::PARAKEET
                }
            }
            Err(_) => pauses::PARAKEET,
        };
        let parakeet = SegmenterParams {
            soft_max_ms: Some(split.soft_max_ms),
            soft_silence_ms: split.min_pause_ms,
            ..SegmenterParams::for_engine(&crate::settings::SttEngine::Parakeet)
        };
        for mode in modes.split(',') {
            let mut results = Vec::new();
            for (path, dur, audio) in &chunks {
                let clock = std::time::Instant::now();
                let (hyp, pieces) = match mode {
                    "unsplit" | "dictation" => {
                        t.split = (mode == "dictation").then_some(split);
                        t.decodes = 0;
                        let text = t.transcribe(audio).unwrap();
                        (text, t.decodes)
                    }
                    _ => {
                        let before = mode == "engine-before";
                        t.split = (!before).then_some(split);
                        let p = if before {
                            SegmenterParams::default()
                        } else {
                            parakeet
                        };
                        let segs = segments(audio, p);
                        t.decodes = 0;
                        let texts: Vec<String> = segs
                            .iter()
                            .map(|s| t.transcribe_timed(s).unwrap().text)
                            .collect();
                        (join_texts(&texts), t.decodes)
                    }
                };
                let secs = clock.elapsed().as_secs_f64();
                println!("{mode} {path} ({pieces} pieces): {hyp}");
                results.push(serde_json::json!({
                    "path": path, "dur": dur, "secs": secs, "hyp": hyp, "pieces": pieces,
                }));
            }
            let name = if mode.starts_with("engine") {
                format!("parakeet{tag}-{mode}-{vad_name}-{lang}.json")
            } else {
                format!("parakeet{tag}-{mode}-{lang}.json")
            };
            let o = serde_json::json!({
                "engine": "parakeet", "mode": mode, "vad": vad_name, "lang": lang,
                "load_s": load_s, "chunks": results,
            });
            std::fs::write(out.join(name), serde_json::to_string_pretty(&o).unwrap()).unwrap();
        }
    }
}
