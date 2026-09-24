//! One ingest interface for every source except hotkey dictation (E1, plan
//! §4.1): a source yields frames of 16 kHz mono audio tagged with a logical
//! channel and positioned on a monotonic sample clock. The long-form engine
//! pulls frames, segments them and transcribes the segments.
//!
//! 0.7 sources: [`mic::MicSource`] (long sessions, separate from the hotkey
//! recorder) and [`file::FileSource`] (decoded from a path, streamed).
//! 0.8: [`url`] fetches a link to a temporary file for the file source.
//! 0.9: [`browser::BrowserSource`] — a meeting from the browser extension,
//! two channels (`mic`, `remote`) on one clock.
//! 0.10: [`system::SystemSource`] — the mic plus any second input device
//! (a virtual loopback device) as two channels (`mic`, `system`), #139.

pub mod browser;
pub mod file;
pub mod mic;
pub mod system;
pub mod url;

pub use crate::archive::Channel;

/// A chunk of 16 kHz mono audio from one logical channel. `start` is the
/// position of its first sample on the channel's clock (samples since the
/// source started).
#[derive(Debug, Clone, PartialEq)]
pub struct Frame {
    pub channel: Channel,
    pub start: u64,
    pub samples: Vec<f32>,
}

/// A pull-based audio source. The engine's ingest thread calls
/// [`Source::next_frame`] in a loop; a file source is naturally throttled by
/// the engine's backpressure, a live source blocks until audio arrives.
///
/// A source may interleave frames of several channels (a meeting, #126):
/// each channel's frames are contiguous on the source's clock, and a
/// channel's first frame may start after 0 (it joined late).
pub trait Source: Send {
    /// The source's main channel (the only one for mic and file sources).
    fn channel(&self) -> Channel;
    /// Total length in samples, when known up front (files).
    fn total_samples(&self) -> Option<u64>;
    /// The next frame, `Ok(None)` at the end of the source.
    fn next_frame(&mut self) -> anyhow::Result<Option<Frame>>;
    /// Problems the user should hear about while the run goes on (a device
    /// lost, device clocks realigned), taken since the last call. The
    /// engine emits each as `engine-warning`. Most sources have none.
    fn take_warnings(&mut self) -> Vec<String> {
        Vec::new()
    }
}

/// Tracks a source's clock: stamps consecutive chunks with their start.
#[derive(Debug, Default)]
pub struct Clock {
    next: u64,
}

impl Clock {
    pub fn stamp(&mut self, channel: Channel, samples: Vec<f32>) -> Frame {
        let start = self.next;
        self.next += samples.len() as u64;
        Frame {
            channel,
            start,
            samples,
        }
    }

    pub fn position(&self) -> u64 {
        self.next
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clock_stamps_consecutive_frames() {
        let mut c = Clock::default();
        let a = c.stamp(Channel::File, vec![0.0; 100]);
        let b = c.stamp(Channel::File, vec![0.0; 50]);
        assert_eq!((a.start, b.start, c.position()), (0, 100, 150));
        assert_eq!(b.channel, Channel::File);
    }
}
