//! Browser source (0.9, #126, plan §4.1): the extension streams a meeting's
//! audio over the `/live` WebSocket as two logical channels — `mic` (the
//! page's own microphone track, the user) and `remote` (everyone else) — at
//! the browser's sample rate.
//!
//! - [`ChannelMux`] (pure): per channel, resamples to 16 kHz mono with the
//!   equivalence-tested [`StreamResampler`], places the audio on the run's
//!   clock, fills lost frames (`seq` gaps) with silence and drops repeated
//!   ones. The silence is bounded twice (#217): at most [`MAX_GAP_SAMPLES`]
//!   per gap, and per channel at most the wall-clock time since the session
//!   started plus [`SILENCE_GRACE_SAMPLES`] in total, so a page that jumps
//!   `seq` cannot turn a few bytes into minutes of (saved) audio. A
//!   channel that starts late (the remote track arrives when the call
//!   connects) begins at the session's current position, so both channels
//!   share one timeline.
//! - [`BrowserSource`]: the engine's [`Source`], fed by the WebSocket thread
//!   through a bounded channel; it ends when the sender is dropped (`stop`,
//!   or the extension went away) or the run is cancelled.

use super::{Channel, Frame, Source};
use crate::audio::resample::StreamResampler;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, SyncSender};
use std::sync::Arc;
use std::time::Duration;

/// The engine's rate.
const RATE: u64 = 16_000;
/// Longest silence inserted for one `seq` gap: a client bug (or a jump of
/// billions) must not stall the run on hours of zeros.
pub const MAX_GAP_SAMPLES: u64 = 60 * RATE;
/// Silence a channel may have inserted beyond the session's wall-clock
/// time: the extension sends up to ~30 s of audio it buffered while
/// (re)connecting in one burst right after `start`, gaps included.
pub const SILENCE_GRACE_SAMPLES: u64 = 30 * RATE;
/// Silence goes to the engine in frames of at most this many samples.
const SILENCE_FRAME: u64 = RATE;
/// Chunks buffered between the WebSocket thread and the engine before the
/// socket reader waits (~ several seconds of audio at typical frame sizes).
pub const CHANNEL_BOUND: usize = 512;
/// How often a waiting source checks for a cancel.
const POLL: Duration = Duration::from_millis(250);

/// What the mux hands to the source.
#[derive(Debug, Clone, PartialEq)]
pub enum Chunk {
    Audio(Frame),
    /// `len` samples of silence on `channel` starting at `start`.
    Silence { channel: Channel, start: u64, len: u64 },
}

impl Chunk {
    pub fn channel(&self) -> Channel {
        match self {
            Chunk::Audio(f) => f.channel,
            Chunk::Silence { channel, .. } => *channel,
        }
    }
}

/// What one pushed frame did.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Pushed {
    pub chunks: Vec<Chunk>,
    /// Frames the client never sent (a `seq` gap), filled with silence.
    pub lost: u64,
    /// The frame repeated an older `seq` and was dropped.
    pub duplicate: bool,
}

struct Lane {
    channel: Channel,
    resampler: StreamResampler,
    /// `seq` expected next.
    next_seq: u32,
    /// Position of the next output sample on the run's clock.
    pos: u64,
    /// Input samples in the last frame (the size of a lost frame).
    last_len: u64,
    /// Silence inserted for `seq` gaps so far, in output samples.
    silence: u64,
}

/// Per-channel resampling and alignment. Pure.
pub struct ChannelMux {
    rate: u32,
    lanes: Vec<Lane>,
}

impl ChannelMux {
    pub fn new(rate: u32) -> Self {
        Self {
            rate: rate.max(1),
            lanes: Vec::new(),
        }
    }

    /// The session's position: the furthest any channel has got, in 16 kHz
    /// samples.
    pub fn position(&self) -> u64 {
        self.lanes.iter().map(|l| l.pos).max().unwrap_or(0)
    }

    /// [`Self::position`] in milliseconds.
    pub fn position_ms(&self) -> u64 {
        self.position() * 1000 / RATE
    }

    fn to_output(&self, input: u64) -> u64 {
        let out = input as u128 * RATE as u128 / self.rate as u128;
        out.min(u64::MAX as u128) as u64
    }

