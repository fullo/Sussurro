//! Saved audio of an item (0.10, #141, P9: only on request).
//!
//! **Layout** — mono 16 kHz audio (what the engine transcribes), one file
//! per logical channel, in the item folder, in the format of the *Saved
//! audio format* setting ([`AudioFormat`], #247): 16-bit PCM WAV, or Ogg
//! Opus at 24 kb/s ([`super::opus`], about 10× smaller):
//!
//! ```text
//! audio.wav                        one channel (mic, file, link)
//! audio-mic.wav + audio-remote.wav browser meeting (#126)
//! audio-mic.wav + audio-system.wav system audio + mic (#139)
//! audio.opus, audio-mic.opus …     the same, as Ogg Opus
//! ```
//!
//! Every file starts at the session's t = 0 (a channel that joins late is
//! padded with silence), so a segment's `start_ms` is also its position in
//! the file. While recording each channel is written as
//! `audio-<channel>.<ext>`; when the session ends (or is recovered) with a
//! single channel file, it is renamed `audio.<ext>`. The format is chosen
//! when a run starts; items saved earlier keep theirs.
//!
//! **Why mono per channel, not one stereo file**: the channels arrive
//! independently (a browser's remote track may start late or stall), so
//! interleaving would need an alignment buffer that grows while one side
//! lags; separate files are written as frames come, with no buffering
//! beyond the `BufWriter`. Per-speaker replay (#142) plays one side alone,
//! and one file per channel works the same for one channel or two.
//!
//! **Incremental and crash-safe**: [`WavWriter`] writes a 44-byte header
//! with zero sizes, appends samples through a buffer, and patches the sizes
//! every [`PATCH_EVERY_SAMPLES`] and on [`WavWriter::finish`]. After a
//! crash, [`repair`] recomputes the sizes from the file length (the startup
//! recovery of #153 does this for items it marks `interrupted`), so the
//! saved audio plays up to the last buffer that reached the disk.
//!
//! **Size cap, no RF64**: a WAV's sizes are 32-bit, so a file holds at most
//! [`MAX_DATA_BYTES`] of samples — at 32 000 bytes/s about 37 h 17 min per
//! channel. A channel that reaches it stops being saved (the file stays a
//! valid WAV and the transcription goes on); RF64 is not needed for any
//! realistic session. An Opus file has no such limit but takes the same
//! duration cap ([`MAX_SAMPLES`]), so both formats behave alike.
//!
//! The frontmatter lists the files under the app-owned key [`AUDIO_KEY`]
//! (`audio: [audio.wav]` or `audio: [audio.opus]`: the extension records
//! the format), kept in [`ItemMeta::extra`] like the session
//! marker so a UI that round-trips only the fields it knows never drops it.
//! What the UI shows, and what "Delete audio" removes, is the files actually
//! in the folder: names are matched against a fixed pattern
//! ([`is_audio_file_name`]), never taken from the frontmatter as paths.

use super::opus::OpusWriter;
use super::types::{Channel, ItemMeta};
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

/// Sample rate of saved audio: the engine's.
pub const RATE: u32 = 16_000;
/// The WAV file of a single-channel item.
pub const AUDIO_FILE: &str = "audio.wav";
/// Frontmatter key listing the item's audio files (app-owned).
pub const AUDIO_KEY: &str = "audio";
/// Canonical PCM header length.
pub const HEADER_LEN: u64 = 44;
/// Largest data chunk a 32-bit RIFF size can describe (`36 + data` must fit
/// in a `u32`), rounded down to whole 16-bit samples.
pub const MAX_DATA_BYTES: u64 = (u32::MAX as u64 - 36) & !1;
const _: () = assert!(36 + MAX_DATA_BYTES <= u32::MAX as u64);
/// Patch the header sizes after this many samples (10 s), so a file copied
/// or synced mid-session is already a valid WAV of most of the audio.
pub const PATCH_EVERY_SAMPLES: u64 = 10 * RATE as u64;
/// Per-channel duration cap of either format: what a WAV can hold
/// ([`MAX_DATA_BYTES`] of 16-bit samples, about 37 h 17 min).
pub const MAX_SAMPLES: u64 = MAX_DATA_BYTES / 2;

/// The *Saved audio format* setting (#247, P16): the format of the files a
/// run saves. Items saved earlier keep theirs; the file extension tells
/// which is which.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AudioFormat {
    /// Mono 16-bit PCM, 16 kHz (about 115 MB per hour).
    Wav,
    /// Ogg Opus, mono, 24 kb/s (about 11 MB per hour): the default for new
    /// installs since the app plays it everywhere (#248, P16). A settings
    /// file from before the setting keeps WAV ([`crate::settings`]).
    #[default]
    Opus,
}

