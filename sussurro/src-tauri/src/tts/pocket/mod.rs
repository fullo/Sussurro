//! Pocket TTS (Kyutai; P18) through the app's own `ort` (E16 option a,
//! E22): the four fp32 graphs of the community ONNX export, driven the way
//! spike #236's reference driver does it — which matched PyTorch at
//! temperature 0 (same length, speaker cosine 0.998).
//!
//! Per piece of text:
//!
//! 1. [`prompt::prepare`] (Kyutai's `prepare_text_prompt`) and the token
//!    cap ([`prompt::MAX_TOKENS`]);
//! 2. tokenize ([`tokenizer`]) → `text_conditioner` → text embeddings;
//! 3. `flow_lm_main` from the voice's state ([`voice`]): one pass over the
//!    text, then one step per 80 ms frame — each step gives a conditioning
//!    vector and an end-of-speech logit, and carries its KV cache over;
//! 4. `flow_lm_flow`: one Euler step from Gaussian noise (scaled by the
//!    temperature) to the frame's latent;
//! 5. `mimi_decoder`, streaming: one latent in, 1,920 samples at 24 kHz out.
//!
//! Generation stops a few frames after the end-of-speech logit crosses
//! [`EOS_THRESHOLD`] (not before step 6), or at Kyutai's length estimate
//! `(tokens / 3 + 2) s`. The voice and decoder states restart for every
//! piece, as upstream does.

pub mod bundle;
pub mod prompt;
pub mod tokenizer;
pub mod voice;

use super::engine::TtsEngine;
use anyhow::{anyhow, bail, Context, Result};
use bundle::Bundle;
use ort::memory::Allocator;
use ort::session::{Session, SessionInputValue};
use ort::value::{DynTensor, DynValue, Tensor, TensorElementType};
use std::borrow::Cow;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use tokenizer::Tokenizer;
use voice::{Data, StateTensor};

/// Default sampling temperature (Kyutai's `DEFAULT_TEMPERATURE`).
pub const DEFAULT_TEMPERATURE: f32 = 0.7;
/// End-of-speech logit threshold (Kyutai's `DEFAULT_EOS_THRESHOLD`).
pub const EOS_THRESHOLD: f32 = -4.0;
/// Steps before an end-of-speech signal is believed (spike driver).
const MIN_EOS_STEP: usize = 6;
/// Kyutai's speech-rate estimate for the length cap.
const TOKENS_PER_SECOND: f32 = 3.0;
const GEN_SECONDS_PADDING: f32 = 2.0;
/// Intra-op threads by default: more only oversubscribes the small
/// sequential matmuls of the step loop (the export's own advice).
const MAX_THREADS: usize = 4;

/// Options of a loaded engine.
#[derive(Debug, Clone, Copy)]
pub struct PocketOptions {
    /// Intra-op threads; 0 = up to [`MAX_THREADS`].
    pub threads: usize,
    /// Sampling temperature; 0 = deterministic.
    pub temperature: f32,
    /// Seed of the noise, so the same text reads the same way.
    pub seed: u64,
}

impl Default for PocketOptions {
    fn default() -> Self {
        PocketOptions {
            threads: 0,
            temperature: DEFAULT_TEMPERATURE,
            seed: 42,
        }
    }
}

/// Pocket TTS with one language's model and one voice loaded.
pub struct PocketTts {
    bundle: Bundle,
    tokenizer: Tokenizer,
    cond: Session,
    main: Session,
    flow: Session,
    mimi: Session,
    /// The voice's starting state of `flow_lm_main`, in manifest order.
    voice_state: Vec<DynValue>,
    voice_id: String,
    /// Starting state of the decoder (small: rebuilt per piece).
    mimi_start: Vec<StateTensor>,
    rng: Rng,
    temperature: f32,
}

impl std::fmt::Debug for PocketTts {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PocketTts")
            .field("voice", &self.voice_id)
            .finish_non_exhaustive()
    }
}

fn session(path: &Path, threads: usize) -> Result<Session> {
    Session::builder()
        .map_err(|e| anyhow!("read-aloud model: {e}"))?
        .with_intra_threads(threads)
        .map_err(|e| anyhow!("read-aloud model: {e}"))?
        .with_inter_threads(1)
        .map_err(|e| anyhow!("read-aloud model: {e}"))?
        .commit_from_file(path)
        .map_err(|e| anyhow!("loading {}: {e}", path.display()))
}

