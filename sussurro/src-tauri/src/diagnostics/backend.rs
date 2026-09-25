//! Which compute backend the STT engine (and the sidecars) actually
//! initialised (#101) — read from what the libraries logged while loading,
//! not guessed from compile features: a Metal/Vulkan build still falls
//! back to the CPU when the GPU is missing or fails to initialise.
//!
//! whisper.cpp logs, each time it sets up a transcription state
//! (`whisper_backend_init_gpu`):
//! `device 0: MTL0 (type: 1)`, then either `using MTL0 backend` or
//! `no GPU found`, and `failed to initialize … backend` when the chosen
//! device then fails. `llama-server` (Qwen3-ASR, the bundled LLM) writes
//! `llama_model_load_from_file_impl: using device MTL0 (Apple M2) …` per
//! GPU it uses and `load_tensors: offloaded 29/29 layers to GPU`.

use serde::Serialize;
use std::sync::Mutex;

/// What the engine runs on.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ComputeBackend {
    /// "Metal", "Vulkan", "CUDA", "CPU"…
    pub kind: String,
    /// The ggml device name (`MTL0`, `Vulkan0`…) and/or its description
    /// (`Apple M2`, `NVIDIA GeForce RTX 3060`), when known.
    pub device: Option<String>,
    /// How we know: `whisper.cpp log`, `llama-server log`, `engine`
    /// (fixed by the engine), or `build (not confirmed)`.
    pub source: String,
    /// Extra facts (layers offloaded, a GPU that failed to start).
    pub note: Option<String>,
}

impl ComputeBackend {
    pub fn gpu(&self) -> bool {
        self.kind != "CPU"
    }
}

/// The backend family of a ggml device name. Pure.
pub fn kind_of(device: &str) -> String {
    let d = device.trim();
    let lower = d.to_ascii_lowercase();
    if lower.starts_with("mtl") || lower.starts_with("metal") {
        "Metal".into()
    } else if lower.starts_with("vulkan") {
        "Vulkan".into()
    } else if lower.starts_with("cuda") {
        "CUDA".into()
    } else if lower.starts_with("rocm") || lower.starts_with("hip") {
        "ROCm".into()
    } else if lower.starts_with("sycl") {
        "SYCL".into()
    } else if lower.starts_with("cpu") {
        "CPU".into()
    } else {
        d.to_string()
    }
}

/// Parse the backend lines whisper.cpp logged. It logs a block each time
/// it sets up compute backends — for every transcription state and for
/// the VAD model, not when the model file loads — so the latest block
/// wins. `None` when the lines say nothing (logging not captured, no
/// transcription yet). Pure.
pub fn parse_whisper_log<S: AsRef<str>>(lines: &[S]) -> Option<ComputeBackend> {
    let mut chosen: Option<String> = None;
    let mut failed: Option<String> = None;
    let mut no_gpu = false;
    let mut accel: Vec<String> = Vec::new();
    for line in lines {
        let line = line.as_ref().trim();
        if let Some(rest) = line.strip_prefix("whisper_backend_init_gpu:") {
            let rest = rest.trim();
            if rest.starts_with("device 0:") {
                // The first line of a new block (GPU requested).
                chosen = None;
                failed = None;
                no_gpu = false;
                accel.clear();
            } else if rest.starts_with("no GPU found") {
                chosen = None;
                failed = None;
                no_gpu = true;
                accel.clear();
            } else if let Some(name) = between(rest, "failed to initialize ", " backend") {
                failed = Some(name.to_string());
            } else if let Some(name) = between(rest, "using ", " backend") {
                chosen = Some(name.to_string());
                failed = None;
                no_gpu = false;
                accel.clear();
            }
        } else if let Some(rest) = line.strip_prefix("whisper_backend_init:") {
            // ACCEL devices (BLAS…) run next to the main one.
            if let Some(name) = between(rest.trim(), "using ", " backend") {
                if !accel.iter().any(|a| a == name) {
                    accel.push(name.to_string());
                }
            }
        }
    }
    let note = |extra: Option<String>| {
        let mut parts: Vec<String> = extra.into_iter().collect();
        if !accel.is_empty() {
            parts.push(format!("with {}", accel.join(", ")));
        }
        (!parts.is_empty()).then(|| parts.join("; "))
    };
    let source = "whisper.cpp log".to_string();
    match (chosen, failed, no_gpu) {
        (Some(dev), Some(bad), _) if bad == dev => Some(ComputeBackend {
            kind: "CPU".into(),
            device: None,
            source,
            note: note(Some(format!("{} ({dev}) failed to initialise", kind_of(&dev)))),
        }),
        (Some(dev), _, _) => Some(ComputeBackend {
            kind: kind_of(&dev),
            device: Some(dev),
            source,
            note: note(None),
        }),
        (None, _, true) => Some(ComputeBackend {
            kind: "CPU".into(),
            device: None,
            source,
            note: note(Some("no GPU found".into())),
        }),
        _ => None,
    }
}