impl AudioFormat {
    /// File extension, without the dot.
    pub fn ext(self) -> &'static str {
        match self {
            AudioFormat::Wav => "wav",
            AudioFormat::Opus => "opus",
        }
    }

    /// The format of an audio file name ([`is_audio_file_name`]). Pure.
    pub fn of_name(name: &str) -> Option<AudioFormat> {
        [AudioFormat::Wav, AudioFormat::Opus]
            .into_iter()
            .find(|f| name.strip_suffix(f.ext()).is_some_and(|s| s.ends_with('.')))
    }

    /// `audio.<ext>`: the file of a single-channel item.
    pub fn single_file_name(self) -> String {
        format!("audio.{}", self.ext())
    }
}

/// `audio-<channel>.<ext>`: the file a channel is written to while
/// recording.
pub fn channel_file_name(channel: Channel, format: AudioFormat) -> String {
    let c = match channel {
        Channel::Mic => "mic",
        Channel::Remote => "remote",
        Channel::System => "system",
        Channel::File => "file",
    };
    format!("audio-{c}.{}", format.ext())
}

/// `audio.<ext>` or `audio-<lowercase letters>.<ext>`, with `<ext>` `wav`
/// or `opus`: the only names the app treats as an item's audio (so nothing
/// else in the folder is ever listed, sized or trashed as audio). Pure.
pub fn is_audio_file_name(name: &str) -> bool {
    let Some(format) = AudioFormat::of_name(name) else {
        return false;
    };
    let stem = &name[..name.len() - format.ext().len() - 1];
    stem == "audio"
        || stem
            .strip_prefix("audio-")
            .is_some_and(|c| !c.is_empty() && c.bytes().all(|b| b.is_ascii_lowercase()))
}

/// The canonical 44-byte header of a mono 16-bit 16 kHz WAV holding
/// `data_bytes` of samples.
pub(crate) fn header(data_bytes: u32) -> [u8; HEADER_LEN as usize] {
    let mut h = [0u8; HEADER_LEN as usize];
    h[0..4].copy_from_slice(b"RIFF");
    h[4..8].copy_from_slice(&(36u32.wrapping_add(data_bytes)).to_le_bytes());
    h[8..16].copy_from_slice(b"WAVEfmt ");
    h[16..20].copy_from_slice(&16u32.to_le_bytes());
    h[20..22].copy_from_slice(&1u16.to_le_bytes()); // PCM
    h[22..24].copy_from_slice(&1u16.to_le_bytes()); // mono
    h[24..28].copy_from_slice(&RATE.to_le_bytes());
    h[28..32].copy_from_slice(&(RATE * 2).to_le_bytes());
    h[32..34].copy_from_slice(&2u16.to_le_bytes()); // block align
    h[34..36].copy_from_slice(&16u16.to_le_bytes());
    h[36..40].copy_from_slice(b"data");
    h[40..44].copy_from_slice(&data_bytes.to_le_bytes());
    h
}

/// Whether `h` is the canonical header [`WavWriter`] writes (mono 16-bit
/// 16 kHz PCM, `data` right after `fmt `), whatever its sizes say.
pub(crate) fn is_our_header(h: &[u8; HEADER_LEN as usize]) -> bool {
    &h[0..4] == b"RIFF"
        && &h[8..16] == b"WAVEfmt "
        && h[16..20] == 16u32.to_le_bytes()
        && h[20..22] == 1u16.to_le_bytes()
        && h[22..24] == 1u16.to_le_bytes()
        && h[24..28] == RATE.to_le_bytes()
        && h[34..36] == 16u16.to_le_bytes()
        && &h[36..40] == b"data"
}

/// Write the two size fields of a canonical header in place.
fn patch_sizes(file: &mut File, data_bytes: u64) -> std::io::Result<()> {
    let data = u32::try_from(data_bytes).unwrap_or(u32::MAX);
    file.seek(SeekFrom::Start(4))?;
    file.write_all(&(36u32.saturating_add(data)).to_le_bytes())?;
    file.seek(SeekFrom::Start(40))?;
    file.write_all(&data.to_le_bytes())?;
    Ok(())
}

/// f32 in [-1, 1] → 16-bit PCM (clamped).
pub(crate) fn to_i16(s: f32) -> i16 {
    (s.clamp(-1.0, 1.0) * i16::MAX as f32).round() as i16
}

/// A whole mono 16-bit 16 kHz WAV in memory: the upload format of the
/// sidecar STT client (`stt::remote`, #117). Callers keep the input short
/// (≤ 30 s, ~1 MB); longer input is capped at [`MAX_DATA_BYTES`]. Pure.
pub fn wav_bytes(samples: &[f32]) -> Vec<u8> {
    let n = samples.len().min((MAX_DATA_BYTES / 2) as usize);
    let mut out = Vec::with_capacity(HEADER_LEN as usize + n * 2);
    out.extend_from_slice(&header((n * 2) as u32));
    for &s in &samples[..n] {
        out.extend_from_slice(&to_i16(s).to_le_bytes());
    }
    out
}

