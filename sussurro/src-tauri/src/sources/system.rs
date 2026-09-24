//! System audio + mic (0.10, #139, plan §4.1 step 1): the microphone and
//! **any second input device** recorded as two channels of one session —
//! `mic` (the user, "You") and `system` (everyone else, clustered into
//! "Voice N"). The second device is whatever carries the computer's output:
//! BlackHole or Loopback on macOS, VB-Cable or Voicemeeter on Windows, a
//! PulseAudio/PipeWire monitor source on Linux. Native loopback without a
//! virtual device is step 2 (#140).
//!
//! Each device has its own [`Recorder`] — its own cpal stream, thread and
//! [`crate::audio::resample::StreamResampler`] from the device's rate and
//! channel count to 16 kHz mono — so two devices at different rates (48 kHz
//! and 44.1 kHz) never share conversion state.
//!
//! **Two clocks.** The devices run on their own crystals (or, for a virtual
//! device, on whatever drives it), so "48 000 samples" is not exactly one
//! second on both: ±50 ppm each is ±0.36 s per hour between the two.
//! [`ChannelSync`] places the audio on the session's timeline:
//! - each channel is timestamped **by its own samples** from the session
//!   start: its first chunk lands where the wall clock says it was captured
//!   (`now − len`, so a device that opens late joins late), and from then on
//!   every chunk follows the previous one — the audio is never cut or
//!   stretched;
//! - every poll it measures each channel's lag behind the wall clock
//!   (`now − position`). Delivery is bursty (callback blocks, the 250 ms
//!   poll), so the lag is smoothed as its **minimum over the last 2 s**,
//!   which removes the jitter and keeps the slow trend;
//! - when the smoothed lags of the two channels differ by more than
//!   [`DRIFT_LIMIT_MS`] (200 ms), the channel that fell behind is
//!   **realigned with silence** of that length (never by dropping audio from
//!   the other) and the user gets an `engine-warning`. A device that stalls
//!   is realigned the same way until it counts as lost.
//!
//! **Device loss.** A device that reports it went away (cpal's
//! `DeviceNotAvailable`), or delivers nothing for [`STALL_MS`], ends *its*
//! channel with a warning; the other goes on. When both have ended the
//! source ends normally and the engine saves what was transcribed — plus the
//! checkpointing of #153, so a crash keeps it too. A device that never
//! delivers in the first [`START_MS`] ends its channel the same way; only
//! when neither ever delivered does the run fail.

use super::{Channel, Frame, Source};
use crate::audio::recorder::Recorder;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// The item's `source` in the frontmatter.
pub const SOURCE_LABEL: &str = "system";

/// The engine's rate.
const RATE: u64 = 16_000;
/// How often the devices are drained.
pub const POLL: Duration = Duration::from_millis(250);
/// Drift between the two channels tolerated before a realignment.
pub const DRIFT_LIMIT_MS: u64 = 200;
const DRIFT_LIMIT: i64 = (DRIFT_LIMIT_MS * RATE / 1000) as i64;
/// Polls in the lag window (2 s at [`POLL`]): the smoothing that tells a
/// drift (it stays) from delivery jitter (it doesn't).
pub const LAG_WINDOW: usize = 8;
/// A device silent this long (no samples at all — a loopback device
/// delivers zeros while nothing plays) is lost.
pub const STALL_MS: u64 = 5_000;
/// A device that has not delivered by then failed to open.
pub const START_MS: u64 = 5_000;

fn ms_to_samples(ms: u64) -> u64 {
    ms * RATE / 1000
}

fn clock(samples: u64) -> String {
    let s = samples / RATE;
    format!("{}:{:02}", s / 60, s % 60)
}

fn device_label(channel: Channel) -> &'static str {
    match channel {
        Channel::Mic => "microphone",
        Channel::System => "system audio device",
        Channel::Remote => "remote audio",
        Channel::File => "file",
    }
}

/// Something [`ChannelSync`] noticed that the user should know.
#[derive(Debug, Clone, PartialEq)]
pub enum Notice {
    /// `channel` fell behind the other by `ms` and got that much silence at
    /// `at` (16 kHz samples on the session's clock).
    Realigned { channel: Channel, ms: u64, at: u64 },
    /// `channel` stopped delivering audio at `at`: it ended.
    Lost { channel: Channel, at: u64 },
    /// `channel` never delivered audio: it is not recorded.
    NeverStarted { channel: Channel },
}

impl Notice {
    /// The `engine-warning` text.
    pub fn message(&self) -> String {
        match self {
            Notice::Realigned { channel, ms, at } => format!(
                "The microphone and the system audio device drifted {:.2} s apart — realigned at {} (silence added to the {} channel).",
                *ms as f64 / 1000.0,
                clock(*at),
                device_label(*channel),
            ),
            Notice::Lost { channel, at } => format!(
                "The {} stopped delivering audio at {} (disconnected?) — its channel ended; the rest of the recording goes on and is kept.",
                device_label(*channel),
                clock(*at),
            ),
            Notice::NeverStarted { channel } => format!(
                "The {} delivered no audio — that channel is not recorded. Check the device in New → System audio + mic.",
                device_label(*channel),
            ),
        }
    }
}