    /// Push one mono frame (`pcm` at the `start` rate) of `channel`;
    /// `elapsed` is the wall-clock time since the session started (it bounds
    /// the silence gaps may insert).
    pub fn push(&mut self, channel: Channel, seq: u32, pcm: &[f32], elapsed: Duration) -> Pushed {
        let mut out = Pushed::default();
        let i = match self.lanes.iter().position(|l| l.channel == channel) {
            Some(i) => i,
            None => {
                // First frame of this channel: it joins the session now.
                let pos = self.position();
                self.lanes.push(Lane {
                    channel,
                    resampler: StreamResampler::new(1, self.rate, RATE as u32),
                    next_seq: seq,
                    pos,
                    last_len: pcm.len() as u64,
                    silence: 0,
                });
                self.lanes.len() - 1
            }
        };
        let expected = self.lanes[i].next_seq;
        if seq < expected {
            out.duplicate = true;
            return out;
        }
        if seq > expected {
            let lost = (seq - expected) as u64;
            let budget = silence_budget(elapsed).saturating_sub(self.lanes[i].silence);
            let len = self
                .to_output(lost.saturating_mul(self.lanes[i].last_len))
                .min(MAX_GAP_SAMPLES)
                .min(budget);
            out.lost = lost;
            if len > 0 {
                let lane = &mut self.lanes[i];
                out.chunks.push(Chunk::Silence {
                    channel,
                    start: lane.pos,
                    len,
                });
                lane.pos += len;
                lane.silence += len;
            }
        }
        let lane = &mut self.lanes[i];
        lane.next_seq = seq.saturating_add(1);
        if !pcm.is_empty() {
            lane.last_len = pcm.len() as u64;
        }
        let samples = lane.resampler.push(pcm);
        if !samples.is_empty() {
            let start = lane.pos;
            lane.pos += samples.len() as u64;
            out.chunks.push(Chunk::Audio(Frame {
                channel,
                start,
                samples,
            }));
        }
        out
    }

    /// End of the session: the resamplers' held-back tails.
    pub fn finish(&mut self) -> Vec<Chunk> {
        let mut out = Vec::new();
        for lane in &mut self.lanes {
            let samples = lane.resampler.flush();
            if !samples.is_empty() {
                let start = lane.pos;
                lane.pos += samples.len() as u64;
                out.push(Chunk::Audio(Frame {
                    channel: lane.channel,
                    start,
                    samples,
                }));
            }
        }
        out
    }
}

/// Total silence one channel may insert `elapsed` into the session.
fn silence_budget(elapsed: Duration) -> u64 {
    let wall = elapsed.as_millis().saturating_mul(RATE as u128) / 1000;
    (wall.min(u64::MAX as u128) as u64).saturating_add(SILENCE_GRACE_SAMPLES)
}

/// The engine side of a browser session.
pub struct BrowserSource {
    rx: Receiver<Chunk>,
    /// Silence still to hand out: channel, next start, samples left.
    silence: Option<(Channel, u64, u64)>,
    cancel: Option<Arc<AtomicBool>>,
}

impl BrowserSource {
    /// A source and the sender the WebSocket thread feeds it through.
    pub fn channel() -> (SyncSender<Chunk>, Self) {
        let (tx, rx) = std::sync::mpsc::sync_channel(CHANNEL_BOUND);
        (
            tx,
            Self {
                rx,
                silence: None,
                cancel: None,
            },
        )
    }

    /// Stop waiting for audio once `cancel` is set (the run's cancel flag).
    pub fn with_cancel(mut self, cancel: Arc<AtomicBool>) -> Self {
        self.cancel = Some(cancel);
        self
    }

    fn take_silence(&mut self) -> Option<Frame> {
        let (channel, start, left) = self.silence?;
        let n = left.min(SILENCE_FRAME);
        self.silence = (left > n).then_some((channel, start + n, left - n));
        Some(Frame {
            channel,
            start,
            samples: vec![0.0; n as usize],
        })
    }
}

impl Source for BrowserSource {
    /// The channel the extension always has; frames carry their own.
    fn channel(&self) -> Channel {
        Channel::Remote
    }

    fn total_samples(&self) -> Option<u64> {
        None
    }