/// Parse a `llama-server` log (the last run in it wins). `None` when the
/// model has not been loaded yet. Pure.
pub fn parse_llama_log(log: &str) -> Option<ComputeBackend> {
    let mut devices: Vec<String> = Vec::new();
    let mut offload: Option<String> = None;
    let mut loaded = false;
    for line in log.lines() {
        let line = line.trim();
        if line.contains("llama_model_load_from_file_impl:") && line.contains("using device ") {
            if loaded {
                // A new run of the server after a finished load: start over.
                devices.clear();
                offload = None;
                loaded = false;
            }
            if let Some(rest) = line.split("using device ").nth(1) {
                let name = rest.split_whitespace().next().unwrap_or("").to_string();
                let desc = between(rest, "(", ")").map(str::to_string);
                if !name.is_empty() {
                    devices.push(match desc {
                        Some(d) if !d.is_empty() => format!("{name}, {d}"),
                        _ => name,
                    });
                }
            }
        } else if line.contains("load_tensors:") {
            loaded = true;
            if let Some(rest) = line.split("offloaded ").nth(1) {
                if let Some(frac) = rest.split_whitespace().next() {
                    offload = Some(format!("{frac} layers on the GPU"));
                }
            }
        }
    }
    if devices.is_empty() && !loaded {
        return None;
    }
    let source = "llama-server log".to_string();
    match devices.first() {
        Some(first) => Some(ComputeBackend {
            kind: kind_of(first.split(',').next().unwrap_or(first)),
            device: Some(devices.join(" + ")),
            source,
            note: offload,
        }),
        None => Some(ComputeBackend {
            kind: "CPU".into(),
            device: None,
            source,
            note: offload,
        }),
    }
}

/// What this build asks whisper.cpp for, when the log said nothing.
pub fn whisper_build_guess() -> ComputeBackend {
    let kind = if cfg!(target_os = "macos") {
        "Metal"
    } else if cfg!(windows) || cfg!(feature = "linux-vulkan") {
        "Vulkan"
    } else {
        "CPU"
    };
    ComputeBackend {
        kind: kind.into(),
        device: None,
        source: "build (not confirmed)".into(),
        note: None,
    }
}

/// ggml's description of a device whisper.cpp named (`MTL0` → `Apple M2`),
/// from the live device registry. `None` when no device has that name.
pub fn ggml_device_description(name: &str) -> Option<String> {
    use whisper_rs::whisper_rs_sys as sys;
    // SAFETY: the registry is static after ggml's first use; names and
    // descriptions are NUL-terminated strings owned by it.
    unsafe {
        for i in 0..sys::ggml_backend_dev_count() {
            let dev = sys::ggml_backend_dev_get(i);
            if dev.is_null() {
                continue;
            }
            let n = sys::ggml_backend_dev_name(dev);
            if n.is_null() || std::ffi::CStr::from_ptr(n).to_string_lossy() != name {
                continue;
            }
            let d = sys::ggml_backend_dev_description(dev);
            if d.is_null() {
                return None;
            }
            let d = std::ffi::CStr::from_ptr(d).to_string_lossy().trim().to_string();
            return (!d.is_empty() && d != name).then_some(d);
        }
    }
    None
}

/// `s` between the first `start` and the next `end` after it.
fn between<'a>(s: &'a str, start: &str, end: &str) -> Option<&'a str> {
    let from = s.find(start)? + start.len();
    let len = s[from..].find(end)?;
    Some(s[from..from + len].trim())
}

// ---- whisper.cpp log capture -----------------------------------------------

/// The latest backend lines whisper.cpp logged (a few per transcription).
static WHISPER_LINES: Mutex<std::collections::VecDeque<String>> =
    Mutex::new(std::collections::VecDeque::new());
const MAX_LINES: usize = 32;