struct SyncLane {
    channel: Channel,
    /// Position of the next sample on the session's clock; `None` until
    /// the first audio.
    pos: Option<u64>,
    /// Wall time (16 kHz samples) of the last delivery.
    last_audio: u64,
    /// Recent `now − pos`, one per poll, at most [`LAG_WINDOW`].
    lags: VecDeque<i64>,
    ended: bool,
}

impl SyncLane {
    fn live(&self) -> bool {
        !self.ended && self.pos.is_some()
    }
}

/// Places the audio of several live devices on one session clock and keeps
/// them aligned (see the module docs). Pure: the wall clock comes in as
/// `now`, in 16 kHz samples since the session started.
pub struct ChannelSync {
    lanes: Vec<SyncLane>,
    resyncs: u32,
}

impl ChannelSync {
    pub fn new(channels: &[Channel]) -> Self {
        Self {
            lanes: channels
                .iter()
                .map(|&channel| SyncLane {
                    channel,
                    pos: None,
                    last_audio: 0,
                    lags: VecDeque::with_capacity(LAG_WINDOW + 1),
                    ended: false,
                })
                .collect(),
            resyncs: 0,
        }
    }

    fn lane_mut(&mut self, channel: Channel) -> Option<&mut SyncLane> {
        self.lanes.iter_mut().find(|l| l.channel == channel)
    }

    /// Audio `channel` delivered by wall time `now`: the frame to hand to
    /// the engine. `None` for nothing, or for a channel that has ended.
    pub fn push(&mut self, channel: Channel, samples: Vec<f32>, now: u64) -> Option<Frame> {
        let lane = self.lane_mut(channel)?;
        if lane.ended || samples.is_empty() {
            return None;
        }
        let len = samples.len() as u64;
        // The first chunk was captured just before now: a device that
        // opened late joins the session where it actually started.
        let start = *lane.pos.get_or_insert(now.saturating_sub(len));
        lane.pos = Some(start + len);
        lane.last_audio = now;
        Some(Frame {
            channel,
            start,
            samples,
        })
    }

    /// End `channel` (its device went away). False if it had already ended.
    pub fn end(&mut self, channel: Channel) -> bool {
        match self.lane_mut(channel) {
            Some(l) if !l.ended => {
                l.ended = true;
                true
            }
            _ => false,
        }
    }

    /// End of a poll at wall time `now`: channels that never started or
    /// stalled end, and a channel that fell more than [`DRIFT_LIMIT_MS`]
    /// behind another gets the silence that realigns it. Returns the silence
    /// frames and what happened.
    pub fn check(&mut self, now: u64) -> (Vec<Frame>, Vec<Notice>) {
        let mut frames = Vec::new();
        let mut notices = Vec::new();
        for lane in self.lanes.iter_mut().filter(|l| !l.ended) {
            match lane.pos {
                None if now >= ms_to_samples(START_MS) => {
                    lane.ended = true;
                    notices.push(Notice::NeverStarted {
                        channel: lane.channel,
                    });
                }
                Some(pos) if now.saturating_sub(lane.last_audio) >= ms_to_samples(STALL_MS) => {
                    lane.ended = true;
                    notices.push(Notice::Lost {
                        channel: lane.channel,
                        at: pos,
                    });
                }
                _ => {}
            }
        }
        for lane in self.lanes.iter_mut().filter(|l| l.live()) {
            let pos = lane.pos.unwrap_or(0);
            lane.lags.push_back(now as i64 - pos as i64);
            if lane.lags.len() > LAG_WINDOW {
                lane.lags.pop_front();
            }
        }
        // Smoothed lag of every live channel with a full window; the one
        // furthest ahead is the reference.
        let smoothed: Vec<(usize, i64)> = self
            .lanes
            .iter()
            .enumerate()
            .filter(|(_, l)| l.live() && l.lags.len() == LAG_WINDOW)
            .map(|(i, l)| (i, l.lags.iter().copied().min().unwrap_or(0)))
            .collect();
        if smoothed.len() >= 2 {
            let reference = smoothed.iter().map(|&(_, lag)| lag).min().unwrap_or(0);
            for &(i, lag) in &smoothed {
                let behind = lag - reference;
                if behind <= DRIFT_LIMIT {
                    continue;
                }
                let lane = &mut self.lanes[i];
                let start = lane.pos.unwrap_or(0);
                lane.pos = Some(start + behind as u64);
                for l in lane.lags.iter_mut() {
                    *l -= behind;
                }
                self.resyncs += 1;
                frames.push(Frame {
                    channel: lane.channel,
                    start,
                    samples: vec![0.0; behind as usize],
                });
                notices.push(Notice::Realigned {
                    channel: lane.channel,
                    ms: behind as u64 * 1000 / RATE,
                    at: start,
                });
            }
        }
        (frames, notices)
    }

    /// Every channel has ended (lost or never started).
    pub fn all_ended(&self) -> bool {
        self.lanes.iter().all(|l| l.ended)
    }

    pub fn is_ended(&self, channel: Channel) -> bool {
        self.lanes.iter().any(|l| l.channel == channel && l.ended)
    }

