//! Text-to-speech (0.12, plan §4.5) — an **experimental, optional module**
//! (P24): off by default, and no TTS model is ever downloaded without the
//! user's explicit request. The rest of the app never depends on it.
//!
//! - [`text`]: an archive item's markdown → speakable, normalised sentence
//!   chunks for Italian and English (#254). Pure and engine-independent;
//!   the Pocket TTS engine (#255, P18) and read-aloud (#256) consume its
//!   [`text::Chunk`]s.

pub mod text;