/// Route whisper.cpp's (and ggml's) log through us: every line still goes
/// to stderr, as whisper.cpp's default logger does, and the backend lines
/// are kept for [`whisper_lines`]. Idempotent.
pub fn install_whisper_log_capture() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| unsafe {
        whisper_rs::whisper_rs_sys::whisper_log_set(Some(whisper_log), std::ptr::null_mut());
    });
}

unsafe extern "C" fn whisper_log(
    _level: whisper_rs::whisper_rs_sys::ggml_log_level,
    text: *const std::os::raw::c_char,
    _user: *mut std::os::raw::c_void,
) {
    if text.is_null() {
        return;
    }
    // SAFETY: whisper.cpp passes a NUL-terminated string valid for the call.
    let bytes = unsafe { std::ffi::CStr::from_ptr(text) }.to_bytes();
    // Never panic across the FFI boundary: plain writes, errors ignored.
    let _ = std::io::Write::write_all(&mut std::io::stderr(), bytes);
    if bytes.starts_with(b"whisper_backend_init") {
        if let Ok(mut lines) = WHISPER_LINES.lock() {
            if lines.len() == MAX_LINES {
                lines.pop_front();
            }
            lines.push_back(String::from_utf8_lossy(bytes).trim().to_string());
        }
    }
}

/// The backend lines captured so far, oldest first.
pub fn whisper_lines() -> Vec<String> {
    WHISPER_LINES
        .lock()
        .map(|l| l.iter().cloned().collect())
        .unwrap_or_default()
}