    fn next_frame(&mut self) -> anyhow::Result<Option<Frame>> {
        if let Some(f) = self.take_silence() {
            return Ok(Some(f));
        }
        loop {
            match self.rx.recv_timeout(POLL) {
                Ok(Chunk::Audio(f)) if f.samples.is_empty() => continue,
                Ok(Chunk::Audio(f)) => return Ok(Some(f)),
                Ok(Chunk::Silence { channel, start, len }) => {
                    self.silence = (len > 0).then_some((channel, start, len));
                    if let Some(f) = self.take_silence() {
                        return Ok(Some(f));
                    }
                }
                Err(RecvTimeoutError::Timeout) => {
                    if self.cancel.as_ref().is_some_and(|c| c.load(Ordering::Relaxed)) {
                        return Ok(None);
                    }
                }
                Err(RecvTimeoutError::Disconnected) => return Ok(None),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const T0: Duration = Duration::ZERO;
    const HOUR: Duration = Duration::from_secs(3600);

    fn audio_len(p: &Pushed) -> usize {
        p.chunks
            .iter()
            .map(|c| match c {
                Chunk::Audio(f) => f.samples.len(),
                Chunk::Silence { len, .. } => *len as usize,
            })
            .sum()
    }

    #[test]
    fn resamples_each_channel_to_16k_on_one_clock() {
        let mut mux = ChannelMux::new(48_000);
        let frame = vec![0.1f32; 480]; // 10 ms at 48 kHz
        let mut mic = 0;
        let mut remote = 0;
        // The remote channel's first frame arrives after the mic's: it
        // joins at the mic's position then.
        let mut remote_start = None;
        for seq in 0..100 {
            for (ch, total) in [(Channel::Mic, &mut mic), (Channel::Remote, &mut remote)] {
                let p = mux.push(ch, seq, &frame, T0);
                for c in &p.chunks {
                    let Chunk::Audio(f) = c else { panic!("no gap here") };
                    assert_eq!(f.channel, ch);
                    let base = if ch == Channel::Remote {
                        *remote_start.get_or_insert(f.start as usize)
                    } else {
                        0
                    };
                    assert_eq!(f.start as usize, base + *total, "contiguous per channel");
                    *total += f.samples.len();
                }
            }
        }
        assert_eq!(remote_start, Some(160), "one mic frame ahead");
        for c in mux.finish() {
            let Chunk::Audio(f) = c else { panic!() };
            match f.channel {
                Channel::Mic => mic += f.samples.len(),
                _ => remote += f.samples.len(),
            }
        }
        // 1 s at 48 kHz → 16 000 samples per channel, like the batch path.
        assert_eq!((mic, remote), (16_000, 16_000));
        assert_eq!(mux.position(), 16_160);
    }

    #[test]
    fn matches_the_batch_resampler() {
        let input: Vec<f32> = (0..44_100).map(|i| ((i as f32) * 0.01).sin()).collect();
        let batch = crate::audio::resample::resample_linear(&input, 44_100, 16_000);
        let mut mux = ChannelMux::new(44_100);
        let mut streamed = Vec::new();
        for (seq, chunk) in input.chunks(441).enumerate() {
            for c in mux.push(Channel::Remote, seq as u32, chunk, T0).chunks {
                let Chunk::Audio(f) = c else { panic!() };
                streamed.extend(f.samples);
            }
        }
        for c in mux.finish() {
            let Chunk::Audio(f) = c else { panic!() };
            streamed.extend(f.samples);
        }
        assert_eq!(streamed, batch);
    }

    #[test]
    fn seq_gaps_become_silence_and_repeats_are_dropped() {
        let mut mux = ChannelMux::new(16_000);
        let frame = vec![0.2f32; 320]; // 20 ms
        assert_eq!(audio_len(&mux.push(Channel::Mic, 0, &frame, HOUR)), 320);
        // Frames 1 and 2 were lost: 640 samples of silence, then the frame.
        let p = mux.push(Channel::Mic, 3, &frame, HOUR);
        assert_eq!(p.lost, 2);
        assert_eq!(
            p.chunks[0],
            Chunk::Silence {
                channel: Channel::Mic,
                start: 320,
                len: 640
            }
        );
        let Chunk::Audio(f) = &p.chunks[1] else { panic!() };
        assert_eq!(f.start, 960);
        // A repeat (or an older frame) is dropped without moving the clock.
        let p = mux.push(Channel::Mic, 3, &frame, HOUR);
        assert!(p.duplicate && p.chunks.is_empty());
        assert!(mux.push(Channel::Mic, 1, &frame, HOUR).duplicate);
        assert_eq!(mux.position(), 1_280);
        // A huge jump is capped.
        let p = mux.push(Channel::Mic, u32::MAX, &frame, HOUR);
        assert_eq!(p.lost, u32::MAX as u64 - 4);
        assert!(matches!(p.chunks[0], Chunk::Silence { len, .. } if len == MAX_GAP_SAMPLES));
    }

    #[test]
    fn inserted_silence_is_bounded_by_the_wall_clock() {
        let mut mux = ChannelMux::new(16_000);
        let frame = vec![0.2f32; 320]; // 20 ms
        let silence = |p: &Pushed| -> u64 {
            p.chunks
                .iter()
                .map(|c| match c {
                    Chunk::Silence { len, .. } => *len,
                    _ => 0,
                })
                .sum()
        };
        mux.push(Channel::Remote, 0, &frame, T0);
        // At the start only the grace is available: one 60 s gap is cut to
        // 30 s, and nothing more fits.
        let p = mux.push(Channel::Remote, 10_000, &frame, T0);
        assert_eq!(silence(&p), SILENCE_GRACE_SAMPLES);
        let p = mux.push(Channel::Remote, 20_000, &frame, T0);
        assert_eq!((p.lost, silence(&p)), (9_999, 0), "counted, not filled");
        // Ten seconds later, ten more seconds of silence are allowed…
        let p = mux.push(Channel::Remote, 30_000, &frame, Duration::from_secs(10));
        assert_eq!(silence(&p), 10 * 16_000);
        // …and a stream of 5-byte jumps adds nothing past the budget.
        let mut total = 0;
        for k in 1..1_000u32 {
            total += silence(&mux.push(Channel::Remote, 30_000 + k * 1_000_000, &[], Duration::from_secs(10)));
        }
        assert_eq!(total, 0);
        // The budget is per channel, and the per-gap cap still applies.
        mux.push(Channel::Mic, 0, &frame, HOUR);
        let p = mux.push(Channel::Mic, 1_000_000, &frame, HOUR);
        assert_eq!(silence(&p), MAX_GAP_SAMPLES);
        // Real audio is never cut.
        let before = mux.position();
        let p = mux.push(Channel::Remote, 1_000_000_000, &frame, Duration::from_secs(10));
        assert_eq!(silence(&p), 0);
        assert_eq!(audio_len(&p), 320);
        assert!(mux.position() >= before);
    }

    #[test]
    fn a_late_channel_joins_at_the_session_position() {
        let mut mux = ChannelMux::new(16_000);
        for seq in 0..50 {
            mux.push(Channel::Mic, seq, &[0.0; 320], T0); // 1 s of mic
        }
        let p = mux.push(Channel::Remote, 7, &[0.5; 320], T0);
        assert_eq!(p.lost, 0, "the first seq of a channel is its start");
        let Chunk::Audio(f) = &p.chunks[0] else { panic!() };
        assert_eq!((f.channel, f.start), (Channel::Remote, 16_000));
        assert_eq!(mux.push(Channel::Remote, 8, &[0.5; 320], T0).lost, 0);
    }

    #[test]
    fn source_relays_chunks_expands_silence_and_ends_on_disconnect() {
        let (tx, mut src) = BrowserSource::channel();
        tx.send(Chunk::Audio(Frame {
            channel: Channel::Mic,
            start: 0,
            samples: vec![0.1; 10],
        }))
        .unwrap();
        tx.send(Chunk::Silence {
            channel: Channel::Remote,
            start: 5,
            len: 20_000,
        })
        .unwrap();
        drop(tx);
        let a = src.next_frame().unwrap().unwrap();
        assert_eq!((a.channel, a.start, a.samples.len()), (Channel::Mic, 0, 10));
        let s1 = src.next_frame().unwrap().unwrap();
        assert_eq!((s1.channel, s1.start, s1.samples.len()), (Channel::Remote, 5, 16_000));
        let s2 = src.next_frame().unwrap().unwrap();
        assert_eq!((s2.start, s2.samples.len()), (16_005, 4_000));
        assert!(s2.samples.iter().all(|&x| x == 0.0));
        assert_eq!(src.next_frame().unwrap(), None);
    }

    #[test]
    fn source_stops_waiting_when_cancelled() {
        let (_tx, src) = BrowserSource::channel();
        let cancel = Arc::new(AtomicBool::new(true));
        let mut src = src.with_cancel(cancel);
        assert_eq!(src.next_frame().unwrap(), None);
    }
}