/// Incremental mono 16-bit WAV writer (see the module docs).
pub struct WavWriter {
    path: PathBuf,
    out: BufWriter<File>,
    data_bytes: u64,
    max_data_bytes: u64,
    since_patch: u64,
}

impl WavWriter {
    /// Create `path` — never over an existing file — with an empty header.
    pub fn create(path: &Path) -> Result<Self> {
        Self::create_capped(path, MAX_DATA_BYTES)
    }

    /// [`Self::create`] with a smaller cap (tests of the size guard).
    pub fn create_capped(path: &Path, max_data_bytes: u64) -> Result<Self> {
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .with_context(|| format!("creating {}", path.display()))?;
        let mut out = BufWriter::with_capacity(64 * 1024, file);
        out.write_all(&header(0))
            .with_context(|| format!("writing {}", path.display()))?;
        Ok(Self {
            path: path.to_path_buf(),
            out,
            data_bytes: 0,
            max_data_bytes: max_data_bytes.min(MAX_DATA_BYTES) & !1,
            since_patch: 0,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Samples written so far.
    pub fn samples(&self) -> u64 {
        self.data_bytes / 2
    }

    /// The size cap was reached: further samples are dropped.
    pub fn is_full(&self) -> bool {
        self.data_bytes >= self.max_data_bytes
    }

    fn room(&self) -> usize {
        usize::try_from((self.max_data_bytes - self.data_bytes.min(self.max_data_bytes)) / 2)
            .unwrap_or(usize::MAX)
    }

    /// Append samples (up to the cap). Returns how many were written.
    pub fn write(&mut self, samples: &[f32]) -> Result<usize> {
        let n = samples.len().min(self.room());
        let mut buf = Vec::with_capacity(n * 2);
        for &s in &samples[..n] {
            buf.extend_from_slice(&to_i16(s).to_le_bytes());
        }
        self.append(&buf, n as u64)?;
        Ok(n)
    }

    /// Append `n` samples of silence (up to the cap), in bounded chunks.
    pub fn write_silence(&mut self, n: u64) -> Result<u64> {
        const CHUNK: usize = 16_000;
        let zeros = [0u8; CHUNK * 2];
        let n = n.min(self.room() as u64);
        let mut left = n;
        while left > 0 {
            let k = left.min(CHUNK as u64);
            self.append(&zeros[..k as usize * 2], k)?;
            left -= k;
        }
        Ok(n)
    }

    fn append(&mut self, bytes: &[u8], samples: u64) -> Result<()> {
        if bytes.is_empty() {
            return Ok(());
        }
        self.out
            .write_all(bytes)
            .with_context(|| format!("writing {}", self.path.display()))?;
        self.data_bytes += bytes.len() as u64;
        self.since_patch += samples;
        if self.since_patch >= PATCH_EVERY_SAMPLES {
            self.patch_header()?;
        }
        Ok(())
    }

    /// Flush the buffer and write the current sizes into the header.
    pub fn patch_header(&mut self) -> Result<()> {
        self.since_patch = 0;
        let data = self.data_bytes;
        let path = &self.path;
        (|| -> std::io::Result<()> {
            self.out.flush()?;
            let file = self.out.get_mut();
            patch_sizes(file, data)?;
            file.seek(SeekFrom::End(0))?;
            Ok(())
        })()
        .with_context(|| format!("updating the header of {}", path.display()))
    }

    /// Final header, flushed to disk. Returns the data size in bytes.
    pub fn finish(mut self) -> Result<u64> {
        self.patch_header()?;
        self.out
            .get_ref()
            .sync_all()
            .with_context(|| format!("saving {}", self.path.display()))?;
        Ok(self.data_bytes)
    }
}

/// The writer of one channel's file, in either format: what the engine's
/// audio output ([`crate::engine::audio_out`]) drives. Both keep the same
/// clock (samples written), cap and failure rules.
pub enum AudioWriter {
    Wav(WavWriter),
    Opus(OpusWriter),
}

impl AudioWriter {
    /// Create `path` in `format` — never over an existing file — with a
    /// duration cap of `max_samples` (at most [`MAX_SAMPLES`]).
    pub fn create(path: &Path, format: AudioFormat, max_samples: u64) -> Result<Self> {
        let max_samples = max_samples.min(MAX_SAMPLES);
        Ok(match format {
            AudioFormat::Wav => AudioWriter::Wav(WavWriter::create_capped(path, max_samples * 2)?),
            AudioFormat::Opus => AudioWriter::Opus(OpusWriter::create_capped(path, max_samples)?),
        })
    }

    pub fn path(&self) -> &Path {
        match self {
            AudioWriter::Wav(w) => w.path(),
            AudioWriter::Opus(w) => w.path(),
        }
    }

    /// Samples written so far: the file's position on the session clock.
    pub fn samples(&self) -> u64 {
        match self {
            AudioWriter::Wav(w) => w.samples(),
            AudioWriter::Opus(w) => w.samples(),
        }
    }

    /// The cap was reached: further samples are dropped.
    pub fn is_full(&self) -> bool {
        match self {
            AudioWriter::Wav(w) => w.is_full(),
            AudioWriter::Opus(w) => w.is_full(),
        }
    }

    /// Append samples (up to the cap). Returns how many were written.
    pub fn write(&mut self, samples: &[f32]) -> Result<usize> {
        match self {
            AudioWriter::Wav(w) => w.write(samples),
            AudioWriter::Opus(w) => w.write(samples),
        }
    }

    /// Append `n` samples of silence (up to the cap).
    pub fn write_silence(&mut self, n: u64) -> Result<u64> {
        match self {
            AudioWriter::Wav(w) => w.write_silence(n),
            AudioWriter::Opus(w) => w.write_silence(n),
        }
    }

    /// Close the file (final header, or the end of the Opus stream) and
    /// sync it to disk.
    pub fn finish(self) -> Result<()> {
        match self {
            AudioWriter::Wav(w) => w.finish().map(drop),
            AudioWriter::Opus(w) => w.finish().map(drop),
        }
    }
}

/// Make a WAV left by a crash valid again: sizes recomputed from the file
/// length (a trailing odd byte dropped), or a fresh empty header if the
/// crash hit before the header was complete. Only the app's own canonical
/// 44-byte header is rewritten; any other file is refused, untouched.
/// Returns the data size in bytes.
pub fn repair(path: &Path) -> Result<u64> {
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .with_context(|| format!("opening {}", path.display()))?;
    let len = file.metadata()?.len();
    if len < HEADER_LEN {
        let mut head = Vec::new();
        file.read_to_end(&mut head)?;
        if !b"RIFF".starts_with(&head[..head.len().min(4)]) {
            bail!("{} is not a WAV written by Sussurro", path.display());
        }
        file.set_len(0)?;
        file.seek(SeekFrom::Start(0))?;
        file.write_all(&header(0))?;
        file.sync_all()?;
        return Ok(0);
    }
    let mut h = [0u8; HEADER_LEN as usize];
    file.read_exact(&mut h)?;
    if !is_our_header(&h) {
        bail!("{} is not a WAV written by Sussurro", path.display());
    }
    let data = ((len - HEADER_LEN) & !1).min(MAX_DATA_BYTES);
    if HEADER_LEN + data != len {
        file.set_len(HEADER_LEN + data)?;
    }
    patch_sizes(&mut file, data)?;
    file.sync_all()?;
    Ok(data)
}

/// [`repair`] a WAV or [`super::opus::repair`] an Opus file, by its name.
/// Returns the samples kept.
pub fn repair_any(path: &Path) -> Result<u64> {
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    match AudioFormat::of_name(name) {
        Some(AudioFormat::Wav) => repair(path).map(|bytes| bytes / 2),
        Some(AudioFormat::Opus) => super::opus::repair(path),
        None => bail!("{} is not a saved audio file", path.display()),
    }
}

/// The item's audio files present in `dir`, by name, sorted.
pub fn files_in(dir: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<String> = entries
        .flatten()
        .filter(|e| e.file_type().map(|t| t.is_file()).unwrap_or(false))
        .filter_map(|e| e.file_name().to_str().map(str::to_string))
        .filter(|n| is_audio_file_name(n))
        .collect();
    out.sort();
    out
}

/// One channel's file alone (`audio-mic.wav`, no `audio.wav`) becomes
/// `audio.wav` (`audio-mic.opus` → `audio.opus`). Returns the files present
/// afterwards. A failed rename keeps the channel name — the file is still
/// listed and playable.
pub fn settle_names(dir: &Path) -> Vec<String> {
    let files = files_in(dir);
    if let [only] = files.as_slice() {
        let single = AudioFormat::of_name(only).map(AudioFormat::single_file_name);
        if let Some(single) = single.filter(|s| s != only) {
            match std::fs::rename(dir.join(only), dir.join(&single)) {
                Ok(()) => return vec![single],
                Err(e) => eprintln!("archive: keeping {only} ({e})"),
            }
        }
    }
    files
}

/// After a crash: [`repair_any`] every audio file of the item folder, then
/// [`settle_names`]. Returns the files to record in the frontmatter.
pub fn recover_files(dir: &Path) -> Vec<String> {
    for name in files_in(dir) {
        match repair_any(&dir.join(&name)) {
            Ok(samples) => eprintln!(
                "archive: {name} repaired ({:.1} s of audio kept)",
                samples as f64 / RATE as f64
            ),
            Err(e) => eprintln!("archive: {name} left as is ({e:#})"),
        }
    }
    settle_names(dir)
}

/// The audio files the frontmatter lists (only well-formed names).
pub fn listed(meta: &ItemMeta) -> Vec<String> {
    match meta.extra.get(AUDIO_KEY) {
        Some(serde_json::Value::Array(xs)) => xs
            .iter()
            .filter_map(|x| x.as_str())
            .filter(|n| is_audio_file_name(n))
            .map(str::to_string)
            .collect(),
        Some(serde_json::Value::String(s)) if is_audio_file_name(s) => vec![s.clone()],
        _ => Vec::new(),
    }
}

/// Record `files` under [`AUDIO_KEY`]; an empty list removes the key.
pub fn set_listed(meta: &mut ItemMeta, files: &[String]) {
    if files.is_empty() {
        meta.extra.remove(AUDIO_KEY);
    } else {
        meta.extra.insert(
            AUDIO_KEY.to_string(),
            serde_json::Value::Array(files.iter().cloned().map(Into::into).collect()),
        );
    }
}

/// One saved audio file, as the UI shows it.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct AudioFile {
    pub name: String,
    pub bytes: u64,
}

/// The audio files in the item folder `dir`, with their sizes.
pub fn files_with_sizes(dir: &Path) -> Vec<AudioFile> {
    files_in(dir)
        .into_iter()
        .map(|name| AudioFile {
            bytes: std::fs::metadata(dir.join(&name))
                .map(|m| m.len())
                .unwrap_or(0),
            name,
        })
        .collect()
}

/// Total size of an item folder (files in it and in its sub-folders; links
/// are not followed).
pub fn folder_bytes(dir: &Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    entries
        .flatten()
        .map(|e| match e.file_type() {
            Ok(t) if t.is_dir() => folder_bytes(&e.path()),
            Ok(t) if t.is_file() => e.metadata().map(|m| m.len()).unwrap_or(0),
            _ => 0,
        })
        .sum()
}

/// "Delete audio, keep transcript": every audio file of the item goes to the
/// OS trash (like the rest of the archive, never a hard delete) and the
/// frontmatter stops listing it; transcript, segments and everything else
/// stay. Refused while the item is being recorded. Returns how many files
/// went to the trash.
pub fn delete_audio(archive: &Path, id: &str) -> Result<usize> {
    use super::store::{
        existing_item_dir, lock_items, move_to_trash, read_segments, transcript_path,
    };
    let _lock = lock_items();
    let dir = existing_item_dir(archive, id)?;
    let path = transcript_path(&dir);
    let bytes = std::fs::read(&path).with_context(|| format!("reading {}", path.display()))?;
    let parsed = super::frontmatter::parse(&String::from_utf8_lossy(&bytes));
    if let Ok((m, _)) = &parsed {
        if m.session_state() == Some(super::types::SessionState::Recording) {
            bail!("'{id}' is still being recorded — stop the session before deleting its audio");
        }
    }
    let files = files_in(&dir);
    for name in &files {
        move_to_trash(&dir.join(name))?;
    }
    // The audio is gone; the list in the frontmatter is only a record, so
    // a frontmatter that can't be rewritten (broken YAML) is left to the
    // user rather than failing a delete that already happened.
    if parsed
        .map(|(m, _)| m.extra.contains_key(AUDIO_KEY))
        .unwrap_or(false)
    {
        let segments = read_segments(&dir).unwrap_or_default();
        if let Err(e) = super::live::rewrite_meta(
            &dir,
            &segments,
            None,
            &|mut m| {
                set_listed(&mut m, &[]);
                m
            },
            &|_| {},
        ) {
            eprintln!("archive: {id}: audio deleted, frontmatter not updated ({e:#})");
        }
    }
    Ok(files.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(n: usize) -> Vec<f32> {
        (0..n).map(|i| 0.5 * ((i as f32) * 0.03).sin()).collect()
    }

    fn u32_at(bytes: &[u8], at: usize) -> u32 {
        u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap())
    }

    /// Decode with symphonia (the app's own decoder, as any player would).
    fn decode(path: &Path) -> Vec<f32> {
        let mut s = crate::audio::decode::FileStream::open(path).unwrap();
        let mut out = Vec::new();
        while let Some(chunk) = s.next_chunk().unwrap() {
            out.extend(chunk);
        }
        out
    }

    #[test]
    fn in_memory_wav_is_a_valid_16k_mono_file() {
        let samples = tone(1_000);
        let bytes = wav_bytes(&samples);
        assert_eq!(&bytes[0..4], b"RIFF");
        assert_eq!(&bytes[8..16], b"WAVEfmt ");
        assert_eq!(u32_at(&bytes, 24), RATE);
        assert_eq!(u32_at(&bytes, 40), 2_000, "data size");
        assert_eq!(u32_at(&bytes, 4), 36 + 2_000, "RIFF size");
        assert_eq!(bytes.len(), HEADER_LEN as usize + 2_000);
        // Same bytes as the file writer produces for the same samples.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.wav");
        let mut w = WavWriter::create(&path).unwrap();
        w.write(&samples).unwrap();
        w.finish().unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        assert_eq!(wav_bytes(&[]).len(), HEADER_LEN as usize);
    }

    #[test]
    fn names() {
        use AudioFormat::{Opus, Wav};
        assert_eq!(channel_file_name(Channel::Mic, Wav), "audio-mic.wav");
        assert_eq!(channel_file_name(Channel::Remote, Wav), "audio-remote.wav");
        assert_eq!(channel_file_name(Channel::System, Wav), "audio-system.wav");
        assert_eq!(channel_file_name(Channel::Mic, Opus), "audio-mic.opus");
        assert_eq!(
            channel_file_name(Channel::Remote, Opus),
            "audio-remote.opus"
        );
        assert_eq!(Wav.single_file_name(), AUDIO_FILE);
        assert_eq!(Opus.single_file_name(), "audio.opus");
        for ok in [
            "audio.wav",
            "audio-mic.wav",
            "audio-system.wav",
            "audio.opus",
            "audio-mic.opus",
            "audio-remote.opus",
        ] {
            assert!(is_audio_file_name(ok), "{ok}");
        }
        assert_eq!(AudioFormat::of_name("audio-mic.opus"), Some(Opus));
        assert_eq!(AudioFormat::of_name("audio.wav"), Some(Wav));
        assert_eq!(AudioFormat::of_name("audio.ogg"), None);
        assert_eq!(AudioFormat::of_name("audioopus"), None);
        for bad in [
            "audio-.wav",
            "audio-Mic.wav",
            "audio-../x.wav",
            "../audio.wav",
            "audio.mp3",
            "transcript.md",
            "audio-mic.wav.tmp",
            "audio-.opus",
            "audio-Mic.opus",
            "audio.ogg",
            "audio.opus.tmp",
            "audioopus",
            "audio-mic.wav.opus",
            "xaudio.opus",
            ".opus",
        ] {
            assert!(!is_audio_file_name(bad), "{bad}");
        }
    }

    #[test]
    fn header_is_patched_after_incremental_writes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("audio-mic.wav");
        let mut w = WavWriter::create(&path).unwrap();
        // Many small writes, like a mic's 250 ms polls.
        let audio = tone(40_000);
        for chunk in audio.chunks(4_000) {
            assert_eq!(w.write(chunk).unwrap(), chunk.len());
        }
        w.write_silence(1_000).unwrap();
        assert_eq!(w.samples(), 41_000);
        let bytes = w.finish().unwrap();
        assert_eq!(bytes, 82_000);

        let file = std::fs::read(&path).unwrap();
        assert_eq!(file.len() as u64, HEADER_LEN + 82_000);
        assert_eq!(&file[0..4], b"RIFF");
        assert_eq!(u32_at(&file, 4), 36 + 82_000);
        assert_eq!(&file[8..16], b"WAVEfmt ");
        assert_eq!(u32_at(&file, 24), 16_000, "sample rate");
        assert_eq!(u32_at(&file, 28), 32_000, "byte rate");
        assert_eq!(&file[36..40], b"data");
        assert_eq!(u32_at(&file, 40), 82_000);

        // It plays: a standard decoder reads every sample back.
        let back = decode(&path);
        assert_eq!(back.len(), 41_000);
        for (a, b) in audio.iter().zip(&back) {
            assert!((a - b).abs() < 1e-3, "{a} vs {b}");
        }
        assert!(back[40_000..].iter().all(|&s| s == 0.0));
    }

    #[test]
    fn never_overwrites_an_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("audio.wav");
        std::fs::write(&path, b"user's own file").unwrap();
        assert!(WavWriter::create(&path).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"user's own file");
    }

