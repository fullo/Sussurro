//! Keep Parakeet from dropping speech after a pause (#194).
//!
//! Parakeet TDT v3 through transcribe-rs 0.3.11 can stop emitting text for
//! the rest of a buffer once a sentence ends and a pause follows: after the
//! sentence-final token the decoder's prediction-network state makes the
//! joint predict blank on every later frame, although the encoder output
//! still carries the speech (decoding the same encoder frames with a fresh
//! decoder state recovers the lost sentences). transcribe-rs exposes no
//! decoding parameter for this (its greedy loop, state handling and
//! `MAX_TOKENS_PER_STEP` are private), so the Parakeet transcriber works
//! around it on the audio, with two measures computed here:
//! - [`Energy::split`]: long input is cut at pauses into pieces of about
//!   `soft_max_ms`, each decoded with a fresh decoder state;
//! - [`Energy::resume_at`]: when a piece's transcript ends while the rest
//!   of the piece still holds speech, that rest is decoded again on its own
//!   (the drop also happens inside short pieces, after pauses of any
//!   length — see the PR for the measurements).
//!
//! Pure, energy-based and relative to the buffer's own level, so it works
//! on any input (dictation after the whisper-mode gain boost, engine
//! segments, files) without a VAD model. Whisper never goes through here.

use std::ops::Range;

/// Analysis frame: 512 samples (32 ms at 16 kHz), the engine's VAD frame.
pub const FRAME: usize = 512;
pub const RATE: usize = 16_000;

/// How Parakeet input is cut and checked. All lengths are in ms.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PauseSplit {
    /// A piece is closed at the first pause that starts after it holds
    /// this much audio.
    pub soft_max_ms: u32,
    /// Quiet this long is a pause (sentence breaks, not gaps between words).
    pub min_pause_ms: u32,
    /// A piece that reaches this length without a pause is cut at its
    /// quietest frame within the last `cap_search_ms`.
    pub hard_max_ms: u32,
    pub cap_search_ms: u32,
    /// Audio after the transcript's last word that still belongs to it
    /// (the word's tail, a breath) before [`Energy::resume_at`] looks.
    pub tail_grace_ms: u32,
    /// Speech (loud frames) after that which means the model stopped
    /// early and the rest must be decoded again. 0 turns the check off.
    pub tail_speech_ms: u32,
}

/// Parakeet's split: pieces of about 8 s cut in the middle of a pause,
/// and a re-decode when ≥ 1 s of speech follows the last word (the #194
/// benchmark: see the PR for the table).
pub const PARAKEET: PauseSplit = PauseSplit {
    soft_max_ms: 8_000,
    min_pause_ms: 300,
    hard_max_ms: 20_000,
    cap_search_ms: 5_000,
    tail_grace_ms: 500,
    tail_speech_ms: 1_000,
};

fn frames(ms: u32) -> usize {
    (ms as usize * RATE / 1000).div_ceil(FRAME).max(1)
}

/// Frame RMS below this is quiet, relative to the buffer itself: 20 dB
/// under its speech level (90th-percentile frame), raised to 6 dB over its
/// noise floor (2nd-percentile frame) in a noisy room, but never above
/// half the speech level. Pauses can be a few percent of a dictation, so
/// the floor is a very low percentile — low enough to land in one pause,
/// high enough to skip a few digital-zero frames at the start of a
/// recording. `None` for a buffer without any energy.
fn quiet_threshold(rms: &[f32]) -> Option<f32> {
    if rms.is_empty() {
        return None;
    }
    let mut sorted = rms.to_vec();
    sorted.sort_by(f32::total_cmp);
    let n = sorted.len();
    let floor = sorted[n / 50];
    let speech = sorted[(n * 9 / 10).min(n - 1)];
    (speech > 0.0).then(|| (0.1 * speech).max(2.0 * floor).min(0.5 * speech))
}

