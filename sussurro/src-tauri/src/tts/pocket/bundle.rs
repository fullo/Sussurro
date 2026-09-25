//! The export's `bundle.json`: sample rate, frame rate and the manifests of
//! the recurrent state the `flow_lm_main` and `mimi_decoder` graphs carry
//! from one step to the next (input `state_N` → output `out_state_N`).

use anyhow::{bail, Context, Result};
use serde::Deserialize;
use std::path::Path;

#[derive(Debug, Clone, Deserialize)]
pub struct Bundle {
    pub sample_rate: u32,
    pub frame_rate: f32,
    pub samples_per_frame: usize,
    pub latent_dim: usize,
    pub conditioning_dim: usize,
    #[serde(default)]
    pub pad_with_spaces_for_short_inputs: bool,
    #[serde(default)]
    pub remove_semicolons: bool,
    /// Frames to keep generating after the end-of-speech signal, when the
    /// model recommends a number (else Kyutai's guess from the text).
    #[serde(default)]
    pub model_recommended_frames_after_eos: Option<usize>,
    pub flow_lm_state_manifest: Vec<StateEntry>,
    pub mimi_state_manifest: Vec<StateEntry>,
}

/// One state tensor.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct StateEntry {
    pub input_name: String,
    pub output_name: String,
    /// `<module>/<key>`, the name of the matching tensor in a voice file.
    pub path: String,
    pub key: String,
    pub dtype: DType,
    pub fill: Fill,
    pub shape: Vec<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DType {
    Float32,
    Int64,
    Bool,
}

/// Initial value of a state tensor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Fill {
    Nan,
    Zeros,
    Ones,
    /// A tensor with no elements (one dimension is 0).
    Empty,
}

impl StateEntry {
    pub fn elements(&self) -> usize {
        self.shape.iter().product()
    }

    /// The module part of [`Self::path`].
    pub fn module(&self) -> &str {
        self.path.rsplit_once('/').map_or("", |(m, _)| m)
    }
}

impl Bundle {
    pub fn from_file(path: &Path) -> Result<Bundle> {
        let text =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        Bundle::parse(&text)
    }

    pub fn parse(json: &str) -> Result<Bundle> {
        let b: Bundle = serde_json::from_str(json).context("bundle.json is not a Pocket bundle")?;
        if b.sample_rate == 0 || b.frame_rate <= 0.0 || b.samples_per_frame == 0 {
            bail!("bundle.json: bad rates");
        }
        for e in b
            .flow_lm_state_manifest
            .iter()
            .chain(&b.mimi_state_manifest)
        {
            if e.fill == Fill::Empty && e.elements() != 0 {
                bail!("bundle.json: {} is 'empty' but has elements", e.input_name);
            }
            if e.shape.is_empty() || e.elements() > 64 * 1024 * 1024 {
                bail!("bundle.json: {} has an unusable shape", e.input_name);
            }
        }
        Ok(b)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) const MINI: &str = r#"{
      "sample_rate": 24000, "frame_rate": 12.5, "samples_per_frame": 1920,
      "latent_dim": 32, "conditioning_dim": 1024,
      "pad_with_spaces_for_short_inputs": false, "remove_semicolons": false,
      "model_recommended_frames_after_eos": null,
      "flow_lm_state_manifest": [
        {"dtype":"float32","fill":"nan","index":0,"input_name":"state_0","key":"cache",
         "module":"transformer.layers.0.self_attn","output_name":"out_state_0",
         "path":"transformer.layers.0.self_attn/cache","shape":[2,1,1000,16,64]},
        {"dtype":"float32","fill":"empty","index":1,"input_name":"state_1","key":"current_end",
         "module":"transformer.layers.0.self_attn","output_name":"out_state_1",
         "path":"transformer.layers.0.self_attn/current_end","shape":[0]},
        {"dtype":"int64","fill":"zeros","index":2,"input_name":"state_2","key":"step",
         "module":"transformer.layers.0.self_attn","output_name":"out_state_2",
         "path":"transformer.layers.0.self_attn/step","shape":[1]}
      ],
      "mimi_state_manifest": [
        {"dtype":"bool","fill":"ones","index":0,"input_name":"state_0","key":"first",
         "module":"decoder.model.0","output_name":"out_state_0",
         "path":"decoder.model.0/first","shape":[1]}
      ],
      "predefined_voices": ["alba"]
    }"#;

    #[test]
    fn a_bundle_parses_with_its_manifests() {
        let b = Bundle::parse(MINI).unwrap();
        assert_eq!(b.sample_rate, 24_000);
        assert_eq!(b.samples_per_frame, 1920);
        assert_eq!(b.model_recommended_frames_after_eos, None);
        let e = &b.flow_lm_state_manifest[0];
        assert_eq!(
            (e.fill, e.dtype, e.elements()),
            (Fill::Nan, DType::Float32, 2_048_000)
        );
        assert_eq!(e.module(), "transformer.layers.0.self_attn");
        assert_eq!(b.flow_lm_state_manifest[1].fill, Fill::Empty);
        assert_eq!(b.mimi_state_manifest[0].dtype, DType::Bool);
    }

    #[test]
    fn broken_bundles_are_refused() {
        assert!(Bundle::parse("{}").is_err());
        let bad = MINI.replace(r#""shape":[0]"#, r#""shape":[3]"#);
        assert!(Bundle::parse(&bad).is_err(), "empty fill with elements");
        let bad = MINI.replace(r#""sample_rate": 24000"#, r#""sample_rate": 0"#);
        assert!(Bundle::parse(&bad).is_err());
        let bad = MINI.replace(r#""dtype":"bool""#, r#""dtype":"float16""#);
        assert!(Bundle::parse(&bad).is_err(), "unknown dtype");
    }
}
