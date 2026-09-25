//! Text-to-speech (0.12, plan §4.5) — an **experimental, optional module**
//! (P24): off by default, and no TTS model is ever downloaded without the
//! user's explicit request. The rest of the app never depends on it.
//!
//! - [`text`]: an archive item's markdown → speakable, normalised sentence
//!   chunks for Italian and English (#254). Pure and engine-independent.
//! - [`engine`]: the [`engine::TtsEngine`] seam, rendering chunks with
//!   their pauses, and the WAV writer (#255).
//! - [`pocket`]: Kyutai's Pocket TTS (P18) through the app's `ort` — the
//!   Italian 24-layer and the English model, fp32 ONNX graphs (E16 a).
//! - [`catalog`] / [`models`]: the pinned files and voices, and their
//!   download (on request only), verification and deletion.
//! - [`service`]: the loaded engine, its idle unload, the preview.
//!
//! File generation only, not real-time playback (P18). Read-aloud of
//! archive items and the marking of generated files are #256 and #264.

pub mod catalog;
pub mod engine;
pub mod marking;
pub mod models;
pub mod pocket;
pub mod resample;
pub mod service;
pub mod text;

#[cfg(test)]
mod live_tests;
