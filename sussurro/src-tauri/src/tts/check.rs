//! *Check a file* (#257, E17 item 3; Code of Practice 2.1 and 2.3): does an
//! audio file carry Sussurro's marks? Local and free — nothing is uploaded.
//! Each layer answers on its own:
//!
//! - **Watermark**: the file is decoded (Ogg Opus through libopus at
//!   16 kHz, anything else symphonia reads at its own rate and then the
//!   band-limited [`super::resample`] to 16 kHz), cut into ≤ 10 s windows
//!   and read by the AudioSeal detector ([`super::watermark`]). The verdict
//!   follows E17 exactly: *found* = at least half the frames and at most 2
//!   of the 16 payload bits wrong; a high frame score with another payload
//!   is *inconclusive* (tones and music can reach the frame score), never
//!   "another tool's mark"; nothing found is "no Sussurro mark found",
//!   never "a human recording".
//! - **Metadata**: the Ogg Opus comments ([`crate::archive::opus::read_tags`])
//!   or a WAV's `LIST/INFO`. Plain tags, not signed: anyone can write or
//!   strip them, and the UI says so.
//! - **Signature** (#257 part 2): signed C2PA metadata — embedded in the
//!   file (WAV, MP3, M4A, FLAC) or in a `<same stem>.c2pa` sidecar next to
//!   it (Ogg can't hold one: that is how Sussurro saves `speech.opus`). The
//!   sidecar is read only as a regular file (no link) of at most
//!   [`super::signing::MAX_SIDECAR_BYTES`], opened with the picker's
//!   guards. *Valid* = signed and unchanged since, by a self-signed
//!   certificate — every Sussurro install signs with its own, which no
//!   trust list knows, and anyone can make such a certificate: the manifest
//!   says who *claims* to have made the file. *Invalid* = the file changed
//!   after signing, the sidecar belongs to another file, or the manifest is
//!   damaged. Checked locally, no network (no OCSP, no trust list).
//!
//! At most [`MAX_CHECK_SECONDS`] of audio is read (the rest is reported as
//! not checked); detection costs ~1–2 s per minute at 2 threads.

use super::watermark::{self, Detection, DetectorModel, Verdict, WINDOW};
use crate::archive::opus::OpusReader;
use anyhow::{anyhow, Context, Result};
use serde::Serialize;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

/// Extensions the picker offers and the guard accepts.
pub const EXTENSIONS: &[&str] = &["opus", "ogg", "wav", "mp3", "m4a", "flac"];
/// Largest file accepted (an hour of 48 kHz stereo 16-bit WAV is ~690 MB).
pub const MAX_FILE_BYTES: u64 = 2 * 1024 * 1024 * 1024;
/// Longest stretch of audio read.
pub const MAX_CHECK_SECONDS: u64 = 60 * 60;
/// Under this, a missing mark says little (spike #240: 1 s excerpts kept
/// the payload in 1 of 22 clips).
pub const SHORT_SECONDS: f64 = 3.0;
/// Tags shown from the file (the ones marking is about), values cut here.
const TAG_KEYS: &[&str] = &[
    "SYNTHETIC",
    "DIGITAL_SOURCE_TYPE",
    "ENCODER",
    "TTS_ENGINE",
    "TTS_VOICE",
    "LANGUAGE",
    "COMMENT",
    "ISFT",
    "ICMT",
];
const MAX_TAG_CHARS: usize = 200;

/// The watermark layer's answer.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct WatermarkLayer {
    pub verdict: Verdict,
    /// Share of the audio the detector found marked (0–1).
    pub frames_marked: f64,
    /// Payload bits that differ from Sussurro's code (0–16).
    pub bit_errors: u32,
}

/// What the file's tags say.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MetadataStatus {
    /// Tagged as synthetic speech made by Sussurro.
    Sussurro,
    /// Tagged as AI-generated, by something else.
    Synthetic,
    /// No such tag.
    None,
    /// This format's tags are not read.
    NotRead,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MetadataLayer {
    pub status: MetadataStatus,
    /// The marking-related tags found, `(name, value)`.
    pub tags: Vec<(String, String)>,
}

pub use super::signing::{SignatureLayer, SignatureSource, SignatureStatus};

