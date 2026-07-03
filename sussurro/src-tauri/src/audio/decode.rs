use anyhow::{anyhow, Context, Result};
use std::path::Path;
use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::DecoderOptions;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

use crate::audio::recorder::TARGET_RATE;
use crate::audio::resample::{downmix_to_mono, resample_linear};

/// Decode a file path to 16 kHz mono f32.
pub fn decode_to_16k_mono(path: &Path) -> Result<Vec<f32>> {
    let file = std::fs::File::open(path).context("open audio file")?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
    decode_stream(mss, ext)
}

/// Decode in-memory bytes (from a browser file input) to 16 kHz mono f32.
/// `ext` is the original file extension, used as a format hint.
pub fn decode_bytes_16k_mono(bytes: Vec<u8>, ext: &str) -> Result<Vec<f32>> {
    let cursor = std::io::Cursor::new(bytes);
    let mss = MediaSourceStream::new(Box::new(cursor), Default::default());
    decode_stream(mss, ext)
}

/// Decode any symphonia-supported audio (wav/mp3/m4a/aac/flac/ogg) to 16 kHz
/// mono f32 — the same format the microphone recorder produces, so the
/// transcription path is identical.
fn decode_stream(mss: MediaSourceStream, ext: &str) -> Result<Vec<f32>> {
    let mut hint = Hint::new();
    if !ext.is_empty() {
        hint.with_extension(ext);
    }

    let probed = symphonia::default::get_probe()
        .format(
            &hint,
            mss,
            &FormatOptions::default(),
            &MetadataOptions::default(),
        )
        .context("unsupported or corrupt audio file")?;
    let mut format = probed.format;

    let track = format
        .default_track()
        .ok_or_else(|| anyhow!("no audio track in file"))?;
    let track_id = track.id;
    let src_rate = track.codec_params.sample_rate.unwrap_or(TARGET_RATE);
    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &DecoderOptions::default())
        .context("no decoder for this audio codec")?;

    let mut interleaved: Vec<f32> = Vec::new();
    let mut channels = 0usize;

    // Any packet error means end of stream.
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
        channels = spec.channels.count();
        let mut buf = SampleBuffer::<f32>::new(decoded.capacity() as u64, spec);
        buf.copy_interleaved_ref(decoded);
        interleaved.extend_from_slice(buf.samples());
    }

    if interleaved.is_empty() || channels == 0 {
        anyhow::bail!("no audio samples decoded");
    }
    let mono = downmix_to_mono(&interleaved, channels);
    Ok(resample_linear(&mono, src_rate, TARGET_RATE))
}