/// Per-frame energy of one buffer (16 kHz mono) and its quiet threshold.
pub struct Energy {
    rms: Vec<f32>,
    quiet: Option<f32>,
    len: usize,
}

impl Energy {
    pub fn of(samples: &[f32]) -> Self {
        let rms: Vec<f32> = samples
            .chunks(FRAME)
            .map(crate::audio::resample::rms)
            .collect();
        let quiet = quiet_threshold(&rms);
        Self {
            rms,
            quiet,
            len: samples.len(),
        }
    }

    fn is_quiet(&self, frame: usize, thr: f32) -> bool {
        self.rms[frame] < thr
    }

    /// Cut the buffer into consecutive pieces at pauses, per `p`. The ranges
    /// tile it: nothing is dropped or repeated, and a buffer without a
    /// qualifying pause (or no longer than `soft_max_ms`) comes back as one
    /// range. Cuts fall on frame boundaries in the middle of a pause (or at
    /// the quietest frame when `hard_max_ms` forces one).
    pub fn split(&self, p: &PauseSplit) -> Vec<Range<usize>> {
        let whole = vec![0..self.len];
        let n = self.rms.len();
        let (soft, pause) = (frames(p.soft_max_ms), frames(p.min_pause_ms));
        let (Some(thr), true) = (self.quiet, n > soft) else {
            return whole;
        };
        let (hard, search) = (frames(p.hard_max_ms).max(soft + 1), frames(p.cap_search_ms));

        let mut cuts: Vec<usize> = Vec::new();
        let mut start = 0usize;
        let mut run: Option<usize> = None;
        for i in 0..n {
            if self.is_quiet(i, thr) {
                run.get_or_insert(i);
            } else if let Some(a) = run.take() {
                // The quiet run [a, i) ended with speech at frame i.
                if a >= start + soft && i - a >= pause {
                    let cut = (a + i) / 2;
                    cuts.push(cut);
                    start = cut;
                }
            }
            if i + 1 - start >= hard && i + 1 < n {
                // No pause: cut after the quietest recent frame (latest on ties).
                let from = (i + 1).saturating_sub(search).max(start + 1);
                let q = (from..=i)
                    .rev()
                    .min_by(|a, b| self.rms[*a].total_cmp(&self.rms[*b]))
                    .unwrap_or(i);
                let cut = q + 1;
                cuts.push(cut);
                start = cut;
                run = run.map(|a| a.max(cut)).filter(|a| *a <= i);
            }
        }

        let mut out = Vec::with_capacity(cuts.len() + 1);
        let mut from = 0usize;
        for c in cuts {
            let at = (c * FRAME).min(self.len);
            if at > from && at < self.len {
                out.push(from..at);
                from = at;
            }
        }
        out.push(from..self.len);
        out
    }

    /// A piece ending at sample `end` was transcribed up to sample
    /// `heard_to` (the end of its last word). If what follows, past
    /// `tail_grace_ms`, still holds at least `tail_speech_ms` of speech, the
    /// model stopped early: returns where to decode again — shortly before
    /// that speech, never before `heard_to`. `None` when the rest is quiet
    /// (or the check is off).
    pub fn resume_at(&self, heard_to: usize, end: usize, p: &PauseSplit) -> Option<usize> {
        let thr = self.quiet?;
        if p.tail_speech_ms == 0 {
            return None;
        }
        let end = end.min(self.len);
        let first = (heard_to + frames(p.tail_grace_ms) * FRAME).div_ceil(FRAME);
        let last = end / FRAME; // whole frames only
        if first >= last {
            return None;
        }
        let loud: Vec<usize> = (first..last).filter(|f| !self.is_quiet(*f, thr)).collect();
        if loud.len() < frames(p.tail_speech_ms) {
            return None;
        }
        // Resume a little before the speech (pre-roll), inside the pause.
        let at = (loud[0].saturating_sub(frames(200)) * FRAME).max(heard_to);
        (at < end).then_some(at)
    }
}