/// The one-line answer, from the layers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Summary {
    /// The watermark carries Sussurro's code.
    MadeBySussurro,
    /// A mark-like signal without Sussurro's code.
    Inconclusive,
    /// No watermark found, but valid signed metadata says Sussurro made it
    /// (signed by a self-signed certificate: a claim, not a trusted one).
    SignedOnly,
    /// No watermark found, but the tags say Sussurro (tags can be copied).
    TagsOnly,
    /// No Sussurro mark found — which says nothing about who made it.
    NoMark,
}

/// *Check a file*'s result. Holds no path, only the file's name.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CheckResult {
    pub file_name: String,
    /// "Ogg Opus", "WAV", "MP3"…
    pub format: String,
    /// Audio read, in seconds.
    pub seconds: f64,
    /// The file is longer than [`MAX_CHECK_SECONDS`]: only that much was read.
    pub truncated: bool,
    /// Under [`SHORT_SECONDS`]: a missing mark means little.
    pub short: bool,
    pub summary: Summary,
    pub watermark: WatermarkLayer,
    pub metadata: MetadataLayer,
    pub signature: SignatureLayer,
}

/// The summary from the layers: the watermark first, then valid signed
/// metadata that names Sussurro, then the tags. Pure.
pub fn summarize(
    watermark: Verdict,
    metadata: MetadataStatus,
    signature: &SignatureLayer,
) -> Summary {
    match watermark {
        Verdict::Found => Summary::MadeBySussurro,
        Verdict::Inconclusive => Summary::Inconclusive,
        Verdict::NotFound
            if signature.status == SignatureStatus::Valid && signature.claims_sussurro =>
        {
            Summary::SignedOnly
        }
        Verdict::NotFound if metadata == MetadataStatus::Sussurro => Summary::TagsOnly,
        Verdict::NotFound => Summary::NoMark,
    }
}

/// The sidecar `<same stem>.c2pa` next to `path`: `None` when there is
/// none, `Err` when one is there but can't be used (a link, not a regular
/// file, too large). Opened with the picker's guards.
fn read_sidecar(path: &Path) -> Option<std::result::Result<Vec<u8>, String>> {
    let sidecar = super::signing::sidecar_path(path);
    std::fs::symlink_metadata(&sidecar).ok()?;
    Some(
        crate::config_io::open_picked_file(
            &sidecar,
            &[super::signing::SIDECAR_EXT],
            super::signing::MAX_SIDECAR_BYTES,
        )
        .and_then(|mut f| {
            let mut buf = Vec::new();
            f.by_ref()
                .take(super::signing::MAX_SIDECAR_BYTES + 1)
                .read_to_end(&mut buf)?;
            Ok(buf)
        })
        .map_err(|e| format!("the .c2pa file next to it was not read ({e})")),
    )
}

/// The signature layer of the file at `path` (opened as `file`): an
/// embedded manifest first, else the sidecar.
fn signature_layer(path: &Path, file: &mut std::fs::File) -> SignatureLayer {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if let Some(layer) = super::signing::verify_embedded(&ext, file) {
        return layer;
    }
    match read_sidecar(path) {
        None => SignatureLayer::none(),
        Some(Ok(bytes)) => super::signing::verify_sidecar(&bytes, file),
        Some(Err(problem)) => SignatureLayer {
            status: SignatureStatus::Invalid,
            source: Some(SignatureSource::Sidecar),
            problem,
            ..SignatureLayer::none()
        },
    }
}

/// What a file's tags say (Ogg comments or WAV `INFO`, names upper case).
/// Pure.
pub fn metadata_status(tags: &[(String, String)]) -> MetadataStatus {
    let get = |k: &str| {
        tags.iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(k))
            .map(|(_, v)| v.trim())
    };
    let sussurro_encoder = get("ENCODER").is_some_and(|v| v.starts_with(super::marking::GENERATOR));
    let synthetic_tag = get("SYNTHETIC") == Some("1");
    let ai_source =
        get("DIGITAL_SOURCE_TYPE").is_some_and(|v| v.ends_with("/trainedAlgorithmicMedia"));
    // A WAV preview: `ISFT=Sussurro` + `ICMT=Synthetic speech generated by Sussurro …`.
    let wav_comment =
        get("ICMT").is_some_and(|v| v.starts_with("Synthetic speech generated by Sussurro"));
    if (synthetic_tag && sussurro_encoder) || wav_comment {
        MetadataStatus::Sussurro
    } else if synthetic_tag || ai_source {
        MetadataStatus::Synthetic
    } else {
        MetadataStatus::None
    }
}

