/// Average interleaved frames down to one channel.
pub fn downmix_to_mono(samples: &[f32], channels: usize) -> Vec<f32> {
    if channels <= 1 {
        return samples.to_vec();
    }
    samples
        .chunks(channels)
        .map(|frame| frame.iter().sum::<f32>() / frame.len() as f32)
        .collect()
}

/// Linear-interpolation resampler. Whisper tolerates this for speech;
/// swap for rubato if accuracy on noisy input becomes a problem.
pub fn resample_linear(input: &[f32], from_rate: u32, to_rate: u32) -> Vec<f32> {
    if from_rate == to_rate || input.is_empty() {
        return input.to_vec();
    }
    let ratio = from_rate as f64 / to_rate as f64;
    let out_len = (input.len() as f64 / ratio).floor() as usize;
    (0..out_len)
        .map(|i| {
            let pos = i as f64 * ratio;
            let idx = pos as usize;
            let frac = (pos - idx as f64) as f32;
            let a = input[idx];
            let b = *input.get(idx + 1).unwrap_or(&a);
            a + (b - a) * frac
        })
        .collect()
}

/// Root-mean-square amplitude of the clip; 0.0 for empty input.
pub fn rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    (samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32).sqrt()
}

/// True when the clip's RMS energy is below `threshold` (~0.01 for typical
/// mics): nothing worth transcribing. Whisper hallucinates text ("you",
/// "thank you") on silence, so we gate it out before inference.
pub fn is_mostly_silence(samples: &[f32], threshold: f32) -> bool {
    if samples.is_empty() {
        return true;
    }
    let rms = (samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32).sqrt();
    rms < threshold
}

/// Amplify quiet speech (whisper mode). Clamps to [-1, 1] to avoid clipping
/// artifacts turning into garbage transcripts.
pub fn boost_gain(samples: &mut [f32], gain: f32) {
    for s in samples.iter_mut() {
        *s = (*s * gain).clamp(-1.0, 1.0);
    }
}

/// VAD-lite: cut leading/trailing silence by RMS windows, keeping `pad`
/// samples of context around the speech. Falls back to the input when no
/// window clears the threshold (the silence gate handles that case).
pub fn trim_silence(samples: &[f32], threshold: f32, window: usize, pad: usize) -> Vec<f32> {
    if samples.is_empty() || window == 0 {
        return samples.to_vec();
    }
    let rms = |w: &[f32]| (w.iter().map(|s| s * s).sum::<f32>() / w.len() as f32).sqrt();
    let Some(first) = samples.chunks(window).position(|w| rms(w) >= threshold) else {
        return samples.to_vec();
    };
    let last = samples
        .chunks(window)
        .rposition(|w| rms(w) >= threshold)
        .unwrap_or(first);
    let start = (first * window).saturating_sub(pad);
    let end = ((last + 1) * window + pad).min(samples.len());
    samples[start..end].to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn boost_amplifies_and_clamps() {
        let mut s = vec![0.1, -0.5, 0.0];
        boost_gain(&mut s, 3.0);
        assert!((s[0] - 0.3).abs() < 1e-6);
        assert_eq!(s[1], -1.0); // clamped
        assert_eq!(s[2], 0.0);
    }

    #[test]
    fn trim_cuts_leading_and_trailing_silence() {
        // 1s silence + 1s tone + 1s silence, window 0.1s, pad 0.2s
        let mut samples = vec![0.0f32; 16_000];
        samples.extend((0..16_000).map(|i| if (i / 18) % 2 == 0 { 0.2 } else { -0.2 }));
        samples.extend(vec![0.0f32; 16_000]);
        let trimmed = trim_silence(&samples, 0.01, 1_600, 3_200);
        // tone (16000) + up to 2*pad (6400): far shorter than the 48000 input
        assert!(trimmed.len() <= 16_000 + 6_400 + 1_600);
        assert!(trimmed.len() >= 16_000);
    }

    #[test]
    fn all_silence_is_returned_unchanged() {
        let samples = vec![0.0f32; 8_000];
        assert_eq!(trim_silence(&samples, 0.01, 1_600, 3_200).len(), 8_000);
    }

    #[test]
    fn rms_of_empty_and_silence_is_zero() {
        assert_eq!(rms(&[]), 0.0);
        assert_eq!(rms(&vec![0.0; 1_000]), 0.0);
    }

    #[test]
    fn rms_of_constant_signal_is_its_amplitude() {
        let signal: Vec<f32> = (0..1_000).map(|i| if i % 2 == 0 { 0.3 } else { -0.3 }).collect();
        // f32 accumulation over 1000 samples: tolerance well above epsilon
        assert!((rms(&signal) - 0.3).abs() < 1e-4);
    }

    #[test]
    fn silence_is_detected() {
        assert!(is_mostly_silence(&[], 0.01));
        assert!(is_mostly_silence(&vec![0.0; 16_000], 0.01));
        let faint_noise: Vec<f32> = (0..16_000).map(|i| if i % 2 == 0 { 0.002 } else { -0.002 }).collect();
        assert!(is_mostly_silence(&faint_noise, 0.01));
    }

    #[test]
    fn speech_level_signal_is_not_silence() {
        // A 440 Hz-ish square wave at 0.1 amplitude — loud enough to matter.
        let signal: Vec<f32> = (0..16_000).map(|i| if (i / 18) % 2 == 0 { 0.1 } else { -0.1 }).collect();
        assert!(!is_mostly_silence(&signal, 0.01));
    }

    #[test]
    fn mono_passthrough() {
        assert_eq!(downmix_to_mono(&[0.1, 0.2, 0.3], 1), vec![0.1, 0.2, 0.3]);
    }

    #[test]
    fn stereo_averages_frames() {
        let out = downmix_to_mono(&[1.0, 0.0, 0.0, 1.0], 2);
        assert_eq!(out, vec![0.5, 0.5]);
    }

    #[test]
    fn same_rate_passthrough() {
        let input = vec![0.5f32; 100];
        assert_eq!(resample_linear(&input, 16_000, 16_000), input);
    }

    #[test]
    fn halves_length_from_32k_to_16k() {
        let input = vec![0.25f32; 32_000];
        let out = resample_linear(&input, 32_000, 16_000);
        assert_eq!(out.len(), 16_000);
        assert!(out.iter().all(|&s| (s - 0.25).abs() < 1e-6));
    }

    #[test]
    fn empty_input_is_fine() {
        assert!(resample_linear(&[], 48_000, 16_000).is_empty());
        assert!(downmix_to_mono(&[], 2).is_empty());
    }
}
