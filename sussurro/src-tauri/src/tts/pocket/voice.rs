//! Voices and the models' starting state.
//!
//! A Pocket voice is not audio: it is the language model's attention cache
//! after it has "listened" to a few seconds of the speaker (a prompt
//! state), saved by Kyutai as a `.safetensors` file with, per layer,
//! `<module>/cache` (`[2, 1, frames, heads, dim]`) and `<module>/offset`.
//! The ONNX graph wants a fixed 1,000-frame cache, NaN where empty, plus
//! a `step` counter — so the voice is copied into the start of that cache
//! (what the spike's reference driver does).
//!
//! The safetensors reader is a few lines on purpose (no new crate): an
//! 8-byte header length, a JSON header, raw little-endian data.

use super::bundle::{DType, Fill, StateEntry};
use anyhow::{bail, Context, Result};
use std::collections::HashMap;
use std::path::Path;

/// A state tensor's values.
#[derive(Debug, Clone, PartialEq)]
pub enum Data {
    F32(Vec<f32>),
    I64(Vec<i64>),
    Bool(Vec<bool>),
}

/// One named state tensor, ready to become a model input.
#[derive(Debug, Clone, PartialEq)]
pub struct StateTensor {
    pub input_name: String,
    pub shape: Vec<usize>,
    pub data: Data,
}

/// One tensor of a safetensors file.
#[derive(Debug, Clone, PartialEq)]
pub struct Tensor {
    pub shape: Vec<usize>,
    pub data: Data,
}

/// Largest voice file accepted (the pinned ones are ≤ 33 MB).
const MAX_VOICE_BYTES: u64 = 128 * 1024 * 1024;

/// Read a safetensors file (F32, BF16, F16 → f32; I64).
pub fn read_safetensors(path: &Path) -> Result<HashMap<String, Tensor>> {
    let len = std::fs::metadata(path)
        .with_context(|| format!("reading {}", path.display()))?
        .len();
    if len > MAX_VOICE_BYTES {
        bail!("{} is too large for a voice", path.display());
    }
    let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    parse_safetensors(&bytes)
}

pub fn parse_safetensors(bytes: &[u8]) -> Result<HashMap<String, Tensor>> {
    let head_len = bytes
        .get(..8)
        .map(|b| u64::from_le_bytes(b.try_into().unwrap_or([0; 8])))
        .context("not a safetensors file")?;
    let head_end = 8usize
        .checked_add(usize::try_from(head_len).context("header too long")?)
        .filter(|&e| e <= bytes.len())
        .context("truncated safetensors header")?;
    let header: HashMap<String, serde_json::Value> =
        serde_json::from_slice(&bytes[8..head_end]).context("bad safetensors header")?;
    let data = &bytes[head_end..];
    let mut out = HashMap::new();
    for (name, v) in header {
        if name == "__metadata__" {
            continue;
        }
        let dtype = v["dtype"].as_str().context("tensor without dtype")?;
        let shape: Vec<usize> = serde_json::from_value(v["shape"].clone()).context("bad shape")?;
        let offs: Vec<usize> =
            serde_json::from_value(v["data_offsets"].clone()).context("bad offsets")?;
        let [start, end] = offs[..] else {
            bail!("bad offsets for {name}");
        };
        let raw = data
            .get(start..end)
            .with_context(|| format!("{name} lies outside the file"))?;
        let n: usize = shape.iter().product();
        let width = match dtype {
            "F32" | "I32" => 4,
            "I64" => 8,
            "BF16" | "F16" => 2,
            other => bail!("{name}: unsupported dtype {other}"),
        };
        if raw.len() != n * width {
            bail!("{name}: {} bytes for {n} values", raw.len());
        }
        let data = match dtype {
            "F32" => Data::F32(
                raw.chunks_exact(4)
                    .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
                    .collect(),
            ),
            "BF16" => Data::F32(
                raw.chunks_exact(2)
                    .map(|b| f32::from_bits(u32::from(u16::from_le_bytes([b[0], b[1]])) << 16))
                    .collect(),
            ),
            "F16" => Data::F32(
                raw.chunks_exact(2)
                    .map(|b| f16_to_f32(u16::from_le_bytes([b[0], b[1]])))
                    .collect(),
            ),
            "I32" => Data::I64(
                raw.chunks_exact(4)
                    .map(|b| i64::from(i32::from_le_bytes([b[0], b[1], b[2], b[3]])))
                    .collect(),
            ),
            _ => Data::I64(
                raw.chunks_exact(8)
                    .map(|b| i64::from_le_bytes(b.try_into().unwrap_or([0; 8])))
                    .collect(),
            ),
        };
        out.insert(name, Tensor { shape, data });
    }
    Ok(out)
}

