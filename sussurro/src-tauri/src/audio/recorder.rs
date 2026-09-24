use anyhow::{anyhow, Context, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};

use crate::audio::resample::StreamResampler;

pub const TARGET_RATE: u32 = 16_000;

/// Owns no audio resources directly — the cpal Stream is !Send, so each
/// recording runs on its own thread and hands samples back over a channel.
/// The buffer is shared so live previews can snapshot it mid-recording.
#[derive(Default)]
pub struct Recorder {
    stop_tx: Option<Sender<()>>,
    result_rx: Option<Receiver<Result<Vec<f32>>>>,
    /// Capture converted to 16 kHz mono as it arrives (the raw device-rate
    /// audio is never buffered whole — see `StreamResampler`).
    live: Arc<Mutex<Vec<f32>>>,
    /// (device sample rate, channel count) — 0 until the stream is up.
    meta: Arc<(AtomicU32, AtomicUsize)>,
    /// The device went away mid-recording (unplugged, removed virtual
    /// device) or could not be opened: the capture delivers nothing more.
    /// Read by long sessions (#139) to end that channel; the dictation path
    /// ignores it (its `stop()` reports the error as before).
    failed: Arc<AtomicBool>,
}

impl Recorder {
    pub fn is_recording(&self) -> bool {
        self.stop_tx.is_some()
    }

    /// `device_name`: empty = system default input. A named device that is
    /// gone falls back to the default input.
    pub fn start(&mut self, device_name: &str) -> Result<()> {
        self.start_with(device_name, true)
    }

    /// Like [`Self::start`], but a named device that is gone is an error
    /// (reported through [`Self::has_failed`]) instead of a fallback to the
    /// default input — the system-audio channel (#139) must never silently
    /// record the microphone twice.
    pub fn start_exact(&mut self, device_name: &str) -> Result<()> {
        self.start_with(device_name, false)
    }

    fn start_with(&mut self, device_name: &str, fallback: bool) -> Result<()> {
        if self.is_recording() {
            return Ok(());
        }
        self.live.lock().unwrap().clear();
        self.meta.0.store(0, Ordering::Relaxed);
        self.meta.1.store(0, Ordering::Relaxed);
        self.failed.store(false, Ordering::Relaxed);

        let (stop_tx, stop_rx) = channel();
        let (result_tx, result_rx) = channel();
        let buffer = self.live.clone();
        let meta = self.meta.clone();
        let failed = self.failed.clone();
        let device_name = device_name.to_string();
        std::thread::spawn(move || {
            let r = record_until_stopped(stop_rx, buffer, meta, failed.clone(), &device_name, fallback);
            if r.is_err() {
                failed.store(true, Ordering::Relaxed);
            }
            let _ = result_tx.send(r);
        });
        self.stop_tx = Some(stop_tx);
        self.result_rx = Some(result_rx);
        Ok(())
    }

    /// Copy of everything captured so far, already 16 kHz mono — for live
    /// preview transcription while the recording continues. None until the
    /// audio stream has actually started. The buffer is stored converted, so
    /// this is a plain copy — no per-snapshot downmix/resample of the whole
    /// capture.
    pub fn snapshot_16k(&self) -> Option<Vec<f32>> {
        let rate = self.meta.0.load(Ordering::Relaxed);
        let channels = self.meta.1.load(Ordering::Relaxed);
        if !self.is_recording() || rate == 0 || channels == 0 {
            return None;
        }
        Some(self.live.lock().unwrap().clone())
    }

    /// Take (drain) everything captured since the last call, 16 kHz mono —
    /// the long-form engine's mic source (#113) consumes a session this way
    /// so the recorder never holds more than one poll interval of audio. A
    /// recording that is drained returns only the undrained tail from
    /// `stop()`. The dictation path never calls this (it snapshots and
    /// stops), so its behaviour is unchanged.
    pub fn take_new_16k(&self) -> Vec<f32> {
        if !self.is_recording() {
            return Vec::new();
        }
        std::mem::take(&mut *self.live.lock().unwrap())
    }

    /// RMS amplitude of the most recent ~100 ms of capture (16 kHz mono) —
    /// the live input level for the mic VU meter. None until the stream is up.
    pub fn level(&self) -> Option<f32> {
        let rate = self.meta.0.load(Ordering::Relaxed);
        let channels = self.meta.1.load(Ordering::Relaxed);
        if !self.is_recording() || rate == 0 || channels == 0 {
            return None;
        }
        let buf = self.live.lock().unwrap();
        let window = TARGET_RATE as usize / 10;
        Some(crate::audio::resample::rms(&buf[buf.len().saturating_sub(window)..]))
    }

    /// The device disappeared while recording, or could not be opened: no
    /// more audio will come. False when idle.
    pub fn has_failed(&self) -> bool {
        self.is_recording() && self.failed.load(Ordering::Relaxed)
    }

    /// Returns 16 kHz mono f32 samples.
    pub fn stop(&mut self) -> Result<Vec<f32>> {
        let stop_tx = self.stop_tx.take().ok_or_else(|| anyhow!("not recording"))?;
        let result_rx = self.result_rx.take().ok_or_else(|| anyhow!("not recording"))?;
        let _ = stop_tx.send(());
        result_rx.recv().context("recording thread died")?
    }
}

/// Names of available input devices. The empty string always means "system
/// default" to callers and is not included here.
pub fn list_input_devices() -> Vec<String> {
    let host = cpal::default_host();
    let Ok(devices) = host.input_devices() else {
        return Vec::new();
    };
    devices.filter_map(|d| d.name().ok()).collect()
}

