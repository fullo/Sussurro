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
//! - [`marking`] / [`watermark`]: every generated file carries tags and an
//!   AudioSeal watermark (P21, #257), and [`signing`] adds signed C2PA
//!   metadata with a per-install key; [`check`] is *Check a file*.
//!
//! File generation only, not real-time playback (P18). Read-aloud of
//! archive items is #256.

pub mod catalog;
pub mod check;
pub mod engine;
pub mod marking;
pub mod models;
pub mod pocket;
pub mod podcast;
pub mod read_aloud;
pub mod resample;
pub mod service;
pub mod signing;
pub mod text;
pub mod watermark;

#[cfg(test)]
mod live_tests;