fn f16_to_f32(h: u16) -> f32 {
    let sign = u32::from(h >> 15) << 31;
    let exp = u32::from((h >> 10) & 0x1f);
    let frac = u32::from(h & 0x3ff);
    let bits = match (exp, frac) {
        (0, 0) => sign,
        (0, f) => {
            // Subnormal: normalise.
            let mut e = 127 - 15 + 1;
            let mut f = f;
            while f & 0x400 == 0 {
                f <<= 1;
                e -= 1;
            }
            sign | (e << 23) | ((f & 0x3ff) << 13)
        }
        (31, f) => sign | 0x7f80_0000 | (f << 13),
        (e, f) => sign | ((e + 127 - 15) << 23) | (f << 13),
    };
    f32::from_bits(bits)
}

/// Starting values of the state tensors in `manifest`. With a `voice`,
/// every `cache` gets the voice's frames at its start and every `step` the
/// voice's offset (the flow model); without, the manifest's fills (the
/// Mimi decoder).
pub fn initial_state(
    manifest: &[StateEntry],
    voice: Option<&HashMap<String, Tensor>>,
) -> Result<Vec<StateTensor>> {
    manifest
        .iter()
        .map(|e| {
            let data = match (voice, e.key.as_str()) {
                (Some(v), "cache") => Data::F32(voice_cache(e, v)?),
                (Some(v), "step") => {
                    let name = format!("{}/offset", e.module());
                    let offset = match v.get(&name).map(|t| &t.data) {
                        Some(Data::I64(o)) if !o.is_empty() => o[0],
                        _ => bail!("the voice has no {name}"),
                    };
                    Data::I64(vec![offset; e.elements()])
                }
                _ => filled(e),
            };
            Ok(StateTensor {
                input_name: e.input_name.clone(),
                shape: e.shape.clone(),
                data,
            })
        })
        .collect()
}

fn filled(e: &StateEntry) -> Data {
    let n = e.elements();
    match (e.dtype, e.fill) {
        (DType::Float32, Fill::Nan) => Data::F32(vec![f32::NAN; n]),
        (DType::Float32, Fill::Ones) => Data::F32(vec![1.0; n]),
        (DType::Float32, _) => Data::F32(vec![0.0; n]),
        (DType::Int64, Fill::Ones) => Data::I64(vec![1; n]),
        (DType::Int64, _) => Data::I64(vec![0; n]),
        (DType::Bool, Fill::Ones) => Data::Bool(vec![true; n]),
        (DType::Bool, _) => Data::Bool(vec![false; n]),
    }
}