/// Name of the system default input device, if there is one.
pub fn default_input_device_name() -> Option<String> {
    cpal::default_host().default_input_device().and_then(|d| d.name().ok())
}

fn record_until_stopped(
    stop_rx: Receiver<()>,
    buffer: Arc<Mutex<Vec<f32>>>,
    meta: Arc<(AtomicU32, AtomicUsize)>,
    failed: Arc<AtomicBool>,
    device_name: &str,
    fallback: bool,
) -> Result<Vec<f32>> {
    let host = cpal::default_host();
    let device = if device_name.is_empty() {
        host.default_input_device()
    } else {
        let named = host
            .input_devices()
            .ok()
            .and_then(|mut ds| ds.find(|d| d.name().map(|n| n == device_name).unwrap_or(false)));
        if named.is_none() && !fallback {
            return Err(anyhow!("the input device '{device_name}' is not available"));
        }
        // Fall back to default if the saved device is gone (unplugged).
        named.or_else(|| host.default_input_device())
    }
    .ok_or_else(|| anyhow!("no input device — check microphone privacy settings"))?;
    let config = device.default_input_config()?;
    let channels = config.channels() as usize;
    let rate = config.sample_rate().0;
    meta.0.store(rate, Ordering::Relaxed);
    meta.1.store(channels, Ordering::Relaxed);

    // A device that goes away (unplugged, a virtual device removed) ends
    // the capture; other stream errors (an xrun) are only logged.
    let err_fn = move |e: cpal::StreamError| {
        eprintln!("audio stream error: {e}");
        if matches!(e, cpal::StreamError::DeviceNotAvailable) {
            failed.store(true, Ordering::Relaxed);
        }
    };

    // Convert to 16 kHz mono inside the callback so only the converted audio
    // is ever buffered (~64 KB/s instead of the raw device rate). Shared with
    // this thread so the tail held by the resampler can be flushed after stop.
    let conv = Arc::new(Mutex::new(StreamResampler::new(channels, rate, TARGET_RATE)));

    let stream = match config.sample_format() {
        cpal::SampleFormat::F32 => {
            let buf = buffer.clone();
            let conv = conv.clone();
            device.build_input_stream(
                &config.into(),
                move |data: &[f32], _| {
                    let out = conv.lock().unwrap().push(data);
                    buf.lock().unwrap().extend_from_slice(&out);
                },
                err_fn,
                None,
            )?
        }
        cpal::SampleFormat::I16 => {
            let buf = buffer.clone();
            let conv = conv.clone();
            device.build_input_stream(
                &config.into(),
                move |data: &[i16], _| {
                    let floats: Vec<f32> =
                        data.iter().map(|s| *s as f32 / i16::MAX as f32).collect();
                    let out = conv.lock().unwrap().push(&floats);
                    buf.lock().unwrap().extend_from_slice(&out);
                },
                err_fn,
                None,
            )?
        }
        f => return Err(anyhow!("unsupported sample format: {f}")),
    };

    stream.play()?;
    let _ = stop_rx.recv(); // blocks until stop() is called (or Recorder is dropped)
    drop(stream);

    // Callbacks have stopped: take the buffer (no full-size clone) and append
    // the interpolation tail the resampler was holding back.
    let mut samples = std::mem::take(&mut *buffer.lock().unwrap());
    samples.extend(conv.lock().unwrap().flush());
    Ok(samples)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stop_without_start_errors() {
        let mut r = Recorder::default();
        assert!(!r.is_recording());
        assert!(r.stop().is_err());
    }

    #[test]
    fn snapshot_is_none_when_idle() {
        let r = Recorder::default();
        assert!(r.snapshot_16k().is_none());
    }

    #[test]
    fn take_new_is_empty_when_idle() {
        let r = Recorder::default();
        r.live.lock().unwrap().extend_from_slice(&[0.1, 0.2]);
        // Not recording: nothing is drained (and the buffer is left alone).
        assert!(r.take_new_16k().is_empty());
        assert_eq!(r.live.lock().unwrap().len(), 2);
    }

    #[test]
    fn has_failed_is_false_when_idle() {
        let r = Recorder::default();
        r.failed.store(true, Ordering::Relaxed);
        // Not recording: a stale flag from an earlier stream means nothing.
        assert!(!r.has_failed());
    }

    #[test]
    fn level_is_none_when_idle() {
        let r = Recorder::default();
        assert!(r.level().is_none());
    }

    /// Needs a real microphone. Run manually: cargo test record_one_second -- --ignored --nocapture
    #[test]
    #[ignore]
    fn record_one_second() {
        let mut r = Recorder::default();
        r.start("").unwrap();
        assert!(r.is_recording());
        std::thread::sleep(std::time::Duration::from_millis(600));
        let snap = r.snapshot_16k();
        std::thread::sleep(std::time::Duration::from_millis(400));
        let samples = r.stop().unwrap();
        println!(
            "captured {} samples at 16 kHz (snapshot at 600ms: {:?})",
            samples.len(),
            snap.as_ref().map(Vec::len)
        );
        // ~1 s of 16 kHz audio, generous tolerance for stream startup latency
        assert!(samples.len() > 8_000, "expected >8000 samples, got {}", samples.len());
        let snap = snap.expect("snapshot mid-recording");
        assert!(snap.len() < samples.len());
    }
}