/// The marking-related tags, values cut to a readable length.
fn relevant(tags: Vec<(String, String)>) -> Vec<(String, String)> {
    tags.into_iter()
        .filter(|(k, _)| TAG_KEYS.iter().any(|t| k.eq_ignore_ascii_case(t)))
        .map(|(k, v)| {
            (
                k.to_ascii_uppercase(),
                v.chars().take(MAX_TAG_CHARS).collect(),
            )
        })
        .collect()
}

/// A WAV's `LIST/INFO` entries (`ICMT`, `ISFT`…), wherever the chunk sits.
/// Empty when there is none or the file isn't a RIFF WAVE.
pub fn read_wav_info<R: Read + Seek>(r: &mut R) -> Vec<(String, String)> {
    fn inner<R: Read + Seek>(r: &mut R) -> Option<Vec<(String, String)>> {
        let mut head = [0u8; 12];
        r.seek(SeekFrom::Start(0)).ok()?;
        r.read_exact(&mut head).ok()?;
        if &head[0..4] != b"RIFF" || &head[8..12] != b"WAVE" {
            return None;
        }
        let mut out = Vec::new();
        // At most 64 chunks: a WAV has a handful.
        for _ in 0..64 {
            let mut ch = [0u8; 8];
            if r.read_exact(&mut ch).is_err() {
                break;
            }
            let len = u32::from_le_bytes(ch[4..8].try_into().ok()?) as u64;
            let padded = len + (len & 1);
            if &ch[0..4] == b"LIST" && (4..=64 * 1024).contains(&len) {
                let mut body = vec![0u8; len as usize];
                r.read_exact(&mut body).ok()?;
                if len & 1 == 1 {
                    r.seek(SeekFrom::Current(1)).ok()?;
                }
                if &body[0..4] == b"INFO" {
                    let mut at = 4;
                    while at + 8 <= body.len() {
                        let id = String::from_utf8_lossy(&body[at..at + 4]).into_owned();
                        let n = u32::from_le_bytes(body[at + 4..at + 8].try_into().ok()?) as usize;
                        let v = body.get(at + 8..at + 8 + n)?;
                        let v = String::from_utf8_lossy(v)
                            .trim_end_matches('\0')
                            .to_string();
                        out.push((id, v));
                        at += 8 + n + (n & 1);
                    }
                }
            } else {
                r.seek(SeekFrom::Current(padded as i64)).ok()?;
            }
        }
        Some(out)
    }
    inner(r).unwrap_or_default()
}

/// Starts like an Ogg file.
fn is_ogg(file: &mut std::fs::File) -> bool {
    let mut magic = [0u8; 4];
    let ok = file.seek(SeekFrom::Start(0)).is_ok() && file.read_exact(&mut magic).is_ok();
    let _ = file.seek(SeekFrom::Start(0));
    ok && &magic == b"OggS"
}

/// Decode to 16 kHz mono, at most `max` samples, handing blocks to `on`.
/// Returns the format name and whether the file had more.
fn decode_16k(
    path: &Path,
    file: std::fs::File,
    max: u64,
    on: &mut dyn FnMut(&[f32]) -> Result<()>,
) -> Result<(String, bool)> {
    let mut file = file;
    if is_ogg(&mut file) {
        // libopus decodes at 16 kHz itself. The reader opens the path again
        // (it indexes pages from its own handle); the guard checked it.
        if let Ok(mut r) = OpusReader::open(path) {
            let mut read = 0u64;
            let mut buf = Vec::with_capacity(WINDOW);
            while read < max {
                buf.clear();
                let want = (WINDOW as u64).min(max - read) as usize;
                if r.read(&mut buf, want)? == 0 {
                    break;
                }
                read += buf.len() as u64;
                on(&buf)?;
            }
            return Ok(("Ogg Opus".into(), r.total_samples() > read));
        }
    }
    decode_symphonia(path, file, max, on)
}

