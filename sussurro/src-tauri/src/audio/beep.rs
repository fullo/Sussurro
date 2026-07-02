use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::time::Duration;

/// Fire-and-forget sine beep on the default output device. Any failure is
/// silent — audio feedback must never break the dictation pipeline.
pub fn beep(freq: f32, dur_ms: u64) {
    std::thread::spawn(move || {
        let Some(device) = cpal::default_host().default_output_device() else {
            return;
        };
        let Ok(config) = device.default_output_config() else {
            return;
        };
        if config.sample_format() != cpal::SampleFormat::F32 {
            return;
        }
        let rate = config.sample_rate().0 as f32;
        let channels = config.channels() as usize;
        let total = (rate * dur_ms as f32 / 1000.0) as f32;
        let mut n = 0f32;

        let stream = device.build_output_stream(
            &config.into(),
            move |data: &mut [f32], _| {
                for frame in data.chunks_mut(channels) {
                    // Linear fade-in/out over 15% of the clip avoids clicks.
                    let progress = (n / total).min(1.0);
                    let envelope = (progress / 0.15).min(1.0) * ((1.0 - progress) / 0.15).min(1.0);
                    let sample =
                        (n * freq * std::f32::consts::TAU / rate).sin() * 0.12 * envelope;
                    for s in frame.iter_mut() {
                        *s = sample;
                    }
                    n += 1.0;
                }
            },
            |e| eprintln!("beep stream error: {e}"),
            None,
        );
        if let Ok(stream) = stream {
            let _ = stream.play();
            std::thread::sleep(Duration::from_millis(dur_ms + 60));
        }
    });
}

/// Rising tone: recording started.
pub fn record_start() {
    beep(880.0, 110);
}

/// Falling tone: recording finished, processing.
pub fn record_stop() {
    beep(520.0, 110);
}