/// [`Energy::split`] of `samples`.
pub fn split_ranges(samples: &[f32], p: &PauseSplit) -> Vec<Range<usize>> {
    Energy::of(samples).split(p)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Synthetic "speech": a loud square wave, and a quiet noise floor.
    fn speech(ms: usize) -> Vec<f32> {
        (0..ms * 16).map(|i| if (i / 20) % 2 == 0 { 0.2 } else { -0.2 }).collect()
    }
    fn quiet(ms: usize) -> Vec<f32> {
        (0..ms * 16).map(|i| if i % 2 == 0 { 3e-4 } else { -3e-4 }).collect()
    }
    fn build(parts: &[(bool, usize)]) -> Vec<f32> {
        parts
            .iter()
            .flat_map(|&(loud, ms)| if loud { speech(ms) } else { quiet(ms) })
            .collect()
    }
    fn assert_tiles(r: &[Range<usize>], len: usize) {
        assert_eq!(r.first().unwrap().start, 0);
        assert_eq!(r.last().unwrap().end, len);
        for w in r.windows(2) {
            assert_eq!(w[0].end, w[1].start);
        }
        assert!(r.iter().all(|x| x.start < x.end));
    }
    fn secs(r: &Range<usize>) -> (f32, f32) {
        (r.start as f32 / 16_000.0, r.end as f32 / 16_000.0)
    }
    fn at(s: f32) -> usize {
        (s * 16_000.0) as usize
    }

    #[test]
    fn short_input_is_never_split() {
        let a = build(&[(true, 3_000), (false, 1_500), (true, 3_000)]);
        assert_eq!(split_ranges(&a, &PARAKEET), vec![0..a.len()]);
        assert_eq!(split_ranges(&[], &PARAKEET), vec![0..0]);
    }

    #[test]
    fn cuts_in_the_first_pause_after_the_soft_length() {
        // Sentences at 0–5 s and 5.4–9.4 s (a 0.4 s pause before 8 s: kept),
        // then a 1 s pause at 9.4 s: the cut, in its middle.
        let a = build(&[
            (true, 5_000),
            (false, 400),
            (true, 4_000),
            (false, 1_000),
            (true, 6_000),
        ]);
        let r = split_ranges(&a, &PARAKEET);
        assert_tiles(&r, a.len());
        assert_eq!(r.len(), 2, "{r:?}");
        let (_, cut) = secs(&r[0]);
        assert!((9.7..10.1).contains(&cut), "cut at {cut} s");
    }

    #[test]
    fn gaps_shorter_than_a_pause_do_not_split() {
        // 18 s of speech with 150 ms gaps between "words".
        let mut parts = Vec::new();
        for _ in 0..12 {
            parts.push((true, 1_350));
            parts.push((false, 150));
        }
        let a = build(&parts);
        assert_eq!(split_ranges(&a, &PARAKEET).len(), 1);
    }

    #[test]
    fn every_later_pause_starts_a_new_piece_after_the_soft_length() {
        // Four 9 s sentences separated by 0.5 s pauses → four pieces.
        let a = build(&[
            (true, 9_000),
            (false, 500),
            (true, 9_000),
            (false, 500),
            (true, 9_000),
            (false, 500),
            (true, 9_000),
        ]);
        let r = split_ranges(&a, &PARAKEET);
        assert_tiles(&r, a.len());
        assert_eq!(r.len(), 4, "{r:?}");
        for (k, x) in r.iter().enumerate().skip(1) {
            // Each cut lies inside the k-th pause (9 s + 0.5 s per sentence).
            let pause_start = k as f32 * 9.5 - 0.5;
            let (s, _) = secs(x);
            assert!(s > pause_start && s < pause_start + 0.5, "cut {k} at {s}");
        }
    }

    #[test]
    fn trailing_silence_stays_with_the_last_piece() {
        let a = build(&[(true, 12_000), (false, 3_000)]);
        assert_eq!(split_ranges(&a, &PARAKEET), vec![0..a.len()]);
    }

    #[test]
    fn long_speech_without_pauses_is_capped_at_the_quietest_frame() {
        // 45 s of speech with one short dip (a 100 ms gap) at 18 s.
        let a = build(&[(true, 18_000), (false, 100), (true, 27_000)]);
        let r = split_ranges(&a, &PARAKEET);
        assert_tiles(&r, a.len());
        assert!(r.len() >= 2, "{r:?}");
        let (_, first_cut) = secs(&r[0]);
        assert!((18.0..18.2).contains(&first_cut), "first cut at {first_cut} s");
        let max = frames(PARAKEET.hard_max_ms) * FRAME;
        assert!(r.iter().all(|x| x.len() <= max), "{r:?}");
    }

    #[test]
    fn a_louder_noise_floor_still_finds_the_pause() {
        // Background noise at 0.02 RMS (above the dictation gate's 0.01)
        // and speech at 0.2: the threshold follows the buffer.
        let noise = |ms: usize| -> Vec<f32> {
            (0..ms * 16).map(|i| if i % 2 == 0 { 0.02 } else { -0.02 }).collect()
        };
        let mut a = speech(9_000);
        a.extend(noise(800));
        a.extend(speech(5_000));
        let r = split_ranges(&a, &PARAKEET);
        assert_eq!(r.len(), 2, "{r:?}");
    }

    #[test]
    fn digital_silence_is_one_piece() {
        let a = vec![0.0; 16_000 * 30];
        assert_eq!(split_ranges(&a, &PARAKEET), vec![0..a.len()]);
        assert_eq!(Energy::of(&a).resume_at(0, a.len(), &PARAKEET), None);
    }

    #[test]
    fn speech_left_after_the_last_word_is_decoded_again() {
        // Sentence 0–3 s, pause 3–3.5 s, sentence 3.5–7 s; the transcript
        // stopped after the first one (last word ends at 3.0 s).
        let a = build(&[(true, 3_000), (false, 500), (true, 3_500), (false, 300)]);
        let e = Energy::of(&a);
        let resume = e.resume_at(at(3.0), a.len(), &PARAKEET).expect("resume");
        // Just before the second sentence, inside the pause.
        assert!(resume >= at(3.0) && resume < at(3.5), "resume at {resume}");
        // Heard up to the end of the second sentence: nothing left.
        assert_eq!(e.resume_at(at(7.0), a.len(), &PARAKEET), None);
        // A word's tail within the grace period is not a lost sentence.
        assert_eq!(e.resume_at(at(6.6), a.len(), &PARAKEET), None);
        // Less than tail_speech_ms of sound left (a 0.6 s word): no retry.
        let b = build(&[(true, 3_000), (false, 800), (true, 600), (false, 300)]);
        assert_eq!(Energy::of(&b).resume_at(at(3.0), b.len(), &PARAKEET), None);
        // The check can be turned off.
        let off = PauseSplit {
            tail_speech_ms: 0,
            ..PARAKEET
        };
        assert_eq!(e.resume_at(at(3.0), a.len(), &off), None);
    }

    #[test]
    fn resume_looks_only_inside_the_piece() {
        // Two pieces: 0–10 s and 10–20 s. The first was fully heard; the
        // speech of the second piece must not trigger a retry of the first.
        let a = build(&[(true, 9_500), (false, 1_000), (true, 9_500)]);
        let e = Energy::of(&a);
        let r = e.split(&PARAKEET);
        assert_eq!(r.len(), 2);
        assert_eq!(e.resume_at(at(9.5), r[0].end, &PARAKEET), None);
        // Nothing heard in the second piece: resume at its speech.
        let resume = e.resume_at(r[1].start, r[1].end, &PARAKEET);
        assert!(resume.is_some());
    }
}
