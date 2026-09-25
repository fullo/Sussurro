use anyhow::{anyhow, Context, Result};
use std::path::Path;
use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::DecoderOptions;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

use crate::archive::opus::OpusReader;
use crate::audio::recorder::TARGET_RATE;
use crate::audio::resample::{downmix_to_mono, resample_linear, StreamResampler};

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

/// Streamed decode of an audio file from its path (long-form engine, #113):
/// packets are decoded, downmixed and resampled to 16 kHz mono one at a time
/// through [`StreamResampler`] (bit-identical to the batch path), so an
/// hour-long file never sits in RAM — only the current packet does.
///
/// Ogg Opus (saved audio of #247, or any mono/stereo Opus file) is decoded
/// by libopus through [`OpusReader`] — symphonia has no Opus decoder — so
/// every reader of saved audio (Identify voices, #248) takes both formats.
pub struct FileStream {
    inner: Inner,
    /// Total length announced by the container, if any.
    pub duration_ms: Option<u64>,
}

enum Inner {
    Symphonia(Box<SymphoniaStream>),
    Opus(Box<OpusReader>),
}

struct SymphoniaStream {
    format: Box<dyn symphonia::core::formats::FormatReader>,
    decoder: Box<dyn symphonia::core::codecs::Decoder>,
    track_id: u32,
    src_rate: u32,
    /// Created on the first decoded packet (its spec gives the channels).
    resampler: Option<(usize, StreamResampler)>,
    done: bool,
}

/// Samples per chunk of an Opus file (0.25 s).
const OPUS_CHUNK: usize = TARGET_RATE as usize / 4;

/// Whether `path` starts like an Ogg file.
fn is_ogg(path: &Path) -> bool {
    use std::io::Read as _;
    let mut magic = [0u8; 4];
    std::fs::File::open(path)
        .and_then(|mut f| f.read_exact(&mut magic))
        .is_ok()
        && &magic == b"OggS"
}

impl FileStream {
    pub fn open(path: &Path) -> Result<Self> {
        if is_ogg(path) {
            // Ogg Vorbis/FLAC fall through to symphonia.
            if let Ok(r) = OpusReader::open(path) {
                return Ok(Self {
                    duration_ms: Some(r.total_samples() * 1000 / TARGET_RATE as u64),
                    inner: Inner::Opus(Box::new(r)),
                });
            }
        }
        let file = std::fs::File::open(path).context("open audio file")?;
        let mss = MediaSourceStream::new(Box::new(file), Default::default());
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
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
        let format = probed.format;
        // The first audio track: a video file downloaded from a link (#123)
        // may list its video track first.
        let track = format
            .tracks()
            .iter()
            .find(|t| {
                t.codec_params.codec != symphonia::core::codecs::CODEC_TYPE_NULL
                    && t.codec_params.sample_rate.is_some()
            })
            .or_else(|| format.default_track())
            .ok_or_else(|| anyhow!("no audio track in file"))?;
        let track_id = track.id;
        let src_rate = track.codec_params.sample_rate.unwrap_or(TARGET_RATE);
        let duration_ms = track
            .codec_params
            .n_frames
            .map(|n| n * 1000 / src_rate.max(1) as u64);
        let decoder = symphonia::default::get_codecs()
            .make(&track.codec_params, &DecoderOptions::default())
            .context("no decoder for this audio codec")?;
        Ok(Self {
            inner: Inner::Symphonia(Box::new(SymphoniaStream {
                format,
                decoder,
                track_id,
                src_rate,
                resampler: None,
                done: false,
            })),
            duration_ms,
        })
    }

    /// The next chunk of 16 kHz mono samples; `None` at end of stream. A
    /// chunk can be empty (a packet whose output the resampler holds back).
    pub fn next_chunk(&mut self) -> Result<Option<Vec<f32>>> {
        match &mut self.inner {
            Inner::Symphonia(s) => s.next_chunk(),
            Inner::Opus(r) => {
                let mut out = Vec::with_capacity(OPUS_CHUNK);
                Ok((r.read(&mut out, OPUS_CHUNK)? > 0).then_some(out))
            }
        }
    }
}

