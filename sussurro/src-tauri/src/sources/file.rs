//! File source: decoded from its path and streamed through symphonia
//! ([`FileStream`]), one packet at a time — never the whole file in RAM and
//! never the file's bytes over IPC.

use super::{Channel, Clock, Frame, Source};
use crate::audio::decode::FileStream;
use std::path::Path;

pub struct FileSource {
    stream: FileStream,
    clock: Clock,
}

impl FileSource {
    pub fn open(path: &Path) -> anyhow::Result<Self> {
        Ok(Self {
            stream: FileStream::open(path)?,
            clock: Clock::default(),
        })
    }
}

impl Source for FileSource {
    fn channel(&self) -> Channel {
        Channel::File
    }

    fn total_samples(&self) -> Option<u64> {
        self.stream.duration_ms.map(|ms| ms * 16)
    }

    fn next_frame(&mut self) -> anyhow::Result<Option<Frame>> {
        loop {
            match self.stream.next_chunk()? {
                None => return Ok(None),
                Some(chunk) if chunk.is_empty() => continue,
                Some(chunk) => return Ok(Some(self.clock.stamp(Channel::File, chunk))),
            }
        }
    }
}

/// `file:<name>` for the archive frontmatter.
pub fn source_label(path: &Path) -> String {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    format!("file:{name}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_source_streams_frames_on_one_clock() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.wav");
        let tone: Vec<f32> = (0..48_000).map(|i| (i as f32 * 0.05).sin() * 0.2).collect();
        crate::audio::decode::write_wav_i16(&path, 48_000, 1, &tone);
        let mut src = FileSource::open(&path).unwrap();
        assert_eq!(src.channel(), Channel::File);
        assert_eq!(src.total_samples(), Some(16_000));
        let mut expected_start = 0;
        let mut total = 0;
        while let Some(f) = src.next_frame().unwrap() {
            assert_eq!(f.start, expected_start);
            expected_start += f.samples.len() as u64;
            total += f.samples.len();
        }
        assert_eq!(total, 16_000);
    }

    #[test]
    fn source_label_uses_the_file_name() {
        assert_eq!(
            source_label(Path::new("/x/y/Weekly sync.m4a")),
            "file:Weekly sync.m4a"
        );
    }
}
