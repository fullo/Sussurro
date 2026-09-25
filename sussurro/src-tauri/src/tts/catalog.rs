//! What the read-aloud module can download (P18, E16, #255): the Pocket TTS
//! models and voices, each file pinned to a HuggingFace revision, a size
//! and a SHA-256. Pure data — [`super::models`] downloads and checks it.
//!
//! **Models**: the community ONNX export `KevinAHM/pocket-tts-onnx` of
//! Kyutai's Pocket TTS (weights CC-BY-4.0), fp32 graphs only — the int8
//! ones diverge from the reference (spike #236). Only the four graphs that
//! speak are listed: `text_conditioner`, `flow_lm_main`, `flow_lm_flow`,
//! `mimi_decoder`. **Never `mimi_encoder.onnx`**: it turns audio into a
//! voice, i.e. cloning, which Kyutai keeps behind a consent prompt (P19).
//!
//! **Voices**: prompt states from Kyutai's own repo *without* voice cloning
//! (`kyutai/pocket-tts-without-voice-cloning`, CC-BY-4.0), at the revision
//! whose tokenizer matches the ONNX export (the same `tokenizer.model`
//! bytes). A later revision changed the tokenizer and the voice states:
//! mixing revisions makes the model stop at its first step. Only voices
//! whose source recordings allow commercial use are listed (CC0 or
//! CC-BY-4.0: Common Voice, voice donations, LibriVox via Voice-Zero,
//! VCTK, Alba MacKenna); the EARS and Expresso voices (`jean`, `cosette`)
//! are non-commercial and left out.

/// The ONNX export of the Pocket TTS models.
pub const MODEL_REPO: &str = "KevinAHM/pocket-tts-onnx";
/// Its pinned revision (validated against PyTorch in #236).
pub const MODEL_REVISION: &str = "58a6d00cf13d239b6748cb0769f35c580a8f606c";
/// Kyutai's repo without voice cloning: the voice states.
pub const VOICE_REPO: &str = "kyutai/pocket-tts-without-voice-cloning";
/// The revision whose tokenizer matches the export's.
pub const VOICE_REVISION: &str = "e81d79e8194ad4c7ce879c87a4258ef20cbf2487";
/// Folder under the models folder that holds everything of this module.
pub const ROOT_DIR: &str = "pocket-tts";
/// Licence of the models and voice states.
pub const MODEL_LICENCE: &str = "CC-BY-4.0";
/// Attribution line shown next to the download and in About.
pub const ATTRIBUTION: &str = "Pocket TTS by Kyutai (CC BY 4.0), ONNX export by KevinAHM";

/// One pinned file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PinnedFile {
    /// Path in the upstream repo, at the pinned revision.
    pub remote: &'static str,
    /// File name on disk.
    pub name: &'static str,
    pub bytes: u64,
    pub sha256: &'static str,
}

/// One voice (a prompt state for one language's model).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Voice {
    /// Kyutai's name for it; also the file stem on disk.
    pub id: &'static str,
    /// Shown in the UI.
    pub label: &'static str,
    /// Where the recording comes from and its licence.
    pub source: &'static str,
    pub file: PinnedFile,
}

/// One language: its model files and voices.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Language {
    /// ISO 639-1 code, as the archive's `language` frontmatter.
    pub code: &'static str,
    pub label: &'static str,
    /// The export's bundle folder; also the folder on disk.
    pub bundle: &'static str,
    /// e.g. "24 layers".
    pub variant: &'static str,
    pub files: &'static [PinnedFile],
    pub voices: &'static [Voice],
    /// Voice used until the user picks another.
    pub default_voice: &'static str,
    /// Sentence the preview reads.
    pub preview: &'static str,
}

impl Language {
    /// Bytes of the model files (voices not included).
    pub fn model_bytes(&self) -> u64 {
        self.files.iter().map(|f| f.bytes).sum()
    }

