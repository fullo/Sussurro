//! Speakers (plan §4.3, layer 3 — acoustic clustering, #130): speaker
//! embeddings (E8) and clustering into generic "Voice 1, Voice 2…" labels
//! (P8), for any long-form source — mic-only meetings, the remote channel
//! of browser meetings without names, transcriptions with "Identify
//! voices".
//!
//! - [`fbank`]: 80-dim Kaldi fbank over `rustfft`
//! - [`model`]: WeSpeaker ResNet34-LM through `ort`, downloaded on first
//!   use and pinned by SHA-256 (fail-closed)
//! - [`cluster`]: online clustering, agglomerative clustering, small-voice
//!   fold (pure)
//! - [`tracker`]: live labels for a running session (engine option)
//! - [`doc`]: a document's speakers — move a line, rename, "Re-detect"
//! - [`names`]: names from the meeting page (layer 2, #131): the page's
//!   speaker timeline and majority-overlap attribution of remote lines
//! - [`profiles`]: voice profiles of People (0.11, #241): enrolment from
//!   confirmed lines and suggestion matching (pure)
//! - [`voices`]: the profile files in `<app data>/voices/` — never in the
//!   archive (P13)
//! - [`own_voice`]: the user's own voice, "You" (0.11, #243, P14):
//!   read-aloud enrolment and labelling single-channel recordings
//! - [`suggestions`]: "Voice N sounds like Anna" for an open document and
//!   the per-document *Not Anna* answers (0.11, #242)
//! - [`overlap`]: overlapping speech inside a line and its second speaker
//!   (0.11, #244, pure); [`segmentation`]: the pyannote segmentation-3.0
//!   model that finds it, through `ort`, pinned like WeSpeaker

pub mod cluster;
pub mod doc;
pub mod fbank;
pub mod map;
pub mod model;
pub mod names;
pub mod overlap;
pub mod own_voice;
pub mod profiles;
pub mod segmentation;
pub mod suggestions;
pub mod tracker;
pub mod voices;

pub use tracker::{SpeakerOptions, Tracker};

// Thresholds and windows from the Phase 0 spike (#107). They were tuned on
// English AMI meeting clips (headset and one far-field mic, 4 speakers
// each) with WeSpeaker ResNet34-LM: re-check them on Italian speech and on
// real meetings before treating them as final.

/// Online clustering while a session runs: cosine similarity to a voice's
/// centroid needed to join it, on windows of at most [`LIVE_WINDOW_MS`].
/// Below ~0.20 two speakers merge; above ~0.30 voices over-split.
pub const ONLINE_THRESHOLD: f32 = 0.275;
/// "Re-detect speakers": average-linkage agglomerative clustering stops
/// merging below this similarity. The spike ran it on segments of at most
/// 10 s ([`REDETECT_SEGMENT_MS`]); here it runs on the per-line embeddings
/// stored in `segments.json` (each the mean over the line's ≤ 3 s windows).
pub const REDETECT_THRESHOLD: f32 = 0.30;
/// Segment length the re-detect threshold was tuned on (#107).
pub const REDETECT_SEGMENT_MS: u64 = 10_000;
/// Longest audio window embedded at once while a session runs.
pub const LIVE_WINDOW_MS: u64 = 3_000;
/// Shortest audio that gets an embedding (and so a voice).
pub const MIN_EMBED_MS: u64 = 1_000;
/// A voice with less speech than this folds into the nearest voice
/// (after "Re-detect" and at the end of a live session).
pub const MIN_VOICE_SPEECH_MS: u64 = 10_000;

#[cfg(test)]
mod tests {
    /// Runs the real model on a short multi-speaker WAV (16 kHz mono,
    /// 16-bit) and checks that clustering finds at least two voices:
    ///
    /// ```text
    /// SUSSURRO_SPEAKER_MODEL=<voxceleb_resnet34_LM.onnx> \
    /// SUSSURRO_SPEAKER_WAV=<two or more speakers, ~1 min> \
    ///   cargo test speakers::tests::real_model -- --ignored --nocapture
    /// ```
    ///
    /// The WAV is cut into 3 s windows (no VAD), embedded, and clustered
    /// online and offline; the voice sequence is printed for a listen-check.
    #[test]
    #[ignore = "needs the model file and a multi-speaker WAV"]
    fn real_model_separates_voices_in_a_wav() {
        use super::cluster::{agglomerative, fold_small, OnlineClusterer};
        use super::model::{SpeakerEmbedder, WeSpeaker, MODEL_SHA256};
        let model = std::path::PathBuf::from(std::env::var("SUSSURRO_SPEAKER_MODEL").unwrap());
        assert_eq!(
            crate::stt::models::sha256_hex(&model).unwrap(),
            MODEL_SHA256
        );
        let wav = std::env::var("SUSSURRO_SPEAKER_WAV").unwrap();
        let bytes = std::fs::read(wav).unwrap();
        // Minimal WAV reader: find the "data" chunk, 16-bit mono 16 kHz.
        let data = bytes
            .windows(4)
            .position(|w| w == b"data")
            .expect("no data chunk");
        let pcm: Vec<f32> = bytes[data + 8..]
            .chunks_exact(2)
            .map(|c| i16::from_le_bytes([c[0], c[1]]) as f32 / 32768.0)
            .collect();
        let mut embedder = WeSpeaker::load(&model).unwrap();
        let win = 48_000;
        let embs: Vec<Vec<f32>> = pcm
            .chunks(win)
            .filter(|c| c.len() >= 16_000)
            .map(|c| embedder.embed(c).unwrap())
            .collect();
        assert!(embs.len() >= 4, "need at least 12 s of audio");
        let mut online = OnlineClusterer::new(super::ONLINE_THRESHOLD);
        let live: Vec<usize> = embs.iter().map(|e| online.assign(e)).collect();
        let refs: Vec<&[f32]> = embs.iter().map(Vec::as_slice).collect();
        let durs = vec![3_000u64; embs.len()];
        let offline = agglomerative(&refs, super::REDETECT_THRESHOLD);
        let offline = fold_small(&refs, &durs, &offline, super::MIN_VOICE_SPEECH_MS);
        eprintln!("online : {live:?}");
        eprintln!("offline: {offline:?}");
        let voices = |l: &[usize]| l.iter().collect::<std::collections::BTreeSet<_>>().len();
        assert!(voices(&live) >= 2, "online found one voice");
        assert!(voices(&offline) >= 2, "re-detect found one voice");
    }
}
