use anyhow::{anyhow, Context, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};

use crate::audio::resample::{downmix_to_mono, resample_linear};

pub const TARGET_RATE: u32 = 16_000;

/// Owns no audio resources directly — the cpal Stream is !Send, so each
/// recording runs on its own thread and hands samples back over a channel.
#[derive(Default)]
pub struct Recorder {
    stop_tx: Option<Sender<()>>,
    result_rx: Option<Receiver<Result<Vec<f32>>>>,
}

impl Recorder {
    pub fn is_recording(&self) -> bool {
        self.stop_tx.is_some()
    }

    pub fn start(&mut self) -> Result<()> {
        if self.is_recording() {
            return Ok(());
        }
        let (stop_tx, stop_rx) = channel();
        let (result_tx, result_rx) = channel();
        std::thread::spawn(move || {
            let _ = result_tx.send(record_until_stopped(stop_rx));
        });
        self.stop_tx = Some(stop_tx);
        self.result_rx = Some(result_rx);
        Ok(())
    }

    /// Returns 16 kHz mono f32 samples.
    pub fn stop(&mut self) -> Result<Vec<f32>> {
        let stop_tx = self.stop_tx.take().ok_or_else(|| anyhow!("not recording"))?;
        let result_rx = self.result_rx.take().ok_or_else(|| anyhow!("not recording"))?;
        let _ = stop_tx.send(());
        result_rx.recv().context("recording thread died")?
    }
}

fn record_until_stopped(stop_rx: Receiver<()>) -> Result<Vec<f32>> {
    let host = cpal::default_host();
    let device = host
        .default_input_device()
        .ok_or_else(|| anyhow!("no input device — check microphone privacy settings"))?;
    let config = device.default_input_config()?;
    let channels = config.channels() as usize;
    let rate = config.sample_rate().0;

    let buffer = Arc::new(Mutex::new(Vec::<f32>::new()));
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

    /// Needs a real microphone. Run manually: cargo test record_one_second -- --ignored --nocapture
    #[test]
    #[ignore]
    fn record_one_second() {
        let mut r = Recorder::default();
        r.start().unwrap();
        assert!(r.is_recording());
        std::thread::sleep(std::time::Duration::from_secs(1));
        let samples = r.stop().unwrap();
        println!("captured {} samples at 16 kHz", samples.len());
        // ~1 s of 16 kHz audio, generous tolerance for stream startup latency
        assert!(samples.len() > 8_000, "expected >8000 samples, got {}", samples.len());
    }
}