    /// Some channel delivered audio at some point.
    pub fn started_any(&self) -> bool {
        self.lanes.iter().any(|l| l.pos.is_some())
    }

    /// Where `channel`'s next sample goes, once it has started.
    pub fn position(&self, channel: Channel) -> Option<u64> {
        self.lanes
            .iter()
            .find(|l| l.channel == channel)
            .and_then(|l| l.pos)
    }

    /// Realignments so far.
    pub fn resyncs(&self) -> u32 {
        self.resyncs
    }
}

// ---- capture and pacing (real devices, or fakes in tests) -------------------

/// One live input device, already converted to 16 kHz mono.
pub trait Capture: Send {
    /// Everything captured since the last call.
    fn take(&mut self) -> Vec<f32>;
    /// The device went away (or never opened): nothing more will come.
    fn failed(&self) -> bool;
    /// Stop capturing; returns what is left (the resampler's tail).
    fn stop(&mut self) -> Vec<f32>;
}

/// The session's wall clock and poll rhythm.
pub trait Pacer: Send {
    /// Wait for the next poll.
    fn wait(&mut self);
    /// Time since the session started, in 16 kHz samples.
    fn now(&self) -> u64;
}

/// Real time: [`POLL`] sleeps on a monotonic clock.
pub struct WallPacer {
    started: Instant,
}

impl WallPacer {
    pub fn new() -> Self {
        Self {
            started: Instant::now(),
        }
    }
}

impl Default for WallPacer {
    fn default() -> Self {
        Self::new()
    }
}

impl Pacer for WallPacer {
    fn wait(&mut self) {
        std::thread::sleep(POLL);
    }
    fn now(&self) -> u64 {
        (self.started.elapsed().as_micros() * RATE as u128 / 1_000_000) as u64
    }
}

/// A cpal device through its own [`Recorder`].
pub struct RecorderCapture(Recorder);

impl Capture for RecorderCapture {
    fn take(&mut self) -> Vec<f32> {
        self.0.take_new_16k()
    }
    fn failed(&self) -> bool {
        self.0.has_failed()
    }
    fn stop(&mut self) -> Vec<f32> {
        if !self.0.is_recording() {
            return Vec::new();
        }
        self.0.stop().unwrap_or_else(|e| {
            eprintln!("system audio: device stopped with an error ({e:#})");
            Vec::new()
        })
    }
}

// ---- the source -------------------------------------------------------------

struct Input {
    channel: Channel,
    capture: Box<dyn Capture>,
    open: bool,
}

/// The engine's [`Source`] for *New → System audio + mic*: frames of both
/// channels, each contiguous on the session clock (gaps filled with
/// silence), until `stop` (or the run's cancel) is set or both devices are
/// gone.
pub struct SystemSource {
    inputs: Vec<Input>,
    sync: ChannelSync,
    pacer: Box<dyn Pacer>,
    stop: Arc<AtomicBool>,
    cancel: Option<Arc<AtomicBool>>,
    pending: VecDeque<Frame>,
    warnings: Vec<String>,
    finished: bool,
}

impl SystemSource {
    /// Open the microphone (`mic_device`, empty = the default input; a
    /// saved device that is gone falls back to the default, as for a mic
    /// session) and the system audio device (`system_device`, exact name —
    /// never a fallback, it would record the mic twice). The session ends
    /// when `stop` is set.
    pub fn start(
        mic_device: &str,
        system_device: &str,
        stop: Arc<AtomicBool>,
    ) -> anyhow::Result<Self> {
        // The clock starts before the devices open, so a device that is
        // slow to open joins late on the timeline, as it should.
        let pacer = WallPacer::new();
        let mut mic = Recorder::default();
        mic.start(mic_device)?;
        let mut system = Recorder::default();
        system.start_exact(system_device)?;
        Ok(Self::from_parts(
            Box::new(RecorderCapture(mic)),
            Box::new(RecorderCapture(system)),
            Box::new(pacer),
            stop,
        ))
    }

    /// A source over any two captures and clock (tests pass fakes).
    pub fn from_parts(
        mic: Box<dyn Capture>,
        system: Box<dyn Capture>,
        pacer: Box<dyn Pacer>,
        stop: Arc<AtomicBool>,
    ) -> Self {
        Self {
            inputs: vec![
                Input {
                    channel: Channel::Mic,
                    capture: mic,
                    open: true,
                },
                Input {
                    channel: Channel::System,
                    capture: system,
                    open: true,
                },
            ],
            sync: ChannelSync::new(&[Channel::Mic, Channel::System]),
            pacer,
            stop,
            cancel: None,
            pending: VecDeque::new(),
            warnings: Vec::new(),
            finished: false,
        }
    }

    /// Stop waiting for audio once `cancel` is set (the run's cancel flag).
    pub fn with_cancel(mut self, cancel: Arc<AtomicBool>) -> Self {
        self.cancel = Some(cancel);
        self
    }

    /// Realignments so far (diagnostics, tests).
    pub fn resyncs(&self) -> u32 {
        self.sync.resyncs()
    }

    fn drain(&mut self, i: usize, now: u64) {
        let input = &mut self.inputs[i];
        if !input.open {
            return;
        }
        let chunk = input.capture.take();
        if let Some(f) = self.sync.push(input.channel, chunk, now) {
            self.pending.push_back(f);
        }
    }