/// A tensor with no elements (a 0 dimension): ORT refuses those from raw
/// data, so the default CPU allocator makes it.
fn empty_tensor(ty: TensorElementType, shape: &[usize]) -> Result<DynValue> {
    let shape: Vec<i64> = shape.iter().map(|&d| d as i64).collect();
    DynTensor::new(&Allocator::default(), ty, shape)
        .map(DynValue::from)
        .map_err(|e| anyhow!("empty tensor: {e}"))
}

fn to_value(t: &StateTensor) -> Result<DynValue> {
    if t.shape.contains(&0) {
        let ty = match t.data {
            Data::F32(_) => TensorElementType::Float32,
            Data::I64(_) => TensorElementType::Int64,
            Data::Bool(_) => TensorElementType::Bool,
        };
        return empty_tensor(ty, &t.shape);
    }
    let shape: Vec<i64> = t.shape.iter().map(|&d| d as i64).collect();
    let v = match &t.data {
        Data::F32(v) => Tensor::from_array((shape, v.clone())).map(Tensor::into_dyn),
        Data::I64(v) => Tensor::from_array((shape, v.clone())).map(Tensor::into_dyn),
        Data::Bool(v) => Tensor::from_array((shape, v.clone())).map(Tensor::into_dyn),
    };
    v.map_err(|e| anyhow!("state {}: {e}", t.input_name))
}

fn f32_tensor(shape: &[usize], data: Vec<f32>) -> Result<DynValue> {
    if shape.contains(&0) {
        return empty_tensor(TensorElementType::Float32, shape);
    }
    Tensor::from_array((shape.to_vec(), data))
        .map(Tensor::into_dyn)
        .map_err(|e| anyhow!("tensor: {e}"))
}

impl PocketTts {
    /// Load the model files in `dir` (see [`super::models`]) and the voice
    /// file `voice_path`.
    pub fn load(dir: &Path, voice_path: &Path, opts: PocketOptions) -> Result<PocketTts> {
        let threads = if opts.threads == 0 {
            std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(1)
                .clamp(1, MAX_THREADS)
        } else {
            opts.threads
        };
        let bundle = Bundle::from_file(&dir.join("bundle.json"))?;
        let tokenizer = Tokenizer::from_file(&dir.join("tokenizer.model"))?;
        let mimi_start = voice::initial_state(&bundle.mimi_state_manifest, None)?;
        let mut tts = PocketTts {
            cond: session(&dir.join("text_conditioner.onnx"), threads)?,
            flow: session(&dir.join("flow_lm_flow.onnx"), threads)?,
            mimi: session(&dir.join("mimi_decoder.onnx"), threads)?,
            main: session(&dir.join("flow_lm_main.onnx"), threads)?,
            bundle,
            tokenizer,
            voice_state: Vec::new(),
            voice_id: String::new(),
            mimi_start,
            rng: Rng::new(opts.seed),
            temperature: opts.temperature.max(0.0),
        };
        tts.set_voice(voice_path)?;
        Ok(tts)
    }

    /// Switch to another voice of the same language.
    pub fn set_voice(&mut self, voice_path: &Path) -> Result<()> {
        let tensors = voice::read_safetensors(voice_path)?;
        let state = voice::initial_state(&self.bundle.flow_lm_state_manifest, Some(&tensors))
            .with_context(|| format!("voice {}", voice_path.display()))?;
        self.voice_state = state.iter().map(to_value).collect::<Result<_>>()?;
        self.voice_id = voice_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or_default()
            .to_string();
        Ok(())
    }

    /// The loaded voice's id (its file stem).
    pub fn voice_id(&self) -> &str {
        &self.voice_id
    }

    pub fn tokenizer(&self) -> &Tokenizer {
        &self.tokenizer
    }

    /// Reset the noise, so the next text reads as a fresh engine would.
    pub fn reseed(&mut self, seed: u64) {
        self.rng = Rng::new(seed);
    }