fn decode_symphonia(
    path: &Path,
    file: std::fs::File,
    max: u64,
    on: &mut dyn FnMut(&[f32]) -> Result<()>,
) -> Result<(String, bool)> {
    use symphonia::core::audio::SampleBuffer;
    use symphonia::core::codecs::{DecoderOptions, CODEC_TYPE_NULL};
    use symphonia::core::formats::FormatOptions;
    use symphonia::core::io::MediaSourceStream;
    use symphonia::core::meta::MetadataOptions;
    use symphonia::core::probe::Hint;

    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let mut hint = Hint::new();
    hint.with_extension(&ext);
    let mss = MediaSourceStream::new(Box::new(file), Default::default());
    let probed = symphonia::default::get_probe()
        .format(
            &hint,
            mss,
            &FormatOptions::default(),
            &MetadataOptions::default(),
        )
        .context("this file's audio format can't be read")?;
    let mut format = probed.format;
    let track = format
        .tracks()
        .iter()
        .find(|t| t.codec_params.codec != CODEC_TYPE_NULL && t.codec_params.sample_rate.is_some())
        .ok_or_else(|| anyhow!("no audio track in this file"))?;
    let track_id = track.id;
    let rate = track.codec_params.sample_rate.unwrap_or(watermark::RATE);
    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &DecoderOptions::default())
        .context("no decoder for this audio codec")?;
    let name = match ext.as_str() {
        "wav" => "WAV",
        "mp3" => "MP3",
        "m4a" => "M4A",
        "flac" => "FLAC",
        "ogg" => "Ogg",
        _ => "audio",
    }
    .to_string();
    let mut rs = super::resample::Resampler::new(rate, watermark::RATE);
    let mut read = 0u64;
    let mut more = false;
    let mut emit = |block: Vec<f32>, read: &mut u64| -> Result<bool> {
        let room = max.saturating_sub(*read) as usize;
        let take = block.len().min(room);
        if take > 0 {
            on(&block[..take])?;
            *read += take as u64;
        }
        Ok(block.len() > take)
    };
    // Any packet error ends the stream (as the transcription path).
    while let Ok(packet) = format.next_packet() {
        if packet.track_id() != track_id {
            continue;
        }
        let decoded = match decoder.decode(&packet) {
            Ok(d) => d,
            Err(symphonia::core::errors::Error::DecodeError(_)) => continue,
            Err(e) => return Err(anyhow!("decode error: {e}")),
        };
        let spec = *decoded.spec();
        let channels = spec.channels.count().max(1);
        let mut buf = SampleBuffer::<f32>::new(decoded.capacity() as u64, spec);
        buf.copy_interleaved_ref(decoded);
        let mono: Vec<f32> = buf
            .samples()
            .chunks(channels)
            .map(|f| f.iter().sum::<f32>() / channels as f32)
            .collect();
        if emit(rs.push(&mono), &mut read)? {
            more = true;
            break;
        }
    }
    if !more && emit(rs.flush(), &mut read)? {
        more = true;
    }
    Ok((name, more))
}

/// The file's tags, by format (Ogg Opus comments, WAV `INFO`), filtered to
/// the marking-related ones; `None` = this format's tags are not read.
fn read_tags(path: &Path, file: &mut std::fs::File, format: &str) -> Option<Vec<(String, String)>> {
    match format {
        "Ogg Opus" => Some(crate::archive::opus::read_tags(path).unwrap_or_default()),
        "WAV" => Some(read_wav_info(file)),
        _ => None,
    }
    .map(relevant)
}

