//! Microphone source for long sessions. It owns its own [`Recorder`] —
//! separate from the hotkey's, so a dictation during a session neither stops
//! nor steals it — and drains the recorder every poll, so the capture never
//! accumulates: the recorder holds at most one poll interval of audio
//! (~16 KB at 250 ms) and the engine consumes it incrementally.

use super::{Channel, Clock, Frame, Source};
use crate::audio::recorder::Recorder;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

pub const POLL: Duration = Duration::from_millis(250);
/// A device that has not delivered audio by then has failed to open.
const START_TIMEOUT: Duration = Duration::from_secs(5);

pub struct MicSource {
    recorder: Recorder,
    stop: Arc<AtomicBool>,
    clock: Clock,
    finished: bool,
    started: Instant,
    live: bool,
}

impl MicSource {
    /// Start capturing from `device` (empty = system default). The session
    /// ends when `stop` is set.
    pub fn start(device: &str, stop: Arc<AtomicBool>) -> anyhow::Result<Self> {
        let mut recorder = Recorder::default();
        recorder.start(device)?;
        Ok(Self {
            recorder,
            stop,
            clock: Clock::default(),
            finished: false,
            started: Instant::now(),
            live: false,
        })
    }
}

impl Source for MicSource {
    fn channel(&self) -> Channel {
        Channel::Mic
    }

    fn total_samples(&self) -> Option<u64> {
        None
    }

    fn next_frame(&mut self) -> anyhow::Result<Option<Frame>> {
        loop {
            if self.finished {
                return Ok(None);
            }
            if self.stop.load(Ordering::Relaxed) {
                self.finished = true;
                // What is left since the last drain, plus the resampler tail.
                let rest = self.recorder.stop()?;
                if rest.is_empty() {
                    return Ok(None);
                }
                return Ok(Some(self.clock.stamp(Channel::Mic, rest)));
            }
            std::thread::sleep(POLL);
            let chunk = self.recorder.take_new_16k();
            if !chunk.is_empty() {
                self.live = true;
                return Ok(Some(self.clock.stamp(Channel::Mic, chunk)));
            }
            // The capture thread reports a failed device only when stopped:
            // don't let a session record nothing for an hour.
            if !self.live && self.started.elapsed() > START_TIMEOUT {
                self.finished = true;
                self.recorder.stop()?;
                anyhow::bail!("the microphone delivered no audio — check the input device");
            }
        }
    }
}