    /// One `flow_lm_main` run. `state` = `None` starts from the voice.
    fn run_main(
        &mut self,
        sequence: DynValue,
        text: DynValue,
        state: Option<Vec<DynValue>>,
    ) -> Result<(DynValue, f32, Vec<DynValue>)> {
        let manifest = &self.bundle.flow_lm_state_manifest;
        let mut inputs: Vec<(Cow<'_, str>, SessionInputValue<'_>)> =
            Vec::with_capacity(manifest.len() + 2);
        inputs.push(("sequence".into(), sequence.into()));
        inputs.push(("text_embeddings".into(), text.into()));
        match state {
            Some(values) => {
                for (e, v) in manifest.iter().zip(values) {
                    inputs.push((e.input_name.as_str().into(), v.into()));
                }
            }
            None => {
                for (e, v) in manifest.iter().zip(&self.voice_state) {
                    inputs.push((e.input_name.as_str().into(), v.into()));
                }
            }
        }
        let mut out = self
            .main
            .run(inputs)
            .map_err(|e| anyhow!("read-aloud model (main): {e}"))?;
        let cond = out
            .remove("conditioning")
            .context("no conditioning output")?;
        let eos = {
            let v = out.get("eos_logit").context("no eos output")?;
            let (_, d) = v
                .try_extract_tensor::<f32>()
                .map_err(|e| anyhow!("eos output: {e}"))?;
            *d.first().context("empty eos output")?
        };
        let state = manifest
            .iter()
            .map(|e| {
                out.remove(&e.output_name)
                    .with_context(|| format!("no {}", e.output_name))
            })
            .collect::<Result<Vec<_>>>()?;
        Ok((cond, eos, state))
    }

    /// Latent of one frame: noise + one Euler step of the flow.
    fn run_flow(&mut self, cond: DynValue) -> Result<Vec<f32>> {
        let dim = self.bundle.latent_dim;
        let std = self.temperature.sqrt();
        let noise: Vec<f32> = (0..dim)
            .map(|_| {
                if std > 0.0 {
                    self.rng.normal() * std
                } else {
                    0.0
                }
            })
            .collect();
        let out = self
            .flow
            .run(vec![
                ("c", SessionInputValue::from(cond)),
                ("s", f32_tensor(&[1, 1], vec![0.0])?.into()),
                ("t", f32_tensor(&[1, 1], vec![1.0])?.into()),
                ("x", f32_tensor(&[1, dim], noise.clone())?.into()),
            ])
            .map_err(|e| anyhow!("read-aloud model (flow): {e}"))?;
        let (_, dir) = out[0]
            .try_extract_tensor::<f32>()
            .map_err(|e| anyhow!("flow output: {e}"))?;
        if dir.len() != dim {
            bail!("flow returned {} values, expected {dim}", dir.len());
        }
        Ok(noise.iter().zip(dir).map(|(x, v)| x + v).collect())
    }

    /// Decode one latent into audio.
    fn run_mimi(
        &mut self,
        latent: &[f32],
        state: Vec<DynValue>,
    ) -> Result<(Vec<f32>, Vec<DynValue>)> {
        let manifest = &self.bundle.mimi_state_manifest;
        let mut inputs: Vec<(Cow<'_, str>, SessionInputValue<'_>)> =
            Vec::with_capacity(manifest.len() + 1);
        inputs.push((
            "latent".into(),
            f32_tensor(&[1, 1, latent.len()], latent.to_vec())?.into(),
        ));
        for (e, v) in manifest.iter().zip(state) {
            inputs.push((e.input_name.as_str().into(), v.into()));
        }
        let mut out = self
            .mimi
            .run(inputs)
            .map_err(|e| anyhow!("read-aloud model (decoder): {e}"))?;
        let audio = {
            let (_, a) = out["audio_frame"]
                .try_extract_tensor::<f32>()
                .map_err(|e| anyhow!("decoder output: {e}"))?;
            a.to_vec()
        };
        let state = manifest
            .iter()
            .map(|e| {
                out.remove(&e.output_name)
                    .with_context(|| format!("no {}", e.output_name))
            })
            .collect::<Result<Vec<_>>>()?;
        Ok((audio, state))
    }