    #[test]
    fn header_is_kept_current_while_recording() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("audio.wav");
        let mut w = WavWriter::create(&path).unwrap();
        w.write(&tone(PATCH_EVERY_SAMPLES as usize + 10)).unwrap();
        // Not finished: the periodic patch already describes the audio.
        let file = std::fs::read(&path).unwrap();
        let data = u32_at(&file, 40) as u64;
        assert_eq!(data, (PATCH_EVERY_SAMPLES + 10) * 2);
        assert_eq!(file.len() as u64, HEADER_LEN + data);
        drop(w);
    }

    /// A crash: the writer never finishes, part of its buffer never reaches
    /// the disk, the last sample is cut in half. The repaired file plays
    /// every whole sample that was saved.
    #[test]
    fn truncated_file_plays_after_repair() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("audio-mic.wav");
        let mut w = WavWriter::create(&path).unwrap();
        w.write(&tone(PATCH_EVERY_SAMPLES as usize + 5_000))
            .unwrap();
        w.patch_header().unwrap();
        w.write(&tone(3_000)).unwrap();
        // Killed: no final patch, the buffered 3 000 samples are lost.
        std::mem::forget(w);
        // …and the file was cut mid-sample (a partial page write).
        let f = OpenOptions::new().write(true).open(&path).unwrap();
        let cut = HEADER_LEN + (PATCH_EVERY_SAMPLES + 4_000) * 2 + 1;
        f.set_len(cut).unwrap();
        drop(f);

        assert_eq!(repair(&path).unwrap(), (PATCH_EVERY_SAMPLES + 4_000) * 2);
        let file = std::fs::read(&path).unwrap();
        assert_eq!(file.len() as u64, cut - 1, "the half sample is dropped");
        assert_eq!(u32_at(&file, 40) as u64, (PATCH_EVERY_SAMPLES + 4_000) * 2);
        assert_eq!(
            u32_at(&file, 4) as u64,
            36 + (PATCH_EVERY_SAMPLES + 4_000) * 2
        );
        assert_eq!(decode(&path).len() as u64, PATCH_EVERY_SAMPLES + 4_000);
        // Idempotent.
        assert_eq!(repair(&path).unwrap(), (PATCH_EVERY_SAMPLES + 4_000) * 2);
    }

    #[test]
    fn repair_of_a_file_cut_inside_the_header_and_of_foreign_files() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("audio.wav");
        std::fs::write(&path, &header(0)[..20]).unwrap();
        assert_eq!(repair(&path).unwrap(), 0);
        assert_eq!(std::fs::read(&path).unwrap(), header(0));
        assert!(decode(&path).is_empty());

        // Not ours (a stereo 44.1 kHz WAV the user dropped in, with junk
        // after it): refused, untouched.
        let other = dir.path().join("audio-x.wav");
        crate::audio::decode::write_wav_i16(&other, 44_100, 2, &tone(100));
        let mut bogus = std::fs::read(&other).unwrap();
        bogus.extend_from_slice(&[1, 2, 3]);
        std::fs::write(&other, &bogus).unwrap();
        assert!(repair(&other).is_err());
        assert_eq!(std::fs::read(&other).unwrap(), bogus);
        let text = dir.path().join("audio-y.wav");
        std::fs::write(&text, b"not a wav at all, just text that is long enough").unwrap();
        assert!(repair(&text).is_err());
        assert_eq!(
            std::fs::read(&text).unwrap(),
            b"not a wav at all, just text that is long enough"
        );
    }

    /// The > 4 GiB guard: a channel stops at the cap, the file stays valid.
    #[test]
    fn size_cap_stops_writing_and_keeps_a_valid_file() {
        assert_eq!(MAX_DATA_BYTES, 4_294_967_258);
        // ~37 h 17 min of 16 kHz 16-bit mono per channel.
        assert_eq!(MAX_DATA_BYTES / 32_000 / 60, 37 * 60 + 16);

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("audio.wav");
        let mut w = WavWriter::create_capped(&path, 1_001).unwrap();
        assert_eq!(w.write(&tone(300)).unwrap(), 300);
        assert_eq!(
            w.write(&tone(300)).unwrap(),
            200,
            "500 samples = 1 000 bytes"
        );
        assert!(w.is_full());
        assert_eq!(w.write(&tone(10)).unwrap(), 0);
        assert_eq!(w.write_silence(10).unwrap(), 0);
        assert_eq!(w.finish().unwrap(), 1_000);
        assert_eq!(decode(&path).len(), 500);
    }

    #[test]
    fn settle_and_list() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        assert!(settle_names(d).is_empty());
        WavWriter::create(&d.join("audio-mic.wav"))
            .unwrap()
            .finish()
            .unwrap();
        std::fs::write(d.join("transcript.md"), "x").unwrap();
        assert_eq!(settle_names(d), vec!["audio.wav"]);
        assert!(!d.join("audio-mic.wav").exists());
        // Two channels keep their names.
        WavWriter::create(&d.join("audio-remote.wav"))
            .unwrap()
            .finish()
            .unwrap();
        std::fs::rename(d.join("audio.wav"), d.join("audio-mic.wav")).unwrap();
        assert_eq!(settle_names(d), vec!["audio-mic.wav", "audio-remote.wav"]);
        let sized = files_with_sizes(d);
        assert_eq!(sized.len(), 2);
        assert!(sized.iter().all(|f| f.bytes == HEADER_LEN));
        assert_eq!(folder_bytes(d), 2 * HEADER_LEN + 1);
    }

    #[test]
    fn a_lone_opus_channel_becomes_audio_opus_and_both_formats_are_listed() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        AudioWriter::create(&d.join("audio-mic.opus"), AudioFormat::Opus, MAX_SAMPLES)
            .unwrap()
            .finish()
            .unwrap();
        assert_eq!(settle_names(d), vec!["audio.opus"]);
        // An item folder holding a WAV the user kept next to it: both are
        // its audio (sized, trashed by Delete audio), nothing is renamed.
        std::fs::write(d.join("audio-x.wav"), header(0)).unwrap();
        assert_eq!(settle_names(d), vec!["audio-x.wav", "audio.opus"]);
        assert_eq!(files_with_sizes(d).len(), 2);
    }

    #[test]
    fn writer_of_either_format_keeps_the_clock_and_the_duration_cap() {
        assert_eq!(MAX_SAMPLES / RATE as u64 / 60, 37 * 60 + 16);
        let dir = tempfile::tempdir().unwrap();
        for format in [AudioFormat::Wav, AudioFormat::Opus] {
            let path = dir.path().join(channel_file_name(Channel::Mic, format));
            let mut w = AudioWriter::create(&path, format, 1_000).unwrap();
            assert_eq!(w.path(), path);
            assert_eq!(w.write_silence(400).unwrap(), 400);
            assert_eq!(w.write(&tone(700)).unwrap(), 600, "{format:?}");
            assert_eq!(w.samples(), 1_000);
            assert!(w.is_full());
            w.finish().unwrap();
            // A finished file needs no repair and reports what it holds.
            assert_eq!(repair_any(&path).unwrap(), 1_000, "{format:?}");
        }
        assert!(repair_any(&dir.path().join("transcript.md")).is_err());
    }

    /// Both channels of a crashed Opus meeting are closed at startup and
    /// listed (two channels keep their names).
    #[test]
    fn recover_files_closes_every_opus_channel() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        for ch in [Channel::Mic, Channel::Remote] {
            let mut w = AudioWriter::create(
                &d.join(channel_file_name(ch, AudioFormat::Opus)),
                AudioFormat::Opus,
                MAX_SAMPLES,
            )
            .unwrap();
            w.write(&tone(3 * RATE as usize + 400)).unwrap();
            std::mem::forget(w);
        }
        assert_eq!(
            recover_files(d),
            vec!["audio-mic.opus", "audio-remote.opus"]
        );
        for name in ["audio-mic.opus", "audio-remote.opus"] {
            let (pcm, complete) = super::super::opus::decode_file(&d.join(name)).unwrap();
            assert!(complete, "{name}");
            // Three whole pages (3 s) minus the encoder's lookahead.
            assert_eq!(pcm.len(), (3 * 48_000 - 312) / 3, "{name}");
        }
    }

    #[test]
    fn format_setting_serializes_in_lowercase() {
        // P16 (#248): Opus for new installs.
        assert_eq!(AudioFormat::default(), AudioFormat::Opus);
        assert_eq!(serde_json::to_value(AudioFormat::Opus).unwrap(), "opus");
        let f: AudioFormat = serde_json::from_str("\"wav\"").unwrap();
        assert_eq!(f, AudioFormat::Wav);
        assert!(serde_json::from_str::<AudioFormat>("\"mp3\"").is_err());
    }

    #[test]
    fn frontmatter_list_accepts_only_audio_names() {
        let mut m = ItemMeta::default();
        assert!(listed(&m).is_empty());
        set_listed(&mut m, &["audio-mic.wav".into(), "audio-remote.wav".into()]);
        assert_eq!(listed(&m), vec!["audio-mic.wav", "audio-remote.wav"]);
        set_listed(&mut m, &["audio.opus".into()]);
        assert_eq!(listed(&m), vec!["audio.opus"]);
        set_listed(&mut m, &["audio-mic.wav".into(), "audio-remote.wav".into()]);
        // Round-trips through the frontmatter.
        let back: ItemMeta = serde_json::from_value(serde_json::to_value(&m).unwrap()).unwrap();
        assert_eq!(listed(&back), listed(&m));
        // Hand-edited values: a scalar works, paths never do.
        m.extra.insert(
            AUDIO_KEY.into(),
            serde_json::json!(["../../etc/passwd", "audio.wav"]),
        );
        assert_eq!(listed(&m), vec!["audio.wav"]);
        m.extra.insert(AUDIO_KEY.into(), "audio.wav".into());
        assert_eq!(listed(&m), vec!["audio.wav"]);
        set_listed(&mut m, &[]);
        assert!(!m.extra.contains_key(AUDIO_KEY));
    }
}
