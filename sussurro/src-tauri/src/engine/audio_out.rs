//! Saved audio on the engine's output side (#141, P9: only on request).
//!
//! When a run saves audio, the ingest thread hands every source frame to an
//! [`AudioOut`] as it arrives, before segmentation: one incremental file per
//! logical channel, WAV or Ogg Opus as the run's [`AudioFormat`] says
//! ([`crate::archive::audio`] has the layout and the crash repair,
//! [`crate::archive::opus`] the Opus stream). Nothing is buffered here
//! beyond each writer's 64 KB (plus one Opus page), so memory stays flat
//! however long the session; a WAV header is patched every 10 s of audio
//! and an Opus page is flushed every second, and both are closed when the
//! run ends.
//!
//! Frames of a channel are contiguous on the run's clock, and a channel
//! may start late; a gap (or a late start) is filled with silence and an
//! overlap is skipped, so every file starts at the run's t = 0 and a
//! segment's `start_ms` is its position in the file.
//!
//! A failure to write audio (disk full, the folder gone) is logged and that
//! channel stops being saved — the transcription is what the user is
//! waiting for, and it goes on. When a run does not save audio, no
//! [`AudioOut`] exists at all.

use crate::archive::audio::{self, AudioFormat, AudioWriter};
use crate::archive::Channel;
use crate::sources::Frame;
use std::path::{Path, PathBuf};

struct ChannelOut {
    channel: Channel,
    writer: AudioWriter,
    /// Writing failed once: this channel is not saved any further.
    failed: bool,
    /// The size cap was reported.
    full_logged: bool,
}

/// The run's audio files, one per channel, in the item folder.
pub struct AudioOut {
    dir: PathBuf,
    format: AudioFormat,
    /// Per-file duration cap, in samples.
    cap: u64,
    channels: Vec<ChannelOut>,
    /// Channels whose file could not be created: not retried per frame.
    refused: Vec<Channel>,
}

impl AudioOut {
    /// Audio saved into the item folder `dir`, in `format`.
    pub fn new(dir: &Path, format: AudioFormat) -> Self {
        Self::with_cap(dir, format, audio::MAX_SAMPLES)
    }

    /// [`Self::new`] with a smaller per-file duration cap, in samples
    /// (tests).
    pub fn with_cap(dir: &Path, format: AudioFormat, cap: u64) -> Self {
        Self {
            dir: dir.to_path_buf(),
            format,
            cap,
            channels: Vec::new(),
            refused: Vec::new(),
        }
    }

    /// Channels with a file so far.
    pub fn channels(&self) -> Vec<Channel> {
        self.channels.iter().map(|c| c.channel).collect()
    }

    /// Save one frame. Never fails: problems are logged (see the module docs).
    pub fn push(&mut self, frame: &Frame) {
        if self.refused.contains(&frame.channel) {
            return;
        }
        let i = match self
            .channels
            .iter()
            .position(|c| c.channel == frame.channel)
        {
            Some(i) => i,
            None => {
                let path = self
                    .dir
                    .join(audio::channel_file_name(frame.channel, self.format));
                match AudioWriter::create(&path, self.format, self.cap) {
                    Ok(writer) => {
                        self.channels.push(ChannelOut {
                            channel: frame.channel,
                            writer,
                            failed: false,
                            full_logged: false,
                        });
                        self.channels.len() - 1
                    }
                    Err(e) => {
                        eprintln!("engine: this channel's audio is not saved ({e:#})");
                        self.refused.push(frame.channel);
                        return;
                    }
                }
            }
        };
        let out = &mut self.channels[i];
        if out.failed {
            return;
        }
        let at = out.writer.samples();
        let written = if frame.start > at {
            out.writer
                .write_silence(frame.start - at)
                .and_then(|_| out.writer.write(&frame.samples))
        } else {
            let skip = ((at - frame.start) as usize).min(frame.samples.len());
            out.writer.write(&frame.samples[skip..])
        };
        if let Err(e) = written {
            eprintln!(
                "engine: saving {} stopped ({e:#})",
                out.writer.path().display()
            );
            out.failed = true;
        } else if out.writer.is_full() && !out.full_logged {
            out.full_logged = true;
            eprintln!(
                "engine: {} reached the saved audio size limit; the rest of this channel is transcribed but not saved",
                out.writer.path().display()
            );
        }
    }

    /// Close every file (last WAV header, end of the Opus stream) and flush
    /// to disk, then give a single-channel run its `audio.<ext>` name.
    /// Returns the files now in the item folder, for the frontmatter.
    pub fn finish(self) -> Vec<String> {
        for out in self.channels {
            let path = out.writer.path().to_path_buf();
            if let Err(e) = out.writer.finish() {
                eprintln!(
                    "engine: finishing {} failed ({e:#}); repairing",
                    path.display()
                );
                if let Err(e) = audio::repair_any(&path) {
                    eprintln!("engine: {} left as is ({e:#})", path.display());
                }
            }
        }
        audio::settle_names(&self.dir)
    }
}