/// Check the audio file at `path`, already opened as `file` through the
/// picker's guards, with `detector`. Blocking.
pub fn check_file(
    path: &Path,
    file: std::fs::File,
    detector: &mut dyn DetectorModel,
) -> Result<CheckResult> {
    let file_name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut tag_handle = file.try_clone().context("reading the file")?;
    let mut sig_handle = file.try_clone().context("reading the file")?;
    let max = MAX_CHECK_SECONDS * u64::from(watermark::RATE);
    let mut detection = Detection::default();
    let mut window: Vec<f32> = Vec::with_capacity(WINDOW);
    let (format, truncated) = decode_16k(path, file, max, &mut |block| {
        let mut block = block;
        while !block.is_empty() {
            let take = (WINDOW - window.len()).min(block.len());
            window.extend_from_slice(&block[..take]);
            block = &block[take..];
            if window.len() == WINDOW {
                detection.run_window(detector, &window)?;
                window.clear();
            }
        }
        Ok(())
    })?;
    detection.run_window(detector, &window)?;
    let seconds = detection.samples as f64 / f64::from(watermark::RATE);
    if detection.samples == 0 {
        anyhow::bail!("this file has no audio to check");
    }
    let tags = read_tags(path, &mut tag_handle, &format);
    let metadata = match tags {
        Some(tags) => MetadataLayer {
            status: metadata_status(&tags),
            tags,
        },
        None => MetadataLayer {
            status: MetadataStatus::NotRead,
            tags: Vec::new(),
        },
    };
    let verdict = detection.verdict();
    let signature = signature_layer(path, &mut sig_handle);
    Ok(CheckResult {
        file_name,
        format,
        seconds,
        truncated,
        short: seconds < SHORT_SECONDS,
        summary: summarize(verdict, metadata.status, &signature),
        watermark: WatermarkLayer {
            verdict,
            frames_marked: detection.fraction(),
            bit_errors: detection.bit_errors(),
        },
        metadata,
        signature,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tts::watermark::{BITS, HOP, PAYLOAD};

    /// Reads a watermark wherever the audio is loud: prob = 0.9 above 0.05
    /// in magnitude, else 0.1; the payload is `code`. Records window sizes.
    struct LoudIsMarked {
        code: u16,
        windows: Vec<usize>,
    }

    impl DetectorModel for LoudIsMarked {
        fn detect(&mut self, audio: &[f32]) -> Result<(Vec<f32>, [f32; BITS])> {
            assert_eq!(audio.len() % HOP, 0);
            assert!(audio.len() <= WINDOW);
            self.windows.push(audio.len());
            let prob = audio
                .iter()
                .map(|x| if x.abs() > 0.05 { 0.9 } else { 0.1 })
                .collect();
            let bits = std::array::from_fn(|i| {
                if (self.code >> (BITS - 1 - i)) & 1 == 1 {
                    0.8
                } else {
                    0.2
                }
            });
            Ok((prob, bits))
        }
    }

    fn write_wav(
        dir: &Path,
        name: &str,
        rate: u32,
        samples: &[f32],
        tagged: bool,
    ) -> std::path::PathBuf {
        let p = dir.join(name);
        let bytes = if tagged {
            crate::tts::engine::wav_bytes(rate, samples)
        } else {
            crate::archive::audio::wav_bytes(samples) // 16 kHz, no INFO
        };
        std::fs::write(&p, bytes).unwrap();
        p
    }

    fn check(path: &Path, code: u16) -> (CheckResult, Vec<usize>) {
        let file = crate::config_io::open_picked_file(path, EXTENSIONS, MAX_FILE_BYTES).unwrap();
        let mut d = LoudIsMarked {
            code,
            windows: Vec::new(),
        };
        let r = check_file(path, file, &mut d).unwrap();
        (r, d.windows)
    }

    #[test]
    fn a_tagged_24_khz_preview_with_the_mark_is_made_by_sussurro() {
        let dir = tempfile::tempdir().unwrap();
        // 25 s at 24 kHz, loud all through: three detector windows.
        let p = write_wav(
            dir.path(),
            "preview.wav",
            24_000,
            &vec![0.3; 24_000 * 25],
            true,
        );
        let (r, windows) = check(&p, PAYLOAD);
        assert_eq!(
            windows,
            [WINDOW, WINDOW, 80_000],
            "≤ 10 s, padded to the hop"
        );
        assert_eq!(r.format, "WAV");
        assert!((r.seconds - 25.0).abs() < 0.01, "{}", r.seconds);
        assert!(!r.truncated && !r.short);
        assert_eq!(r.watermark.verdict, Verdict::Found);
        assert_eq!(r.watermark.bit_errors, 0);
        assert!(r.watermark.frames_marked > 0.99);
        assert_eq!(r.metadata.status, MetadataStatus::Sussurro);
        assert!(r.metadata.tags.iter().any(|(k, _)| k == "ICMT"));
        assert_eq!(r.signature.status, SignatureStatus::None, "not signed");
        assert_eq!(r.summary, Summary::MadeBySussurro);
        assert_eq!(r.file_name, "preview.wav");
    }

    #[test]
    fn marked_frames_with_another_payload_are_inconclusive() {
        let dir = tempfile::tempdir().unwrap();
        let p = write_wav(
            dir.path(),
            "tone.wav",
            16_000,
            &vec![0.3; 16_000 * 4],
            false,
        );
        let (r, _) = check(&p, !PAYLOAD);
        assert_eq!(r.watermark.verdict, Verdict::Inconclusive);
        assert_eq!(r.watermark.bit_errors, 16);
        assert_eq!(r.metadata.status, MetadataStatus::None);
        assert_eq!(r.summary, Summary::Inconclusive);
    }

    #[test]
    fn quiet_audio_has_no_mark_and_short_files_say_so() {
        let dir = tempfile::tempdir().unwrap();
        let p = write_wav(
            dir.path(),
            "quiet.wav",
            16_000,
            &vec![0.01; 16_000 * 2],
            false,
        );
        let (r, _) = check(&p, PAYLOAD);
        assert_eq!(
            r.watermark.verdict,
            Verdict::NotFound,
            "the payload alone is not enough"
        );
        assert_eq!(r.summary, Summary::NoMark);
        assert!(r.short);
        // Tagged by Sussurro, but no watermark: the tags alone answer.
        let p = write_wav(
            dir.path(),
            "tagged.wav",
            24_000,
            &vec![0.01; 24_000 * 4],
            true,
        );
        let (r, _) = check(&p, PAYLOAD);
        assert_eq!(r.summary, Summary::TagsOnly);
    }

    #[test]
    fn an_ogg_opus_speech_file_is_decoded_at_16_khz_with_its_tags() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("speech.opus");
        let marker_tags = vec![
            ("SYNTHETIC".to_string(), "1".to_string()),
            ("ENCODER".to_string(), crate::tts::marking::generator()),
            ("TTS_VOICE".to_string(), "alba".to_string()),
            ("ARTIST".to_string(), "not shown".to_string()),
        ];
        let mut w = crate::archive::opus::OpusWriter::create_with(
            &p,
            crate::archive::audio::MAX_SAMPLES,
            crate::archive::opus::SPEECH,
            &marker_tags,
        )
        .unwrap();
        let tone: Vec<f32> = (0..24_000 * 5)
            .map(|i| 0.4 * (i as f32 * 440.0 * std::f32::consts::TAU / 24_000.0).sin())
            .collect();
        w.write(&tone).unwrap();
        w.finish().unwrap();
        let (r, _) = check(&p, PAYLOAD);
        assert_eq!(r.format, "Ogg Opus");
        assert!((r.seconds - 5.0).abs() < 0.01, "{}", r.seconds);
        assert_eq!(r.metadata.status, MetadataStatus::Sussurro);
        assert!(r.metadata.tags.iter().all(|(k, _)| k != "ARTIST"));
        assert_eq!(r.summary, Summary::MadeBySussurro);
    }

    #[test]
    fn the_picker_guard_refuses_other_files() {
        let dir = tempfile::tempdir().unwrap();
        let txt = dir.path().join("notes.txt");
        std::fs::write(&txt, b"hi").unwrap();
        let err = crate::config_io::open_picked_file(&txt, EXTENSIONS, MAX_FILE_BYTES)
            .unwrap_err()
            .to_string();
        assert!(err.contains(".opus, .ogg, .wav"), "{err}");
        // Not audio at all, but named .wav.
        let fake = dir.path().join("fake.wav");
        std::fs::write(&fake, b"not a wav file at all").unwrap();
        let file = crate::config_io::open_picked_file(&fake, EXTENSIONS, MAX_FILE_BYTES).unwrap();
        let mut d = LoudIsMarked {
            code: PAYLOAD,
            windows: Vec::new(),
        };
        assert!(check_file(&fake, file, &mut d).is_err());
        #[cfg(unix)]
        {
            let link = dir.path().join("link.wav");
            std::os::unix::fs::symlink(&fake, &link).unwrap();
            assert!(crate::config_io::open_picked_file(&link, EXTENSIONS, MAX_FILE_BYTES).is_err());
        }
    }

    #[test]
    fn tag_rules() {
        let t = |v: &[(&str, &str)]| -> Vec<(String, String)> {
            v.iter()
                .map(|(a, b)| (a.to_string(), b.to_string()))
                .collect()
        };
        assert_eq!(
            metadata_status(&t(&[("SYNTHETIC", "1"), ("ENCODER", "Sussurro 0.12.0")])),
            MetadataStatus::Sussurro
        );
        assert_eq!(
            metadata_status(&t(&[("SYNTHETIC", "1"), ("ENCODER", "OtherTTS")])),
            MetadataStatus::Synthetic
        );
        assert_eq!(
            metadata_status(&t(&[(
                "DIGITAL_SOURCE_TYPE",
                crate::tts::marking::DIGITAL_SOURCE_TYPE
            )])),
            MetadataStatus::Synthetic
        );
        assert_eq!(
            metadata_status(&t(&[("ICMT", crate::tts::engine::SYNTHETIC_COMMENT)])),
            MetadataStatus::Sussurro
        );
        assert_eq!(
            metadata_status(&t(&[("ENCODER", "Sussurro 1")])),
            MetadataStatus::None
        );
        assert_eq!(metadata_status(&[]), MetadataStatus::None);
        let long = "x".repeat(500);
        let kept = relevant(t(&[("comment", &long), ("TITLE", "t")]));
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].0, "COMMENT");
        assert_eq!(kept[0].1.chars().count(), MAX_TAG_CHARS);
    }

    #[test]
    fn summaries() {
        use MetadataStatus as M;
        let none = SignatureLayer::none();
        let signed = SignatureLayer {
            status: SignatureStatus::Valid,
            claims_sussurro: true,
            ..SignatureLayer::none()
        };
        let other = SignatureLayer {
            claims_sussurro: false,
            ..signed.clone()
        };
        let broken = SignatureLayer {
            status: SignatureStatus::Invalid,
            ..signed.clone()
        };
        assert_eq!(
            summarize(Verdict::Found, M::None, &none),
            Summary::MadeBySussurro
        );
        assert_eq!(
            summarize(Verdict::Found, M::None, &broken),
            Summary::MadeBySussurro
        );
        assert_eq!(
            summarize(Verdict::Inconclusive, M::Sussurro, &signed),
            Summary::Inconclusive
        );
        assert_eq!(
            summarize(Verdict::NotFound, M::None, &signed),
            Summary::SignedOnly
        );
        assert_eq!(
            summarize(Verdict::NotFound, M::Sussurro, &other),
            Summary::TagsOnly
        );
        assert_eq!(
            summarize(Verdict::NotFound, M::Sussurro, &broken),
            Summary::TagsOnly
        );
        assert_eq!(
            summarize(Verdict::NotFound, M::Sussurro, &none),
            Summary::TagsOnly
        );
        assert_eq!(
            summarize(Verdict::NotFound, M::Synthetic, &none),
            Summary::NoMark
        );
        assert_eq!(
            summarize(Verdict::NotFound, M::NotRead, &broken),
            Summary::NoMark
        );
    }

    /// An Ogg Opus speech file signed like read aloud does it.
    fn signed_opus(dir: &Path, name: &str) -> std::path::PathBuf {
        let p = dir.join(name);
        let mut w = crate::archive::opus::OpusWriter::create_with(
            &p,
            crate::archive::audio::MAX_SAMPLES,
            crate::archive::opus::SPEECH,
            &[("SYNTHETIC".to_string(), "1".to_string())],
        )
        .unwrap();
        w.write(&vec![0.01f32; 24_000 * 4]).unwrap();
        w.finish().unwrap();
        let id = crate::tts::signing::tests::test_identity(&dir.join("c2pa"));
        let prov = crate::tts::marking::Provenance {
            engine: "Pocket TTS".into(),
            voice: "alba".into(),
            language: "en".into(),
            voice_b: None,
        };
        let manifest =
            crate::tts::signing::sign_sidecar(&id, &prov, "2026-09-28T10:00:00Z", &p).unwrap();
        std::fs::write(crate::tts::signing::sidecar_path(&p), manifest).unwrap();
        p
    }

    #[test]
    fn a_sidecar_next_to_the_file_is_checked() {
        let dir = tempfile::tempdir().unwrap();
        let p = signed_opus(dir.path(), "speech.opus");
        // Quiet audio: the fake detector finds no watermark, the signature answers.
        let (r, _) = check(&p, PAYLOAD);
        assert_eq!(
            r.signature.status,
            SignatureStatus::Valid,
            "{}",
            r.signature.problem
        );
        assert_eq!(r.signature.source, Some(SignatureSource::Sidecar));
        assert!(r.signature.claims_sussurro && r.signature.ai_generated);
        assert!(r.signature.signer.starts_with("Sussurro install "));
        assert_eq!(r.signature.voice, "alba");
        assert_eq!(r.summary, Summary::SignedOnly);

        // One byte changed after signing.
        let mut bytes = std::fs::read(&p).unwrap();
        let at = bytes.len() / 2;
        bytes[at] ^= 1;
        std::fs::write(&p, bytes).unwrap();
        let (r, _) = check(&p, PAYLOAD);
        assert_eq!(r.signature.status, SignatureStatus::Invalid);
        assert!(
            r.signature.problem.contains("dataHash.mismatch"),
            "{}",
            r.signature.problem
        );
        assert_ne!(r.summary, Summary::SignedOnly);

        // Another file's sidecar.
        let q = signed_opus(dir.path(), "other.opus");
        std::fs::copy(
            crate::tts::signing::sidecar_path(&q),
            crate::tts::signing::sidecar_path(&p),
        )
        .unwrap();
        let (r, _) = check(&p, PAYLOAD);
        assert_eq!(r.signature.status, SignatureStatus::Invalid);

        // No sidecar at all.
        std::fs::remove_file(crate::tts::signing::sidecar_path(&p)).unwrap();
        let (r, _) = check(&p, PAYLOAD);
        assert_eq!(r.signature, SignatureLayer::none());

        // A link or an oversized file is never read.
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(
                crate::tts::signing::sidecar_path(&q),
                crate::tts::signing::sidecar_path(&p),
            )
            .unwrap();
            let (r, _) = check(&p, PAYLOAD);
            assert_eq!(r.signature.status, SignatureStatus::Invalid);
            assert!(
                r.signature.problem.contains("not read"),
                "{}",
                r.signature.problem
            );
            std::fs::remove_file(crate::tts::signing::sidecar_path(&p)).unwrap();
        }
        std::fs::write(
            crate::tts::signing::sidecar_path(&p),
            vec![0u8; crate::tts::signing::MAX_SIDECAR_BYTES as usize + 1],
        )
        .unwrap();
        let (r, _) = check(&p, PAYLOAD);
        assert_eq!(r.signature.status, SignatureStatus::Invalid);
        assert!(
            r.signature.problem.contains("not read"),
            "{}",
            r.signature.problem
        );
    }

    #[test]
    fn a_signed_preview_wav_answers_from_inside() {
        let dir = tempfile::tempdir().unwrap();
        let id = crate::tts::signing::tests::test_identity(dir.path());
        let prov = crate::tts::marking::Provenance {
            engine: "Pocket TTS".into(),
            voice: "giovanni".into(),
            language: "it".into(),
            voice_b: None,
        };
        let wav = crate::tts::engine::wav_bytes(24_000, &vec![0.3; 24_000 * 4]);
        let signed =
            crate::tts::signing::embed_in_wav(&id, &prov, "2026-09-28T10:00:00Z", &wav).unwrap();
        let p = dir.path().join("preview.wav");
        std::fs::write(&p, signed).unwrap();
        let (r, _) = check(&p, PAYLOAD);
        assert!(
            (r.seconds - 4.0).abs() < 0.01,
            "the manifest chunk is not audio: {}",
            r.seconds
        );
        assert_eq!(
            r.signature.status,
            SignatureStatus::Valid,
            "{}",
            r.signature.problem
        );
        assert_eq!(r.signature.source, Some(SignatureSource::Embedded));
        assert_eq!(r.metadata.status, MetadataStatus::Sussurro);
        assert_eq!(r.summary, Summary::MadeBySussurro);
    }

    #[test]
    fn wav_info_is_found_after_the_data_too() {
        let mut bytes = crate::archive::audio::wav_bytes(&[0.0; 10]);
        // Append a LIST/INFO chunk after `data` and fix the RIFF size.
        let mut list = b"INFO".to_vec();
        list.extend_from_slice(b"ICMT");
        list.extend_from_slice(&3u32.to_le_bytes());
        list.extend_from_slice(b"hi\0\0");
        bytes.extend_from_slice(b"LIST");
        bytes.extend_from_slice(&(list.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&list);
        let riff = (bytes.len() - 8) as u32;
        bytes[4..8].copy_from_slice(&riff.to_le_bytes());
        let info = read_wav_info(&mut std::io::Cursor::new(bytes));
        assert_eq!(info, [("ICMT".to_string(), "hi".to_string())]);
        assert!(read_wav_info(&mut std::io::Cursor::new(b"OggS....".to_vec())).is_empty());
    }
}