    pub fn voice(&self, id: &str) -> Option<&'static Voice> {
        self.voices.iter().find(|v| v.id == id)
    }

    /// URL of a model file at the pinned revision.
    pub fn file_url(&self, f: &PinnedFile) -> String {
        resolve_url(MODEL_REPO, MODEL_REVISION, f.remote)
    }

    /// URL of a voice file at the pinned revision.
    pub fn voice_url(&self, v: &Voice) -> String {
        resolve_url(VOICE_REPO, VOICE_REVISION, v.file.remote)
    }
}

fn resolve_url(repo: &str, revision: &str, path: &str) -> String {
    format!("https://huggingface.co/{repo}/resolve/{revision}/{path}")
}

const VCTK: &str = "VCTK corpus, CSTR, University of Edinburgh (CC BY 4.0)";
const ALBA: &str = "Alba MacKenna (CC BY 4.0)";
const DONATION: &str = "Unmute voice donation (CC0)";

pub const ITALIAN: Language = Language {
    code: "it",
    label: "Italian",
    bundle: "italian_24l",
    variant: "24 layers",
    files: &[
        PinnedFile {
            remote: "onnx/italian_24l/bundle.json",
            name: "bundle.json",
            bytes: 42_241,
            sha256: "4b7fd6f191cd1f48a8459833a390a1e2ad59bc284fa11e518bcdb64f36fb95d1",
        },
        PinnedFile {
            remote: "onnx/italian_24l/tokenizer.model",
            name: "tokenizer.model",
            bytes: 60_078,
            sha256: "6583b974a11b90e14d8a4c8e9c43f06c3861b9ede6e5023a4c27ab5a3a7d4c39",
        },
        PinnedFile {
            remote: "onnx/italian_24l/text_conditioner.onnx",
            name: "text_conditioner.onnx",
            bytes: 16_388_344,
            sha256: "00ff8327e832c8e46a4562cef886b53977c76c1e09bb4448c4a879689bced83d",
        },
        PinnedFile {
            remote: "onnx/italian_24l/flow_lm_flow.onnx",
            name: "flow_lm_flow.onnx",
            bytes: 39_097_095,
            sha256: "3646665d2b64f829d30bb8e6bdcf71802ee16dee8ae9af1ee460510eb8d9898f",
        },
        PinnedFile {
            remote: "onnx/italian_24l/mimi_decoder.onnx",
            name: "mimi_decoder.onnx",
            bytes: 41_471_926,
            sha256: "f474670d1494cd8ed43b610c68c90203a01211fac50086ae2aaf3d691af4efbb",
        },
        PinnedFile {
            remote: "onnx/italian_24l/flow_lm_main.onnx",
            name: "flow_lm_main.onnx",
            bytes: 1_210_441_908,
            sha256: "e213ce648733d0d2756b6567c9fb02ef5932a82e88b255ee7a1067eb4e783360",
        },
    ],
    voices: &[
        Voice {
            id: "giovanni",
            label: "Giovanni",
            source: "Common Voice Italian (CC0)",
            file: PinnedFile {
                remote: "languages/italian_24l/embeddings/giovanni.safetensors",
                name: "giovanni.safetensors",
                bytes: 18_486_272,
                sha256: "3bd832fc10cfacc752956288101725626d133b456cb3c47551c2776fed6fe087",
            },
        },
        Voice {
            id: "alba",
            label: "Alba",
            source: ALBA,
            file: PinnedFile {
                remote: "languages/italian_24l/embeddings/alba.safetensors",
                name: "alba.safetensors",
                bytes: 24_777_760,
                sha256: "323a2f5757df5c003c54c34d3aaefb993d80565302683df6f9e883f8e4155d4e",
            },
        },
        Voice {
            id: "marius",
            label: "Marius",
            source: DONATION,
            file: PinnedFile {
                remote: "languages/italian_24l/embeddings/marius.safetensors",
                name: "marius.safetensors",
                bytes: 24_777_760,
                sha256: "647474a57af63a32243f7172ed26d0f8d8af69051eae0b606ab2ce6c72a5a292",
            },
        },
        Voice {
            id: "anna",
            label: "Anna",
            source: VCTK,
            file: PinnedFile {
                remote: "languages/italian_24l/embeddings/anna.safetensors",
                name: "anna.safetensors",
                bytes: 31_265_824,
                sha256: "79b769775905fc6308538f024945c4df826009aab899c1265814aa728e85fab2",
            },
        },
    ],
    default_voice: "giovanni",
    preview:
        "Ciao, sono la voce di lettura di Sussurro. Così suonerà un documento letto ad alta voce.",
};