/// The manifest's NaN cache `[2, 1, T, H, D]` with the voice's
/// `[2, 1, L, H, D]` frames copied into `[:, :, :L]`.
fn voice_cache(e: &StateEntry, voice: &HashMap<String, Tensor>) -> Result<Vec<f32>> {
    let name = format!("{}/cache", e.module());
    let t = voice
        .get(&name)
        .with_context(|| format!("the voice has no {name} — it belongs to another model"))?;
    let Data::F32(src) = &t.data else {
        bail!("{name} is not a float tensor");
    };
    let (want, got) = (&e.shape, &t.shape);
    if want.len() != 5 || got.len() != 5 || want[..2] != got[..2] || want[3..] != got[3..] {
        bail!("{name} has shape {got:?}, the model wants {want:?} — another model's voice");
    }
    let (total, frames) = (want[2], got[2]);
    // Leave room for the text and the speech this voice will be asked for.
    if frames + 300 > total {
        bail!("{name} is too long ({frames} frames of {total})");
    }
    let row = want[3] * want[4];
    let mut out = vec![f32::NAN; e.elements()];
    for k in 0..want[0] * want[1] {
        let dst = k * total * row;
        let from = k * frames * row;
        out[dst..dst + frames * row].copy_from_slice(&src[from..from + frames * row]);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Serialize tensors as safetensors (tests only).
    pub(crate) fn safetensors(tensors: &[(&str, &str, Vec<usize>, Vec<u8>)]) -> Vec<u8> {
        let mut header = serde_json::Map::new();
        let mut data = Vec::new();
        for (name, dtype, shape, raw) in tensors {
            let start = data.len();
            data.extend_from_slice(raw);
            header.insert(
                name.to_string(),
                serde_json::json!({"dtype": dtype, "shape": shape, "data_offsets": [start, data.len()]}),
            );
        }
        let head = serde_json::to_vec(&header).unwrap();
        let mut out = (head.len() as u64).to_le_bytes().to_vec();
        out.extend(head);
        out.extend(data);
        out
    }

    fn f32s(v: &[f32]) -> Vec<u8> {
        v.iter().flat_map(|x| x.to_le_bytes()).collect()
    }

    fn entry(
        input: &str,
        path: &str,
        key: &str,
        dtype: DType,
        fill: Fill,
        shape: Vec<usize>,
    ) -> StateEntry {
        StateEntry {
            input_name: input.into(),
            output_name: format!("out_{input}"),
            path: path.into(),
            key: key.into(),
            dtype,
            fill,
            shape,
        }
    }

    #[test]
    fn safetensors_are_read_with_their_types() {
        let bytes = safetensors(&[
            ("a/cache", "F32", vec![2], f32s(&[1.5, -2.0])),
            ("a/offset", "I64", vec![1], 7i64.to_le_bytes().to_vec()),
            ("b", "BF16", vec![1], vec![0x80, 0x3f]), // 1.0
            ("c", "F16", vec![2], vec![0x00, 0x3c, 0x00, 0xc0]), // 1.0, -2.0
        ]);
        let t = parse_safetensors(&bytes).unwrap();
        assert_eq!(t["a/cache"].data, Data::F32(vec![1.5, -2.0]));
        assert_eq!(t["a/offset"].data, Data::I64(vec![7]));
        assert_eq!(t["b"].data, Data::F32(vec![1.0]));
        assert_eq!(t["c"].data, Data::F32(vec![1.0, -2.0]));
        assert_eq!(f16_to_f32(0x0001), 2f32.powi(-24), "subnormal");
        assert!(f16_to_f32(0x7c00).is_infinite());
    }

    #[test]
    fn broken_safetensors_are_refused() {
        assert!(parse_safetensors(b"").is_err());
        assert!(parse_safetensors(&[255, 0, 0, 0, 0, 0, 0, 0, b'{']).is_err());
        let short = safetensors(&[("x", "F32", vec![4], f32s(&[1.0]))]);
        assert!(parse_safetensors(&short).is_err(), "4 values, 1 stored");
        let odd = safetensors(&[("x", "U8", vec![1], vec![1])]);
        assert!(parse_safetensors(&odd).is_err());
    }

    #[test]
    fn the_voice_fills_the_start_of_the_cache() {
        let m = "t.layers.0.attn";
        let manifest = [
            entry(
                "state_0",
                &format!("{m}/cache"),
                "cache",
                DType::Float32,
                Fill::Nan,
                vec![2, 1, 400, 1, 2],
            ),
            entry(
                "state_1",
                &format!("{m}/current_end"),
                "current_end",
                DType::Float32,
                Fill::Empty,
                vec![0],
            ),
            entry(
                "state_2",
                &format!("{m}/step"),
                "step",
                DType::Int64,
                Fill::Zeros,
                vec![1],
            ),
        ];
        // Two frames of voice: k=0 → 1,2,3,4 ; k=1 → 5,6,7,8.
        let voice = parse_safetensors(&safetensors(&[
            (
                &format!("{m}/cache"),
                "F32",
                vec![2, 1, 2, 1, 2],
                f32s(&[1., 2., 3., 4., 5., 6., 7., 8.]),
            ),
            (
                &format!("{m}/offset"),
                "I64",
                vec![1],
                2i64.to_le_bytes().to_vec(),
            ),
        ]))
        .unwrap();
        let st = initial_state(&manifest, Some(&voice)).unwrap();
        let Data::F32(cache) = &st[0].data else {
            panic!()
        };
        assert_eq!(cache.len(), 2 * 400 * 2);
        assert_eq!(&cache[..4], &[1., 2., 3., 4.]);
        assert!(cache[4].is_nan());
        assert_eq!(&cache[800..804], &[5., 6., 7., 8.]);
        assert!(cache[804..].iter().all(|x| x.is_nan()));
        assert_eq!(st[1].data, Data::F32(vec![]));
        assert_eq!(st[2].data, Data::I64(vec![2]));
    }

    #[test]
    fn a_voice_for_another_model_is_refused() {
        let m = "t.layers.0.attn";
        let manifest = [entry(
            "state_0",
            &format!("{m}/cache"),
            "cache",
            DType::Float32,
            Fill::Nan,
            vec![2, 1, 400, 2, 2],
        )];
        let wrong_heads = parse_safetensors(&safetensors(&[(
            &format!("{m}/cache"),
            "F32",
            vec![2, 1, 1, 1, 2],
            f32s(&[0.; 4]),
        )]))
        .unwrap();
        assert!(initial_state(&manifest, Some(&wrong_heads)).is_err());
        let other_layer = parse_safetensors(&safetensors(&[(
            "t.layers.9.attn/cache",
            "F32",
            vec![1],
            f32s(&[0.]),
        )]))
        .unwrap();
        let err = initial_state(&manifest, Some(&other_layer)).unwrap_err();
        assert!(err.to_string().contains("another model"), "{err}");
    }

    #[test]
    fn without_a_voice_the_fills_apply() {
        let manifest = [
            entry(
                "state_0",
                "d.0/first",
                "first",
                DType::Bool,
                Fill::Ones,
                vec![1],
            ),
            entry(
                "state_1",
                "d.0/previous",
                "previous",
                DType::Float32,
                Fill::Zeros,
                vec![1, 2, 2],
            ),
            entry(
                "state_2",
                "d.0/previous",
                "previous",
                DType::Float32,
                Fill::Empty,
                vec![1, 2, 0],
            ),
            entry(
                "state_3",
                "d.0/offset",
                "offset",
                DType::Int64,
                Fill::Zeros,
                vec![1],
            ),
        ];
        let st = initial_state(&manifest, None).unwrap();
        assert_eq!(st[0].data, Data::Bool(vec![true]));
        assert_eq!(st[1].data, Data::F32(vec![0.0; 4]));
        assert_eq!(st[2].data, Data::F32(vec![]));
        assert_eq!(st[3].data, Data::I64(vec![0]));
    }
}