impl SymphoniaStream {
    fn next_chunk(&mut self) -> Result<Option<Vec<f32>>> {
        if self.done {
            return Ok(None);
        }
        loop {
            // Any packet error means end of stream (same as the batch path).
            let Ok(packet) = self.format.next_packet() else {
                self.done = true;
                let tail = self
                    .resampler
                    .as_mut()
                    .map(|(_, r)| r.flush())
                    .unwrap_or_default();
                return Ok(if tail.is_empty() && self.resampler.is_none() {
                    None
                } else {
                    Some(tail)
                });
            };
            if packet.track_id() != self.track_id {
                continue;
            }
            let decoded = match self.decoder.decode(&packet) {
                Ok(d) => d,
                Err(symphonia::core::errors::Error::DecodeError(_)) => continue,
                Err(e) => return Err(anyhow!("decode error: {e}")),
            };
            let spec = *decoded.spec();
            let channels = spec.channels.count().max(1);
            let mut buf = SampleBuffer::<f32>::new(decoded.capacity() as u64, spec);
            buf.copy_interleaved_ref(decoded);
            let mut out = Vec::new();
            // A channel-count change mid-stream (rare) restarts the resampler.
            if self.resampler.as_ref().is_some_and(|(c, _)| *c != channels) {
                if let Some((_, mut old)) = self.resampler.take() {
                    out.extend(old.flush());
                }
            }
            let (_, r) = self.resampler.get_or_insert_with(|| {
                (channels, StreamResampler::new(channels, self.src_rate, TARGET_RATE))
            });
            out.extend(r.push(buf.samples()));
            return Ok(Some(out));
        }
    }
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

/// Minimal 16-bit PCM WAV writer for tests (no extra dependency).
#[cfg(test)]
pub(crate) fn write_wav_i16(path: &Path, rate: u32, channels: u16, interleaved: &[f32]) {
    use std::io::Write as _;
    let data_len = (interleaved.len() * 2) as u32;
    let mut f = std::io::BufWriter::new(std::fs::File::create(path).unwrap());
    f.write_all(b"RIFF").unwrap();
    f.write_all(&(36 + data_len).to_le_bytes()).unwrap();
    f.write_all(b"WAVEfmt ").unwrap();
    f.write_all(&16u32.to_le_bytes()).unwrap();
    f.write_all(&1u16.to_le_bytes()).unwrap(); // PCM
    f.write_all(&channels.to_le_bytes()).unwrap();
    f.write_all(&rate.to_le_bytes()).unwrap();
    f.write_all(&(rate * channels as u32 * 2).to_le_bytes()).unwrap();
    f.write_all(&(channels * 2).to_le_bytes()).unwrap();
    f.write_all(&16u16.to_le_bytes()).unwrap();
    f.write_all(b"data").unwrap();
    f.write_all(&data_len.to_le_bytes()).unwrap();
    for s in interleaved {
        let v = (s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
        f.write_all(&v.to_le_bytes()).unwrap();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(rate: u32, channels: u16, secs: f32) -> Vec<f32> {
        let frames = (rate as f32 * secs) as usize;
        let mut out = Vec::with_capacity(frames * channels as usize);
        for i in 0..frames {
            let t = i as f32 / rate as f32;
            for c in 0..channels {
                let f = 220.0 + 110.0 * c as f32;
                out.push(0.3 * (2.0 * std::f32::consts::PI * f * t).sin());
            }
        }
        out
    }

    #[test]
    fn streamed_decode_matches_the_batch_decode() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("stereo.wav");
        write_wav_i16(&path, 44_100, 2, &tone(44_100, 2, 2.5));

        let batch = decode_to_16k_mono(&path).unwrap();
        let mut stream = FileStream::open(&path).unwrap();
        assert_eq!(stream.duration_ms, Some(2_500));
        let mut streamed = Vec::new();
        let mut chunks = 0;
        while let Some(chunk) = stream.next_chunk().unwrap() {
            streamed.extend(chunk);
            chunks += 1;
        }
        assert!(chunks > 1, "decoded as a stream, not in one piece");
        assert_eq!(streamed, batch);
        // End of stream is sticky.
        assert!(stream.next_chunk().unwrap().is_none());
    }

    #[test]
    fn streamed_decode_of_16k_mono_is_passthrough() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mono.wav");
        write_wav_i16(&path, 16_000, 1, &tone(16_000, 1, 1.0));
        let mut stream = FileStream::open(&path).unwrap();
        let mut n = 0;
        while let Some(chunk) = stream.next_chunk().unwrap() {
            n += chunk.len();
        }
        assert_eq!(n, 16_000);
    }

    #[test]
    fn opening_a_non_audio_file_fails() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("x.wav");
        std::fs::write(&path, b"definitely not audio").unwrap();
        assert!(FileStream::open(&path).is_err());
        assert!(FileStream::open(&dir.path().join("missing.mp3")).is_err());
    }

    /// Saved Opus (#247) streams through libopus (#248): the same samples
    /// as a full decode, whatever the extension says.
    #[test]
    fn ogg_opus_streams_through_libopus() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("audio.opus");
        let mut w = crate::archive::opus::OpusWriter::create_capped(&path, u64::MAX).unwrap();
        w.write(&tone(16_000, 1, 2.3)).unwrap();
        w.finish().unwrap();
        let (full, _) = crate::archive::opus::decode_file(&path).unwrap();
        for p in [path.clone(), dir.path().join("renamed.bin")] {
            if p != path {
                std::fs::copy(&path, &p).unwrap();
            }
            let mut stream = FileStream::open(&p).unwrap();
            assert_eq!(stream.duration_ms, Some(2_300));
            let mut got = Vec::new();
            while let Some(chunk) = stream.next_chunk().unwrap() {
                got.extend(chunk);
            }
            assert_eq!(got, full);
            assert!(stream.next_chunk().unwrap().is_none());
        }
        // An Ogg file that isn't Opus is still symphonia's to judge.
        let not_opus = dir.path().join("x.ogg");
        std::fs::write(&not_opus, b"OggS but nothing else").unwrap();
        assert!(FileStream::open(&not_opus).is_err());
    }
}