pub const ENGLISH: Language = Language {
    code: "en",
    label: "English",
    bundle: "english_2026-04",
    variant: "April 2026",
    files: &[
        PinnedFile {
            remote: "onnx/english_2026-04/bundle.json",
            name: "bundle.json",
            bytes: 24_381,
            sha256: "bab643150f437f37df080a710520ff39ed9ebd9a339f8ebdc739f7eddfc28b3f",
        },
        PinnedFile {
            remote: "onnx/english_2026-04/tokenizer.model",
            name: "tokenizer.model",
            bytes: 59_339,
            sha256: "d461765ae179566678c93091c5fa6f2984c31bbe990bf1aa62d92c64d91bc3f6",
        },
        PinnedFile {
            remote: "onnx/english_2026-04/text_conditioner.onnx",
            name: "text_conditioner.onnx",
            bytes: 16_388_344,
            sha256: "4ecee995fb69f85c7a7493d11f7b5ee15d9950facc7ab3f5c9c49ef1e03847bb",
        },
        PinnedFile {
            remote: "onnx/english_2026-04/flow_lm_flow.onnx",
            name: "flow_lm_flow.onnx",
            bytes: 39_097_095,
            sha256: "085d239f68897e28fb06e95c743738ad8b8c092ee6dc55f5491313e81ff08062",
        },
        PinnedFile {
            remote: "onnx/english_2026-04/mimi_decoder.onnx",
            name: "mimi_decoder.onnx",
            bytes: 41_471_926,
            sha256: "86f038caa02a96a0ff9c25526a0ff43a4906c418197ed72d3e30f720ac7ce802",
        },
        PinnedFile {
            remote: "onnx/english_2026-04/flow_lm_main.onnx",
            name: "flow_lm_main.onnx",
            bytes: 302_742_149,
            sha256: "6d18315e2c33ca3e3aa4a4e3dca22f56d007fd823127e24948b37695bf54190f",
        },
    ],
    voices: &[
        Voice {
            id: "alba",
            label: "Alba",
            source: ALBA,
            file: PinnedFile {
                remote: "languages/english_2026-04/embeddings/alba.safetensors",
                name: "alba.safetensors",
                bytes: 6_194_424,
                sha256: "69c32db63ca56843d994f81f343f62e0bf2d73f7e4c9bc73e44bb1110b1d8845",
            },
        },
        Voice {
            id: "marius",
            label: "Marius",
            source: DONATION,
            file: PinnedFile {
                remote: "languages/english_2026-04/embeddings/marius.safetensors",
                name: "marius.safetensors",
                bytes: 6_194_424,
                sha256: "04f84efcb77a0547ba582c058db496f7ff4920891d49d37b9950d128422582a8",
            },
        },
        Voice {
            id: "javert",
            label: "Javert",
            source: DONATION,
            file: PinnedFile {
                remote: "languages/english_2026-04/embeddings/javert.safetensors",
                name: "javert.safetensors",
                bytes: 6_194_424,
                sha256: "0ae88e03ca4e76a0e16cbf321a807428febda9d9e9bc0358c02e7f9c9e2c263b",
            },
        },
        Voice {
            id: "anna",
            label: "Anna",
            source: VCTK,
            file: PinnedFile {
                remote: "languages/english_2026-04/embeddings/anna.safetensors",
                name: "anna.safetensors",
                bytes: 7_816_440,
                sha256: "5ea82f78db006c9fd34e32ddd5aae82674b5b32646097977436458d00af80dfa",
            },
        },
        Voice {
            id: "george",
            label: "George",
            source: VCTK,
            file: PinnedFile {
                remote: "languages/english_2026-04/embeddings/george.safetensors",
                name: "george.safetensors",
                bytes: 6_243_576,
                sha256: "0c1c6c57c55a98d81254b33728150c7776f40647fe95258d1a6c1a02780b5d02",
            },
        },
        Voice {
            id: "peter_yearsley",
            label: "Peter",
            source: "LibriVox reader Peter Yearsley, via Voice-Zero (CC0)",
            file: PinnedFile {
                remote: "languages/english_2026-04/embeddings/peter_yearsley.safetensors",
                name: "peter_yearsley.safetensors",
                bytes: 3_736_816,
                sha256: "dd977a6e15591e347c9a23fa7cc09e35a65b462917f5eeb162baff6dc9e3f685",
            },
        },
    ],
    default_voice: "alba",
    preview: "Hello, I am Sussurro's reading voice. This is how a document will sound when it is read aloud.",
};