/// What whisper.cpp runs on now: the latest logged block, the device
/// described by ggml's registry. `None` until whisper.cpp set up a
/// backend (the first transcription or VAD run).
pub fn whisper_backend() -> Option<ComputeBackend> {
    let mut b = parse_whisper_log(&whisper_lines())?;
    if let Some(dev) = b.device.clone() {
        if let Some(desc) = ggml_device_description(&dev) {
            // "Metal" says nothing the kind doesn't: just "Apple M1 Pro".
            b.device = Some(if kind_of(&dev) == dev {
                desc
            } else {
                format!("{dev}, {desc}")
            });
        }
    }
    Some(b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_names_map_to_backend_families() {
        assert_eq!(kind_of("MTL0"), "Metal");
        assert_eq!(kind_of("Metal"), "Metal");
        assert_eq!(kind_of("Vulkan0"), "Vulkan");
        assert_eq!(kind_of("CUDA0"), "CUDA");
        assert_eq!(kind_of("CPU"), "CPU");
        assert_eq!(kind_of("BLAS"), "BLAS");
    }

    #[test]
    fn whisper_metal_log() {
        let lines = [
            "whisper_backend_init_gpu: device 0: MTL0 (type: 1)",
            "whisper_backend_init_gpu: found GPU device 0: MTL0 (type: 1, cnt: 0)",
            "whisper_backend_init_gpu: using MTL0 backend",
            "whisper_backend_init: using BLAS backend",
        ];
        let b = parse_whisper_log(&lines).unwrap();
        assert_eq!(b.kind, "Metal");
        assert_eq!(b.device.as_deref(), Some("MTL0"));
        assert_eq!(b.note.as_deref(), Some("with BLAS"));
        assert_eq!(b.source, "whisper.cpp log");
        assert!(b.gpu());
    }

    #[test]
    fn whisper_vulkan_log() {
        let lines = ["whisper_backend_init_gpu: using Vulkan0 backend\n"];
        let b = parse_whisper_log(&lines).unwrap();
        assert_eq!((b.kind.as_str(), b.device.as_deref()), ("Vulkan", Some("Vulkan0")));
    }

    #[test]
    fn whisper_without_gpu_is_cpu() {
        let lines = [
            "whisper_backend_init_gpu: device 0: CPU (type: 0)",
            "whisper_backend_init_gpu: no GPU found",
        ];
        let b = parse_whisper_log(&lines).unwrap();
        assert_eq!(b.kind, "CPU");
        assert_eq!(b.note.as_deref(), Some("no GPU found"));
        assert!(!b.gpu());
    }

    #[test]
    fn whisper_gpu_that_fails_to_start_is_cpu() {
        let lines = [
            "whisper_backend_init_gpu: using Vulkan0 backend",
            "whisper_backend_init_gpu: failed to initialize Vulkan0 backend",
        ];
        let b = parse_whisper_log(&lines).unwrap();
        assert_eq!(b.kind, "CPU");
        assert_eq!(b.note.as_deref(), Some("Vulkan (Vulkan0) failed to initialise"));
    }

    #[test]
    fn whisper_latest_block_wins() {
        let lines = [
            "whisper_backend_init_gpu: device 0: Vulkan0 (type: 1)",
            "whisper_backend_init_gpu: using Vulkan0 backend",
            "whisper_backend_init_gpu: failed to initialize Vulkan0 backend",
            // next transcription: the GPU came up this time
            "whisper_backend_init_gpu: device 0: Vulkan0 (type: 1)",
            "whisper_backend_init_gpu: using Vulkan0 backend",
            "whisper_backend_init: using BLAS backend",
            "whisper_backend_init: using BLAS backend",
        ];
        let b = parse_whisper_log(&lines).unwrap();
        assert_eq!(b.kind, "Vulkan");
        assert_eq!(b.note.as_deref(), Some("with BLAS"));
        // and back to the CPU (use_gpu off, or the device disappeared)
        let mut more = lines.to_vec();
        more.push("whisper_backend_init_gpu: no GPU found");
        assert_eq!(parse_whisper_log(&more).unwrap().kind, "CPU");
    }

    #[test]
    fn whisper_log_without_backend_lines_says_nothing() {
        assert_eq!(parse_whisper_log::<&str>(&[]), None);
        assert_eq!(parse_whisper_log(&["whisper_model_load: n_vocab = 51866"]), None);
    }

    /// Needs a whisper model: SUSSURRO_TEST_MODEL=/path/ggml-base.en.bin
    /// cargo test live_whisper_backend -- --ignored --nocapture
    #[test]
    #[ignore]
    fn live_whisper_backend_is_detected_from_the_log() {
        let model = std::env::var("SUSSURRO_TEST_MODEL").expect("set SUSSURRO_TEST_MODEL");
        install_whisper_log_capture();
        let t = crate::stt::whisper::Transcriber::load(std::path::Path::new(&model)).unwrap();
        t.transcribe(&vec![0.0f32; 16_000], None, "auto").unwrap();
        let b = whisper_backend().expect("backend lines captured");
        println!("whisper backend: {b:?}");
        if cfg!(target_os = "macos") {
            assert_eq!(b.kind, "Metal");
        }
    }

    #[test]
    fn llama_metal_log() {
        let log = "\
build: 11146 (abc) with clang for arm64-apple-darwin
llama_model_load_from_file_impl: using device MTL0 (Apple M2 Pro) (unknown id) - 21845 MiB free
llama_model_loader: loaded meta data with 30 key-value pairs
load_tensors: offloading 28 repeating layers to GPU
load_tensors: offloaded 29/29 layers to GPU
load_tensors:   CPU_Mapped model buffer size =   315.30 MiB
";
        let b = parse_llama_log(log).unwrap();
        assert_eq!(b.kind, "Metal");
        assert_eq!(b.device.as_deref(), Some("MTL0, Apple M2 Pro"));
        assert_eq!(b.note.as_deref(), Some("29/29 layers on the GPU"));
        assert_eq!(b.source, "llama-server log");
    }

    #[test]
    fn llama_cpu_only_log() {
        let log = "load_tensors: loading model tensors, this can take a while... (mmap = true)\nload_tensors:   CPU_Mapped model buffer size =  1000 MiB\n";
        let b = parse_llama_log(log).unwrap();
        assert_eq!(b.kind, "CPU");
        assert_eq!(b.device, None);
    }

    #[test]
    fn llama_log_before_load_says_nothing() {
        assert_eq!(parse_llama_log(""), None);
        assert_eq!(parse_llama_log("main: starting\n"), None);
    }

    #[test]
    fn llama_log_latest_run_wins() {
        let log = "\
llama_model_load_from_file_impl: using device Vulkan0 (Old GPU) (unknown id) - 100 MiB free
load_tensors: offloaded 10/29 layers to GPU
llama_model_load_from_file_impl: using device Vulkan1 (NVIDIA GeForce RTX 3060) (0000:01:00.0) - 11000 MiB free
load_tensors: offloaded 29/29 layers to GPU
";
        let b = parse_llama_log(log).unwrap();
        assert_eq!(b.device.as_deref(), Some("Vulkan1, NVIDIA GeForce RTX 3060"));
        assert_eq!(b.note.as_deref(), Some("29/29 layers on the GPU"));
    }
}
