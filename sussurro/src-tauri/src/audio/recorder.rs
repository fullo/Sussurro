use anyhow::{anyhow, Context, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};

use crate::audio::resample::{downmix_to_mono, resample_linear};

pub const TARGET_RATE: u32 = 16_000;

/// Owns no audio resources directly — the cpal Stream is !Send, so each
/// recording runs on its own thread and hands samples back over a channel.
/// The raw buffer is shared so live previews can snapshot it mid-recording.
#[derive(Default)]
pub struct Recorder {
    stop_tx: Option<Sender<()>>,
    result_rx: Option<Receiver<Result<Vec<f32>>>>,
    live: Arc<Mutex<Vec<f32>>>,
    /// (device sample rate, channel count) — 0 until the stream is up.
    meta: Arc<(AtomicU32, AtomicUsize)>,
}

impl Recorder {
    pub fn is_recording(&self) -> bool {
        self.stop_tx.is_some()
    }

    /// `device_name`: empty = system default input.
    pub fn start(&mut self, device_name: &str) -> Result<()> {
        if self.is_recording() {
            return Ok(());
        }
        self.live.lock().unwrap().clear();
        self.meta.0.store(0, Ordering::Relaxed);
        self.meta.1.store(0, Ordering::Relaxed);

        let (stop_tx, stop_rx) = channel();
        let (result_tx, result_rx) = channel();
        let buffer = self.live.clone();
        let meta = self.meta.clone();
        let device_name = device_name.to_string();
        std::thread::spawn(move || {
            let _ = result_tx.send(record_until_stopped(stop_rx, buffer, meta, &device_name));
        });
        self.stop_tx = Some(stop_tx);
        self.result_rx = Some(result_rx);
        Ok(())
    }

    /// Copy of everything captured so far, already 16 kHz mono — for live
    /// preview transcription while the recording continues. None until the
    /// audio stream has actually started.
    pub fn snapshot_16k(&self) -> Option<Vec<f32>> {
        let rate = self.meta.0.load(Ordering::Relaxed);
        let channels = self.meta.1.load(Ordering::Relaxed);
        if !self.is_recording() || rate == 0 || channels == 0 {
            return None;
        }
        let raw = self.live.lock().unwrap().clone();
        let mono = downmix_to_mono(&raw, channels);
        Some(resample_linear(&mono, rate, TARGET_RATE))
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

fn record_until_stopped(
    stop_rx: Receiver<()>,
    buffer: Arc<Mutex<Vec<f32>>>,
    meta: Arc<(AtomicU32, AtomicUsize)>,
    device_name: &str,
) -> Result<Vec<f32>> {
    let host = cpal::default_host();
    let device = if device_name.is_empty() {
        host.default_input_device()
    } else {
        // Fall back to default if the saved device is gone (unplugged).
        host.input_devices()
            .ok()
            .and_then(|mut ds| ds.find(|d| d.name().map(|n| n == device_name).unwrap_or(false)))
            .or_else(|| host.default_input_device())
    }
    .ok_or_else(|| anyhow!("no input device — check microphone privacy settings"))?;
    let config = device.default_input_config()?;
    let channels = config.channels() as usize;
    let rate = config.sample_rate().0;
    meta.0.store(rate, Ordering::Relaxed);
    meta.1.store(channels, Ordering::Relaxed);

    let err_fn = |e| eprintln!("audio stream error: {e}");

    let stream = match config.sample_format() {
        cpal::SampleFormat::F32 => {
            let buf = buffer.clone();
            device.build_input_stream(
                &config.into(),
                move |data: &[f32], _| buf.lock().unwrap().extend_from_slice(data),
                err_fn,
                None,
            )?
        }
        cpal::SampleFormat::I16 => {
            let buf = buffer.clone();
            device.build_input_stream(
                &config.into(),
                move |data: &[i16], _| {
                    buf.lock()
                        .unwrap()
                        .extend(data.iter().map(|s| *s as f32 / i16::MAX as f32));
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

    let samples = buffer.lock().unwrap().clone();
    let mono = downmix_to_mono(&samples, channels);
    Ok(resample_linear(&mono, rate, TARGET_RATE))
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