    /// Stop device `i`, keeping its tail.
    fn close(&mut self, i: usize, now: u64) {
        let input = &mut self.inputs[i];
        if !input.open {
            return;
        }
        input.open = false;
        let tail = input.capture.stop();
        if let Some(f) = self.sync.push(input.channel, tail, now) {
            self.pending.push_back(f);
        }
    }

    fn warn(&mut self, notice: &Notice) {
        let message = notice.message();
        eprintln!("system audio: {message}");
        self.warnings.push(message);
    }

    fn stopping(&self) -> bool {
        self.stop.load(Ordering::Relaxed)
            || self
                .cancel
                .as_ref()
                .is_some_and(|c| c.load(Ordering::Relaxed))
    }
}

impl Source for SystemSource {
    fn channel(&self) -> Channel {
        Channel::System
    }

    fn total_samples(&self) -> Option<u64> {
        None
    }

    fn next_frame(&mut self) -> anyhow::Result<Option<Frame>> {
        loop {
            if let Some(f) = self.pending.pop_front() {
                return Ok(Some(f));
            }
            if self.finished {
                return Ok(None);
            }
            if self.stopping() {
                // What arrived since the last poll, then the tails.
                let now = self.pacer.now();
                for i in 0..self.inputs.len() {
                    self.drain(i, now);
                    self.close(i, now);
                }
                self.finished = true;
                continue;
            }
            if self.sync.all_ended() {
                self.finished = true;
                for i in 0..self.inputs.len() {
                    self.inputs[i].open = false;
                    self.inputs[i].capture.stop();
                }
                if !self.sync.started_any() {
                    anyhow::bail!(
                        "neither the microphone nor the system audio device delivered audio — check the input devices"
                    );
                }
                // Both devices went away mid-session: the run ends
                // normally and keeps what was recorded.
                continue;
            }
            self.pacer.wait();
            let now = self.pacer.now();
            for i in 0..self.inputs.len() {
                if !self.inputs[i].open {
                    continue;
                }
                self.drain(i, now);
                if self.inputs[i].capture.failed() {
                    // The device went away: its channel ends here, with
                    // whatever it still held.
                    let channel = self.inputs[i].channel;
                    self.close(i, now);
                    if self.sync.end(channel) {
                        let notice = match self.sync.position(channel) {
                            Some(at) => Notice::Lost { channel, at },
                            None => Notice::NeverStarted { channel },
                        };
                        self.warn(&notice);
                    }
                }
            }
            let (frames, notices) = self.sync.check(now);
            self.pending.extend(frames);
            for notice in notices {
                if let Notice::Lost { channel, .. } | Notice::NeverStarted { channel } = notice {
                    // Release the device; anything it still held is past
                    // the end of its channel.
                    if let Some(i) = self.inputs.iter().position(|x| x.channel == channel) {
                        self.inputs[i].open = false;
                        self.inputs[i].capture.stop();
                    }
                }
                self.warn(&notice);
            }
        }
    }

    fn take_warnings(&mut self) -> Vec<String> {
        std::mem::take(&mut self.warnings)
    }
}

// ---- device choice ----------------------------------------------------------

/// Name fragments of devices that usually carry the computer's output
/// (virtual loopback drivers, monitor sources, "Stereo Mix"). Only a hint
/// for the picker: any input device can be chosen.
const LOOPBACK_HINTS: &[&str] = &[
    "blackhole",
    "loopback",
    "soundflower",
    "background music",
    "ishowu",
    "vb-audio",
    "vb-cable",
    "cable output",
    "voicemeeter",
    "stereo mix",
    "what u hear",
    "wave out",
    "monitor",
    "virtual",
];

/// Whether an input device's name looks like a loopback/virtual device.
/// Pure.
pub fn looks_like_loopback(name: &str) -> bool {
    let n = name.to_lowercase();
    LOOPBACK_HINTS.iter().any(|h| n.contains(h))
}