/// Every language the module offers, in UI order.
pub const LANGUAGES: &[Language] = &[ITALIAN, ENGLISH];

/// The language for an ISO code (`it`, `en-GB` → `en`…).
pub fn language(code: &str) -> Option<&'static Language> {
    let base = code.trim().to_ascii_lowercase();
    let base = base.split(['-', '_']).next().unwrap_or("");
    LANGUAGES.iter().find(|l| l.code == base)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stt::models::validate_model_name;

    #[test]
    fn every_file_is_pinned_and_safe_to_store() {
        for l in LANGUAGES {
            assert!(validate_model_name(l.bundle).is_ok(), "{}", l.bundle);
            let files = l.files.iter().chain(l.voices.iter().map(|v| &v.file));
            for f in files {
                assert!(validate_model_name(f.name).is_ok(), "{}", f.name);
                assert!(f.remote.ends_with(f.name), "{} vs {}", f.remote, f.name);
                assert_eq!(f.sha256.len(), 64, "{}", f.name);
                assert!(f.sha256.chars().all(|c| c.is_ascii_hexdigit()));
                assert!(f.bytes > 0);
            }
            assert!(l.voice(l.default_voice).is_some(), "{}", l.code);
            let url = l.file_url(&l.files[0]);
            assert!(url.contains(MODEL_REVISION), "{url}");
            let url = l.voice_url(&l.voices[0]);
            assert!(url.contains(VOICE_REVISION), "{url}");
        }
    }

    /// E16 / P19: the four speaking graphs in fp32, never the encoder
    /// (cloning) nor an int8 graph.
    #[test]
    fn only_the_four_fp32_graphs_without_the_cloning_encoder() {
        for l in LANGUAGES {
            let graphs: Vec<&str> = l
                .files
                .iter()
                .map(|f| f.name)
                .filter(|n| n.ends_with(".onnx"))
                .collect();
            assert_eq!(
                graphs,
                [
                    "text_conditioner.onnx",
                    "flow_lm_flow.onnx",
                    "mimi_decoder.onnx",
                    "flow_lm_main.onnx"
                ]
            );
            for f in l.files {
                assert!(!f.remote.contains("encoder"), "{}", f.remote);
                assert!(!f.remote.contains("int8"), "{}", f.remote);
                assert!(f.remote.starts_with(&format!("onnx/{}/", l.bundle)));
            }
            for v in l.voices {
                assert!(v
                    .file
                    .remote
                    .starts_with(&format!("languages/{}/embeddings/", l.bundle)));
            }
        }
    }

    #[test]
    fn non_commercial_voices_are_not_offered() {
        for l in LANGUAGES {
            for v in l.voices {
                assert!(!["jean", "cosette"].contains(&v.id), "{}", v.id);
                assert!(
                    v.source.contains("CC0") || v.source.contains("CC BY 4.0"),
                    "{}: {}",
                    v.id,
                    v.source
                );
            }
        }
    }

    #[test]
    fn sizes_match_what_the_ui_announces() {
        assert_eq!(ITALIAN.model_bytes(), 1_307_501_592);
        assert_eq!(ENGLISH.model_bytes(), 399_783_234);
    }

    #[test]
    fn languages_by_code() {
        assert_eq!(language("it").unwrap().bundle, "italian_24l");
        assert_eq!(language("en-GB").unwrap().bundle, "english_2026-04");
        assert_eq!(language(" IT_it ").unwrap().code, "it");
        assert!(language("fr").is_none());
        assert!(language("").is_none());
    }
}