    /// Generate one prepared prompt of at most [`prompt::MAX_TOKENS`].
    fn generate(
        &mut self,
        text: &str,
        frames_after_eos: usize,
        cancel: &AtomicBool,
        out: &mut dyn FnMut(&[f32]) -> Result<()>,
    ) -> Result<()> {
        let ids: Vec<i64> = self
            .tokenizer
            .encode(text)
            .into_iter()
            .map(i64::from)
            .collect();
        if ids.is_empty() {
            return Ok(());
        }
        let n_tokens = ids.len();
        let embeddings = {
            let ids = Tensor::from_array((vec![1usize, n_tokens], ids))
                .map_err(|e| anyhow!("tokens: {e}"))?;
            let mut o = self
                .cond
                .run(vec![("token_ids", SessionInputValue::from(ids))])
                .map_err(|e| anyhow!("read-aloud model (text): {e}"))?;
            o.remove("embeddings").context("no text embeddings")?
        };
        let latent_dim = self.bundle.latent_dim;
        let cond_dim = self.bundle.conditioning_dim;

        // Prompt the text on top of the voice.
        let (_, _, mut state) = self.run_main(
            f32_tensor(&[1, 0, latent_dim], Vec::new())?,
            embeddings,
            None,
        )?;
        let mut mimi_state: Vec<DynValue> = self
            .mimi_start
            .iter()
            .map(to_value)
            .collect::<Result<_>>()?;

        let max_len = ((n_tokens as f32 / TOKENS_PER_SECOND + GEN_SECONDS_PADDING)
            * self.bundle.frame_rate)
            .ceil() as usize;
        let mut seq = vec![f32::NAN; latent_dim];
        let mut eos_step = None;
        for step in 0..max_len {
            if cancel.load(Ordering::Relaxed) {
                bail!("reading cancelled");
            }
            let (cond, eos, next) = self.run_main(
                f32_tensor(&[1, 1, latent_dim], std::mem::take(&mut seq))?,
                f32_tensor(&[1, 0, cond_dim], Vec::new())?,
                Some(std::mem::take(&mut state)),
            )?;
            state = next;
            if eos_step.is_none() && eos > EOS_THRESHOLD && step >= MIN_EOS_STEP {
                eos_step = Some(step);
            }
            if eos_step.is_some_and(|e| step >= e + frames_after_eos) {
                break;
            }
            let latent = self.run_flow(cond)?;
            let (audio, next) = self.run_mimi(&latent, std::mem::take(&mut mimi_state))?;
            mimi_state = next;
            out(&audio)?;
            seq = latent;
        }
        if eos_step.is_none() {
            eprintln!("read-aloud: a piece reached its length cap without end of speech");
        }
        Ok(())
    }
}

impl TtsEngine for PocketTts {
    fn sample_rate(&self) -> u32 {
        self.bundle.sample_rate
    }

    fn speak(
        &mut self,
        text: &str,
        cancel: &AtomicBool,
        out: &mut dyn FnMut(&[f32]) -> Result<()>,
    ) -> Result<()> {
        let pieces = {
            let tok = &self.tokenizer;
            prompt::split_to_tokens(text, prompt::MAX_TOKENS, &|s| tok.count(s))
        };
        for piece in pieces {
            let Some((prepared, guess)) = prompt::prepare(
                &piece,
                self.bundle.pad_with_spaces_for_short_inputs,
                self.bundle.remove_semicolons,
            ) else {
                continue;
            };
            let after = self
                .bundle
                .model_recommended_frames_after_eos
                .unwrap_or(guess);
            self.generate(&prepared, after, cancel, out)?;
        }
        Ok(())
    }
}

/// SplitMix64 + Box–Muller: reproducible Gaussian noise without a crate.
#[derive(Debug, Clone)]
struct Rng {
    state: u64,
    spare: Option<f32>,
}

impl Rng {
    fn new(seed: u64) -> Rng {
        Rng {
            state: seed,
            spare: None,
        }
    }

    fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform in (0, 1].
    fn uniform(&mut self) -> f64 {
        ((self.next_u64() >> 11) as f64 + 1.0) / (1u64 << 53) as f64
    }

    fn normal(&mut self) -> f32 {
        if let Some(s) = self.spare.take() {
            return s;
        }
        let (u1, u2) = (self.uniform(), self.uniform());
        let r = (-2.0 * u1.ln()).sqrt();
        let t = std::f64::consts::TAU * u2;
        self.spare = Some((r * t.sin()) as f32);
        (r * t.cos()) as f32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn noise_is_standard_normal_and_reproducible() {
        let mut a = Rng::new(42);
        let mut b = Rng::new(42);
        let xs: Vec<f32> = (0..20_000).map(|_| a.normal()).collect();
        let ys: Vec<f32> = (0..20_000).map(|_| b.normal()).collect();
        assert_eq!(xs, ys);
        let mean = xs.iter().map(|&x| f64::from(x)).sum::<f64>() / xs.len() as f64;
        let var = xs
            .iter()
            .map(|&x| (f64::from(x) - mean).powi(2))
            .sum::<f64>()
            / xs.len() as f64;
        assert!(mean.abs() < 0.03, "mean {mean}");
        assert!((var - 1.0).abs() < 0.05, "variance {var}");
        assert!(xs.iter().all(|x| x.is_finite()));
        assert_ne!(Rng::new(1).normal(), Rng::new(2).normal());
    }
}