/// Check the two devices before a session starts: a system device must be
/// chosen and must not be the microphone (by name, or the default input
/// when the mic is the default). Pure.
pub fn validate_devices(
    mic: &str,
    system: &str,
    default_input: Option<&str>,
) -> anyhow::Result<()> {
    let system = system.trim();
    if system.is_empty() {
        anyhow::bail!("choose the system audio device (a loopback device such as BlackHole, VB-Cable or a monitor source)");
    }
    let mic = mic.trim();
    let mic_name = if mic.is_empty() {
        default_input.unwrap_or("")
    } else {
        mic
    };
    if mic_name == system {
        anyhow::bail!(
            "the system audio device is the microphone — choose a different device for one of them"
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::resample::StreamResampler;
    use std::sync::atomic::AtomicU64;
    use std::sync::Mutex;

    const S: u64 = RATE; // one second in samples
    const TICK: u64 = RATE / 4; // one poll

    // ---- ChannelSync (pure) ----

    #[test]
    fn channels_are_timestamped_by_their_own_samples_and_join_late() {
        let mut sync = ChannelSync::new(&[Channel::Mic, Channel::System]);
        // Mic: 3 900 samples by the first poll (it opened ~6 ms late).
        let a = sync.push(Channel::Mic, vec![0.1; 3_900], TICK).unwrap();
        assert_eq!((a.channel, a.start), (Channel::Mic, 100));
        // System opened 150 ms late: 1 600 samples at the same poll.
        let b = sync.push(Channel::System, vec![0.3; 1_600], TICK).unwrap();
        assert_eq!((b.channel, b.start), (Channel::System, 2_400));
        // From then on each channel follows its own samples, whatever the
        // wall clock says.
        let c = sync.push(Channel::Mic, vec![0.1; 4_100], 2 * TICK).unwrap();
        assert_eq!(c.start, 4_000);
        let d = sync
            .push(Channel::System, vec![0.3; 3_950], 2 * TICK)
            .unwrap();
        assert_eq!(d.start, 4_000);
        assert!(sync.push(Channel::Mic, Vec::new(), 3 * TICK).is_none());
        assert_eq!(sync.position(Channel::Mic), Some(8_100));
    }

    #[test]
    fn an_ended_channel_takes_no_more_audio() {
        let mut sync = ChannelSync::new(&[Channel::Mic, Channel::System]);
        sync.push(Channel::System, vec![0.0; 4_000], TICK);
        assert!(sync.end(Channel::System));
        assert!(!sync.end(Channel::System), "only once");
        assert!(sync
            .push(Channel::System, vec![0.0; 4_000], 2 * TICK)
            .is_none());
        assert!(sync.is_ended(Channel::System) && !sync.all_ended());
        assert!(sync.end(Channel::Mic));
        assert!(sync.all_ended());
    }

    #[test]
    fn a_channel_that_falls_behind_is_realigned_with_silence() {
        let mut sync = ChannelSync::new(&[Channel::Mic, Channel::System]);
        let mut resynced = Vec::new();
        // System delivers 2 % less than real time: 80 samples short a poll.
        for t in 1..=60u64 {
            let now = t * TICK;
            sync.push(Channel::Mic, vec![0.1; TICK as usize], now);
            sync.push(Channel::System, vec![0.3; (TICK - 80) as usize], now);
            let (frames, notices) = sync.check(now);
            for f in frames {
                assert_eq!(f.channel, Channel::System, "only the slow side is padded");
                assert!(f.samples.iter().all(|&x| x == 0.0));
                resynced.push((now, f.samples.len()));
            }
            assert!(notices.iter().all(|n| matches!(
                n,
                Notice::Realigned {
                    channel: Channel::System,
                    ..
                }
            )));
        }
        // 60 polls × 80 = 4 800 samples (300 ms) of drift: one realignment
        // once it passes 200 ms, not before.
        assert_eq!(resynced.len(), 1, "{resynced:?}");
        let (at, len) = resynced[0];
        assert!(len as i64 > DRIFT_LIMIT && len < 4_000, "{len}");
        assert!(
            at >= 40 * TICK,
            "not before the drift passed the limit ({at})"
        );
        let gap = sync.position(Channel::Mic).unwrap() as i64
            - sync.position(Channel::System).unwrap() as i64;
        assert!(gap.abs() <= DRIFT_LIMIT, "{gap}");
    }

    #[test]
    fn delivery_jitter_is_not_drift() {
        let mut sync = ChannelSync::new(&[Channel::Mic, Channel::System]);
        // The system device delivers 500 ms blocks every other poll (a
        // Bluetooth-like buffer): its lag swings by 250 ms but never drifts.
        for t in 1..=400u64 {
            let now = t * TICK;
            sync.push(Channel::Mic, vec![0.1; TICK as usize], now);
            if t % 2 == 0 {
                sync.push(Channel::System, vec![0.3; 2 * TICK as usize], now);
            }
            let (frames, notices) = sync.check(now);
            assert!(
                frames.is_empty() && notices.is_empty(),
                "poll {t}: {notices:?}"
            );
        }
        assert_eq!(sync.resyncs(), 0);
    }

    #[test]
    fn a_stalled_channel_ends_and_one_that_never_starts_is_reported() {
        let mut sync = ChannelSync::new(&[Channel::Mic, Channel::System]);
        let mut notices = Vec::new();
        for t in 1..=40u64 {
            let now = t * TICK;
            // The mic stops delivering after 2 s; system never delivers.
            if t <= 8 {
                sync.push(Channel::Mic, vec![0.1; TICK as usize], now);
            }
            notices.extend(sync.check(now).1);
        }
        assert!(notices.contains(&Notice::NeverStarted {
            channel: Channel::System
        }));
        assert!(notices.contains(&Notice::Lost {
            channel: Channel::Mic,
            at: 2 * S
        }));
        assert!(sync.all_ended() && sync.started_any());
    }

    // ---- the source, over fake devices and a fake clock ----

    /// Wall clock of a fake session, in 16 kHz samples.
    type Wall = Arc<AtomicU64>;

    struct FakePacer(Wall);

    impl Pacer for FakePacer {
        fn wait(&mut self) {
            self.0.fetch_add(TICK, Ordering::SeqCst);
        }
        fn now(&self) -> u64 {
            self.0.load(Ordering::SeqCst)
        }
    }

    /// A device at a nominal `rate` whose clock really runs at
    /// `rate × (1 + skew)`, converted by its own `StreamResampler` (as a
    /// `Recorder` does). It produces a constant `level`, so every output
    /// sample says which device it came from.
    struct FakeDevice {
        wall: Wall,
        rate: u32,
        skew: f64,
        level: f32,
        opens_at: u64,
        /// Stops delivering (stalls) from this wall time on.
        stalls_at: Option<u64>,
        /// Reports `DeviceNotAvailable` from this wall time on.
        fails_at: Option<u64>,
        produced: u64,
        resampler: StreamResampler,
        /// 16 kHz samples handed out (audio, not silence).
        out: Arc<Mutex<u64>>,
    }

    impl FakeDevice {
        fn new(wall: &Wall, rate: u32, level: f32) -> Self {
            Self {
                wall: wall.clone(),
                rate,
                skew: 0.0,
                level,
                opens_at: 0,
                stalls_at: None,
                fails_at: None,
                produced: 0,
                resampler: StreamResampler::new(1, rate, RATE as u32),
                out: Arc::default(),
            }
        }
    }

    impl Capture for FakeDevice {
        fn take(&mut self) -> Vec<f32> {
            let mut now = self.wall.load(Ordering::SeqCst);
            for limit in [self.stalls_at, self.fails_at].into_iter().flatten() {
                now = now.min(limit);
            }
            if now <= self.opens_at {
                return Vec::new();
            }
            let secs = (now - self.opens_at) as f64 / RATE as f64;
            let due = (secs * self.rate as f64 * (1.0 + self.skew)) as u64;
            let n = due.saturating_sub(self.produced);
            self.produced = due;
            let out = self.resampler.push(&vec![self.level; n as usize]);
            *self.out.lock().unwrap() += out.len() as u64;
            out
        }
        fn failed(&self) -> bool {
            self.fails_at
                .is_some_and(|t| self.wall.load(Ordering::SeqCst) >= t)
        }
        fn stop(&mut self) -> Vec<f32> {
            let out = self.resampler.flush();
            *self.out.lock().unwrap() += out.len() as u64;
            out
        }
    }

    struct Run {
        frames: Vec<Frame>,
        warnings: Vec<String>,
        resyncs: u32,
        error: Option<String>,
    }

    /// Run a source for `secs` of fake time, then stop it.
    fn run(mic: FakeDevice, system: FakeDevice, wall: &Wall, secs: u64) -> Run {
        let stop = Arc::new(AtomicBool::new(false));
        let mut src = SystemSource::from_parts(
            Box::new(mic),
            Box::new(system),
            Box::new(FakePacer(wall.clone())),
            stop.clone(),
        );
        let mut frames = Vec::new();
        let mut warnings = Vec::new();
        let error = loop {
            if wall.load(Ordering::SeqCst) >= secs * S {
                stop.store(true, Ordering::SeqCst);
            }
            match src.next_frame() {
                Ok(Some(f)) => frames.push(f),
                Ok(None) => break None,
                Err(e) => break Some(format!("{e:#}")),
            }
            warnings.extend(src.take_warnings());
        };
        warnings.extend(src.take_warnings());
        Run {
            frames,
            warnings,
            resyncs: src.resyncs(),
            error,
        }
    }

    /// Frames of one channel: contiguous, and (audio, silence) totals.
    fn channel(frames: &[Frame], ch: Channel) -> (u64, u64, u64, u64) {
        let mine: Vec<&Frame> = frames.iter().filter(|f| f.channel == ch).collect();
        for w in mine.windows(2) {
            assert_eq!(
                w[0].start + w[0].samples.len() as u64,
                w[1].start,
                "{ch:?} frames must be contiguous"
            );
        }
        let first = mine.first().map(|f| f.start).unwrap_or(0);
        let end = mine
            .last()
            .map(|f| f.start + f.samples.len() as u64)
            .unwrap_or(0);
        let silence: u64 = mine
            .iter()
            .flat_map(|f| f.samples.iter())
            .filter(|&&x| x == 0.0)
            .count() as u64;
        let total: u64 = mine.iter().map(|f| f.samples.len() as u64).sum();
        (first, end, total - silence, silence)
    }

    #[test]
    fn two_devices_at_different_rates_become_two_labelled_channels() {
        let wall: Wall = Arc::default();
        let mic = FakeDevice::new(&wall, 48_000, 0.1);
        let mut system = FakeDevice::new(&wall, 44_100, 0.3);
        system.opens_at = 3_000; // ~190 ms after the mic
        let (mic_out, sys_out) = (mic.out.clone(), system.out.clone());
        let r = run(mic, system, &wall, 10);
        assert!(
            r.error.is_none() && r.warnings.is_empty(),
            "{:?}",
            r.warnings
        );
        assert_eq!(r.resyncs, 0);
        // Every frame is tagged with its device: the mic's level on `mic`,
        // the system device's on `system`.
        for f in &r.frames {
            let level = match f.channel {
                Channel::Mic => 0.1,
                Channel::System => 0.3,
                other => panic!("unexpected channel {other:?}"),
            };
            assert!(f.samples.iter().all(|&x| (x - level).abs() < 1e-6));
        }
        let (m0, m_end, m_audio, m_sil) = channel(&r.frames, Channel::Mic);
        let (s0, s_end, s_audio, s_sil) = channel(&r.frames, Channel::System);
        // Nothing dropped, nothing added: each channel is exactly what its
        // own resampler produced (10 s → 160 000 samples at 16 kHz).
        assert_eq!((m_audio, m_sil), (*mic_out.lock().unwrap(), 0));
        assert_eq!((s_audio, s_sil), (*sys_out.lock().unwrap(), 0));
        assert!((159_900..=160_100).contains(&m_audio), "{m_audio}");
        assert!((156_900..=157_100).contains(&s_audio), "{s_audio}");
        // The late device joins late, on the same timeline.
        assert_eq!(m0, 0);
        assert!((2_900..=3_100).contains(&s0), "{s0}");
        assert!(m_end.abs_diff(s_end) < 200, "{m_end} vs {s_end}");
    }

    #[test]
    fn clock_drift_between_devices_is_realigned_both_ways() {
        for (mic_skew, sys_skew, padded) in [
            (0.0, -0.01, Channel::System), // system clock 1 % slow
            (0.01, 0.0, Channel::System),  // mic clock 1 % fast
            (-0.01, 0.0, Channel::Mic),    // mic clock 1 % slow
        ] {
            let wall: Wall = Arc::default();
            let mut mic = FakeDevice::new(&wall, 48_000, 0.1);
            mic.skew = mic_skew;
            let mut system = FakeDevice::new(&wall, 44_100, 0.3);
            system.skew = sys_skew;
            let (mic_out, sys_out) = (mic.out.clone(), system.out.clone());
            // 60 s at 1 % = 600 ms of drift.
            let r = run(mic, system, &wall, 60);
            let case = format!("mic {mic_skew}, system {sys_skew}");
            assert!(r.error.is_none(), "{case}");
            assert!(r.resyncs >= 2, "{case}: {} resyncs", r.resyncs);
            assert!(r.warnings.iter().all(|w| w.contains("drifted")), "{case}");
            assert_eq!(r.warnings.len() as u32, r.resyncs, "{case}");
            let (_, m_end, m_audio, m_sil) = channel(&r.frames, Channel::Mic);
            let (_, s_end, s_audio, s_sil) = channel(&r.frames, Channel::System);
            // No audio lost on either side; silence only where it lagged.
            assert_eq!(m_audio, *mic_out.lock().unwrap(), "{case}");
            assert_eq!(s_audio, *sys_out.lock().unwrap(), "{case}");
            let (pad, other_pad) = if padded == Channel::Mic {
                (m_sil, s_sil)
            } else {
                (s_sil, m_sil)
            };
            assert!(
                pad > DRIFT_LIMIT as u64 && other_pad == 0,
                "{case}: {pad} / {other_pad}"
            );
            // The two channels end within the limit of each other.
            assert!(
                m_end.abs_diff(s_end) <= DRIFT_LIMIT as u64 + TICK,
                "{case}: {m_end} vs {s_end}"
            );
        }
    }

    #[test]
    fn a_device_that_disappears_ends_its_channel_and_the_other_goes_on() {
        let wall: Wall = Arc::default();
        let mic = FakeDevice::new(&wall, 48_000, 0.1);
        let mut system = FakeDevice::new(&wall, 48_000, 0.3);
        system.fails_at = Some(4 * S);
        let r = run(mic, system, &wall, 10);
        assert!(r.error.is_none());
        assert_eq!(r.warnings.len(), 1, "{:?}", r.warnings);
        assert!(r.warnings[0].contains("system audio device stopped delivering audio at 0:04"));
        let (_, m_end, ..) = channel(&r.frames, Channel::Mic);
        let (_, s_end, ..) = channel(&r.frames, Channel::System);
        assert!(s_end <= 4 * S + 100, "{s_end}");
        assert!(m_end >= 10 * S - 100, "the mic kept recording: {m_end}");
    }

    #[test]
    fn a_stalled_microphone_counts_as_lost() {
        let wall: Wall = Arc::default();
        let mut mic = FakeDevice::new(&wall, 44_100, 0.1);
        mic.stalls_at = Some(3 * S);
        let system = FakeDevice::new(&wall, 48_000, 0.3);
        let r = run(mic, system, &wall, 12);
        assert!(r.error.is_none());
        assert!(
            r.warnings
                .iter()
                .any(|w| w.contains("microphone stopped delivering audio")),
            "{:?}",
            r.warnings
        );
        let (_, s_end, ..) = channel(&r.frames, Channel::System);
        assert!(s_end >= 12 * S - 100);
        // While it stalled the mic was realigned with silence, never past
        // the moment it counted as lost.
        let (_, m_end, m_audio, _) = channel(&r.frames, Channel::Mic);
        assert!(
            m_audio <= 3 * S + 100 && m_end <= 8 * S + TICK,
            "{m_audio} {m_end}"
        );
    }

    #[test]
    fn a_device_that_never_delivers_leaves_the_other_channel() {
        let wall: Wall = Arc::default();
        let mic = FakeDevice::new(&wall, 48_000, 0.1);
        let mut system = FakeDevice::new(&wall, 48_000, 0.3);
        system.opens_at = u64::MAX;
        let r = run(mic, system, &wall, 8);
        assert!(r.error.is_none());
        assert_eq!(r.warnings.len(), 1);
        assert!(r.warnings[0].contains("system audio device delivered no audio"));
        assert!(r.frames.iter().all(|f| f.channel == Channel::Mic));
    }

    #[test]
    fn both_devices_lost_ends_the_session_normally() {
        let wall: Wall = Arc::default();
        let mut mic = FakeDevice::new(&wall, 48_000, 0.1);
        mic.fails_at = Some(2 * S);
        let mut system = FakeDevice::new(&wall, 48_000, 0.3);
        system.fails_at = Some(3 * S);
        // No stop: the source ends by itself, keeping the audio.
        let r = run(mic, system, &wall, 3_600);
        assert!(r.error.is_none());
        assert_eq!(r.warnings.len(), 2);
        assert!(wall.load(Ordering::SeqCst) < 4 * S);
        let (_, m_end, ..) = channel(&r.frames, Channel::Mic);
        let (_, s_end, ..) = channel(&r.frames, Channel::System);
        assert!((2 * S - 100..=2 * S + 100).contains(&m_end), "{m_end}");
        assert!((3 * S - 100..=3 * S + 100).contains(&s_end), "{s_end}");
    }

    #[test]
    fn neither_device_delivering_is_an_error() {
        let wall: Wall = Arc::default();
        let mut mic = FakeDevice::new(&wall, 48_000, 0.1);
        mic.opens_at = u64::MAX;
        let mut system = FakeDevice::new(&wall, 48_000, 0.3);
        system.fails_at = Some(0);
        let r = run(mic, system, &wall, 3_600);
        assert!(r.error.unwrap().contains("neither"));
        assert!(r.frames.is_empty());
    }

    #[test]
    fn cancel_stops_the_source() {
        let wall: Wall = Arc::default();
        let cancel = Arc::new(AtomicBool::new(true));
        let mut src = SystemSource::from_parts(
            Box::new(FakeDevice::new(&wall, 48_000, 0.1)),
            Box::new(FakeDevice::new(&wall, 48_000, 0.3)),
            Box::new(FakePacer(wall.clone())),
            Arc::new(AtomicBool::new(false)),
        )
        .with_cancel(cancel);
        assert_eq!(src.next_frame().unwrap(), None);
        assert_eq!(src.channel(), Channel::System);
    }

    // ---- device choice ----

    #[test]
    fn loopback_devices_are_recognised_by_name() {
        for name in [
            "BlackHole 2ch",
            "Loopback Audio",
            "CABLE Output (VB-Audio Virtual Cable)",
            "Voicemeeter Out B1 (VB-Audio Voicemeeter VAIO)",
            "Stereo Mix (Realtek(R) Audio)",
            "Monitor of Built-in Audio Analog Stereo",
            "alsa_output.pci-0000_00_1f.3.analog-stereo.monitor",
            "system_monitor",
        ] {
            assert!(looks_like_loopback(name), "{name}");
        }
        for name in [
            "MacBook Pro Microphone",
            "USB Audio Device",
            "default",
            "Headset (AirPods)",
        ] {
            assert!(!looks_like_loopback(name), "{name}");
        }
    }

    #[test]
    fn the_two_devices_must_differ() {
        assert!(validate_devices("", "BlackHole 2ch", Some("MacBook Pro Microphone")).is_ok());
        assert!(validate_devices("USB Mic", "BlackHole 2ch", None).is_ok());
        assert!(
            validate_devices("", " ", None).is_err(),
            "a system device is required"
        );
        assert!(validate_devices("BlackHole 2ch", "BlackHole 2ch", None).is_err());
        // The mic is the default input, and that is the chosen system device.
        assert!(validate_devices("", "BlackHole 2ch", Some("BlackHole 2ch")).is_err());
    }

    /// Needs a microphone and a loopback device. Run manually, with some
    /// audio playing:
    /// `SUSSURRO_SYSTEM_DEVICE="BlackHole 2ch" cargo test real_devices -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn real_devices_record_two_channels() {
        let device = std::env::var("SUSSURRO_SYSTEM_DEVICE")
            .expect("set SUSSURRO_SYSTEM_DEVICE to a loopback input device's name");
        let stop = Arc::new(AtomicBool::new(false));
        let mut src = SystemSource::start("", &device, stop.clone()).unwrap();
        let started = Instant::now();
        // Samples per channel: [mic, system].
        let mut per = [0u64; 2];
        while let Some(f) = src.next_frame().unwrap() {
            per[usize::from(f.channel == Channel::System)] += f.samples.len() as u64;
            for w in src.take_warnings() {
                println!("warning: {w}");
            }
            if started.elapsed() > Duration::from_secs(4) {
                stop.store(true, Ordering::Relaxed);
            }
        }
        println!("samples [mic, system]: {per:?}, resyncs: {}", src.resyncs());
        assert!(per.iter().all(|&n| n > 3 * S), "{per:?}");
    }
}
