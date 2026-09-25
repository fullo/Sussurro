//! Overlapping speech (0.11, #244; spike #237, plan E19 as reshaped by
//! it): where two people talk at once inside one line, and who the second
//! one is. Pure — the model itself is in [`super::segmentation`].
//!
//! **Detection** runs when a line is labelled (the [`super::Tracker`]: end
//! of a run with speakers, "Identify voices"), on the line's own audio cut
//! into ≤ 10 s pieces, each zero-padded to the model's 10 s window. The
//! model (pyannote segmentation-3.0) gives, per 16.875 ms frame, a
//! powerset distribution over {none, S1, S2, S3, S1+S2, S1+S3, S2+S3};
//! P(overlap) is the sum of the three pairs. Frames at or above
//! [`OVERLAP_THRESHOLD`] are overlap. A line **counts as overlapped** when
//! they cover at least [`MIN_OVERLAP_MS`] and [`MIN_OVERLAP_SHARE`] of it
//! (line precision 0.95, recall 0.67–0.72 on AMI in the spike); only then
//! are its spans stored (`Segment.overlap`), so a stored span always means
//! an overlapped line.
//!
//! **Second speaker.** "Re-detect" has no audio, so the spans are kept and
//! every span gets a second speaker from the lines around it: the nearest
//! line of the same channel whose speaker differs from the line's own
//! ([`assign_second_speakers`]). In the spike this took DER from 17.7 % to
//! 13.4 % and halved missed speech in overlap; mapping the model's local
//! speakers to voices did worse. Overlapped lines stay in the clustering
//! (leaving them out gained nothing and lost a quiet speaker) — they are
//! left out only of voice profiles (#241) and, where a voice has other
//! lines, of the voice's mean used to match it (suggestions, "You").
//!
//! Thresholds come from English AMI meetings (in-domain for the model):
//! re-check them on Italian speech and laptop mics before calling them
//! final.

use crate::archive::{OverlapSpan, Segment, SegmentsFile};
use anyhow::{bail, Result};

/// Sample rate of the model's input.
pub const RATE: u64 = 16_000;
/// The model's window: 10 s of 16 kHz audio.
pub const WINDOW_SAMPLES: usize = 160_000;
/// Frame step and receptive field of the model, in samples (a 10 s window
/// gives 589 frames).
pub const FRAME_STEP: usize = 270;
pub const RECEPTIVE_FIELD: usize = 991;
/// Powerset classes: none, S1, S2, S3, S1+S2, S1+S3, S2+S3.
pub const CLASSES: usize = 7;
/// The classes with two speakers.
const PAIRS: [usize; 3] = [4, 5, 6];
/// P(overlap) a frame needs to count as overlap (spike: frame precision
/// 0.87, recall 0.62 at 0.5; 0.3–0.4 if spans are ever used frame-wise).
pub const OVERLAP_THRESHOLD: f32 = 0.5;
/// A line is overlapped with at least this much overlap…
pub const MIN_OVERLAP_MS: u64 = 300;
/// …making up at least this share of it.
pub const MIN_OVERLAP_SHARE: f64 = 0.10;
/// Stored spans closer than this are joined (a flicker of the threshold
/// is not two overlaps).
pub const JOIN_GAP_MS: u64 = 200;
/// A line further than this from the span gives no second speaker (not
/// measured in the spike: a guard so a lone false alarm in a monologue is
/// not pinned on someone minutes away).
pub const MAX_SECOND_SPEAKER_GAP_MS: u64 = 60_000;
/// Lines shorter than this are not checked (same minimum as a voice).
pub const MIN_LINE_MS: u64 = super::MIN_EMBED_MS;
/// Windows run through the model at once.
pub const BATCH: usize = 8;

/// P(overlap) per frame of 10 s windows of 16 kHz mono audio. The app
/// uses [`super::segmentation::Segmentation`]; tests pass fakes.
pub trait OverlapModel: Send {
    /// One vector per window (each exactly [`WINDOW_SAMPLES`] long, zero
    /// padded), with P(overlap) for each of the model's frames.
    fn frame_overlap(&mut self, windows: &[Vec<f32>]) -> Result<Vec<Vec<f32>>>;
}

/// Loads the model the first time a line needs it (it may have to be
/// downloaded).
pub type OverlapLoader = Box<dyn FnOnce() -> Result<Box<dyn OverlapModel>> + Send>;

/// P(overlap) of one frame from the model's 7 outputs (log-probabilities;
/// normalised here, so plain logits work too).
pub fn powerset_overlap(frame: &[f32]) -> f32 {
    if frame.len() != CLASSES || !frame.iter().all(|x| x.is_finite()) {
        return 0.0;
    }
    let max = frame.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    let exp: Vec<f32> = frame.iter().map(|x| (x - max).exp()).collect();
    let total: f32 = exp.iter().sum();
    if total <= 0.0 {
        return 0.0;
    }
    PAIRS.iter().map(|&k| exp[k]).sum::<f32>() / total
}

/// P(overlap) per frame from a `[frames, 7]` block of model output.
pub fn decode_frames(logits: &[f32]) -> Vec<f32> {
    logits.chunks_exact(CLASSES).map(powerset_overlap).collect()
}

/// The stretch of a window frame `i` stands for, in samples: the step
/// around the centre of its receptive field (the first frame reaches back
/// to the window start).
fn frame_range(i: usize) -> (usize, usize) {
    let centre = i * FRAME_STEP + RECEPTIVE_FIELD / 2;
    let start = if i == 0 { 0 } else { centre - FRAME_STEP / 2 };
    (start, centre + FRAME_STEP.div_ceil(2))
}

/// Cut a line into the model's windows: equal ≤ 10 s pieces (the
/// tracker's cut), each zero-padded to [`WINDOW_SAMPLES`]. Returns the
/// pieces' ranges and the padded windows.
pub fn line_windows(samples: &[f32]) -> (Vec<std::ops::Range<usize>>, Vec<Vec<f32>>) {
    let pieces = super::tracker::windows(samples.len(), WINDOW_SAMPLES, 1);
    let windows = pieces
        .iter()
        .map(|r| {
            let mut w = samples[r.clone()].to_vec();
            w.resize(WINDOW_SAMPLES, 0.0);
            w
        })
        .collect();
    (pieces, windows)
}

/// Stitch the windows' frames back onto the line: the overlap frames as
/// `(start, end)` sample ranges of the line, in order, clipped to each
/// piece (frames over the zero padding are dropped).
pub fn stitch(
    pieces: &[std::ops::Range<usize>],
    probs: &[Vec<f32>],
    threshold: f32,
) -> Vec<(usize, usize)> {
    let mut out: Vec<(usize, usize)> = Vec::new();
    for (piece, frames) in pieces.iter().zip(probs) {
        let len = piece.len();
        for (i, p) in frames.iter().enumerate() {
            let (a, b) = frame_range(i);
            if a >= len {
                break;
            }
            if *p < threshold {
                continue;
            }
            let (a, b) = (piece.start + a, piece.start + b.min(len));
            match out.last_mut() {
                Some(last) if last.1 >= a => last.1 = last.1.max(b),
                _ => out.push((a, b)),
            }
        }
    }
    out
}

fn to_ms(samples: usize) -> u64 {
    samples as u64 * 1000 / RATE
}

/// The overlap spans to store for a line of `line_ms`, from its overlap
/// frames (`stitch`), in ms from the line start: none unless the line
/// counts as overlapped ([`MIN_OVERLAP_MS`], [`MIN_OVERLAP_SHARE`]);
/// spans closer than [`JOIN_GAP_MS`] joined.
pub fn line_spans(frames: &[(usize, usize)], line_ms: u64) -> Vec<(u64, u64)> {
    let total: u64 = frames.iter().map(|(a, b)| to_ms(b - a)).sum();
    if line_ms == 0 || total < MIN_OVERLAP_MS || (total as f64) < MIN_OVERLAP_SHARE * line_ms as f64
    {
        return Vec::new();
    }
    let mut out: Vec<(u64, u64)> = Vec::new();
    for &(a, b) in frames {
        let (a, b) = (to_ms(a), to_ms(b).min(line_ms));
        match out.last_mut() {
            Some(last) if a <= last.1 + JOIN_GAP_MS => last.1 = last.1.max(b),
            _ => out.push((a, b)),
        }
    }
    out
}

/// Run the model on one line (16 kHz mono): its overlap spans in ms from
/// the line start, empty when the line is short or not overlapped.
pub fn detect(model: &mut dyn OverlapModel, samples: &[f32]) -> Result<Vec<(u64, u64)>> {
    let line_ms = to_ms(samples.len());
    if line_ms < MIN_LINE_MS {
        return Ok(Vec::new());
    }
    let (pieces, windows) = line_windows(samples);
    let mut probs: Vec<Vec<f32>> = Vec::with_capacity(windows.len());
    for batch in windows.chunks(BATCH) {
        let got = model.frame_overlap(batch)?;
        if got.len() != batch.len() {
            bail!(
                "overlap model returned {} windows for {}",
                got.len(),
                batch.len()
            );
        }
        probs.extend(got);
    }
    Ok(line_spans(
        &stitch(&pieces, &probs, OVERLAP_THRESHOLD),
        line_ms,
    ))
}

/// Spans relative to a line (`detect`) on the session clock.
pub fn spans_at(line_start_ms: u64, spans: &[(u64, u64)]) -> Vec<OverlapSpan> {
    spans
        .iter()
        .map(|&(a, b)| OverlapSpan {
            start_ms: line_start_ms + a,
            end_ms: line_start_ms + b,
            speaker_id: None,
        })
        .collect()
}

/// Whether a line is overlapped (it carries stored spans).
pub fn is_overlapped(s: &Segment) -> bool {
    !s.overlap.is_empty()
}

/// Time between a span and a line (0 when they meet).
fn gap(span: &OverlapSpan, s: &Segment) -> u64 {
    if s.end_ms <= span.start_ms {
        span.start_ms - s.end_ms
    } else if s.start_ms >= span.end_ms {
        s.start_ms - span.end_ms
    } else {
        0
    }
}

/// Give every overlap span its second speaker: the nearest line of the
/// same channel (within [`MAX_SECOND_SPEAKER_GAP_MS`]) whose speaker is
/// not the line's own; ties go to the earlier line. `None` when there is
/// none. Idempotent, and cheap enough to run after every edit that moves
/// lines between speakers (a moved neighbour changes the answer). Returns
/// how many spans changed.
pub fn assign_second_speakers(file: &mut SegmentsFile) -> usize {
    if file.segments.iter().all(|s| s.overlap.is_empty()) {
        return 0;
    }
    // Lines with a speaker, per channel, by start.
    let mut voiced: Vec<usize> = (0..file.segments.len())
        .filter(|&i| {
            let s = &file.segments[i];
            s.speaker_id.is_some() && s.stt_error.is_none()
        })
        .collect();
    voiced.sort_by_key(|&i| (file.segments[i].start_ms, file.segments[i].id));
    let mut changed = 0;
    for i in 0..file.segments.len() {
        if file.segments[i].overlap.is_empty() {
            continue;
        }
        let (own, channel) = (
            file.segments[i].speaker_id.clone(),
            file.segments[i].channel,
        );
        let answers: Vec<Option<String>> = file.segments[i]
            .overlap
            .iter()
            .map(|span| {
                voiced
                    .iter()
                    .map(|&j| &file.segments[j])
                    .filter(|s| s.channel == channel && s.speaker_id != own)
                    .map(|s| (gap(span, s), s))
                    .filter(|(g, _)| *g <= MAX_SECOND_SPEAKER_GAP_MS)
                    // Stable: the earliest line wins a tie.
                    .min_by_key(|(g, _)| *g)
                    .and_then(|(_, s)| s.speaker_id.clone())
            })
            .collect();
        for (span, want) in file.segments[i].overlap.iter_mut().zip(answers) {
            if span.speaker_id != want {
                span.speaker_id = want;
                changed += 1;
            }
        }
    }
    changed
}

/// The second speakers of a line, each once, in span order.
pub fn second_speakers(s: &Segment) -> Vec<&str> {
    let mut out: Vec<&str> = Vec::new();
    for id in s.overlap.iter().filter_map(|o| o.speaker_id.as_deref()) {
        if Some(id) != s.speaker_id.as_deref() && !out.contains(&id) {
            out.push(id);
        }
    }
    out
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::archive::Channel;

    /// Log-probabilities of a frame putting `p` on overlap (class 4) and
    /// the rest on one speaker (class 1).
    fn frame(p: f32) -> [f32; CLASSES] {
        let eps = 1e-6f32;
        let mut f = [eps.ln(); CLASSES];
        f[4] = p.max(eps).ln();
        f[1] = (1.0 - p).max(eps).ln();
        f
    }

    #[test]
    fn powerset_decoding_sums_the_pairs() {
        // Exactly the model's log-softmax shape.
        let probs = [0.1f32, 0.2, 0.05, 0.05, 0.3, 0.2, 0.1];
        let logp: Vec<f32> = probs.iter().map(|p| p.ln()).collect();
        assert!((powerset_overlap(&logp) - 0.6).abs() < 1e-5);
        // Plain logits are normalised first.
        let logits: Vec<f32> = logp.iter().map(|x| x + 3.0).collect();
        assert!((powerset_overlap(&logits) - 0.6).abs() < 1e-5);
        // Nobody or one speaker: no overlap.
        assert!(powerset_overlap(&frame(0.0)) < 1e-4);
        // Malformed frames count as none.
        assert_eq!(powerset_overlap(&[0.0; 6]), 0.0);
        assert_eq!(powerset_overlap(&[f32::NAN; 7]), 0.0);
        let block: Vec<f32> = [frame(0.9), frame(0.1)].concat();
        let d = decode_frames(&block);
        assert_eq!(d.len(), 2);
        assert!((d[0] - 0.9).abs() < 1e-3 && (d[1] - 0.1).abs() < 1e-3);
    }

    #[test]
    fn a_ten_second_window_has_589_frames_covering_it() {
        let frames = (WINDOW_SAMPLES - RECEPTIVE_FIELD) / FRAME_STEP + 1;
        assert_eq!(frames, 589);
        assert_eq!(frame_range(0).0, 0);
        // Consecutive frames tile the window without gaps.
        for i in 1..frames {
            assert_eq!(frame_range(i).0, frame_range(i - 1).1, "frame {i}");
        }
        // The last frame ends within ~40 ms of the window's end.
        assert!(WINDOW_SAMPLES - frame_range(frames - 1).1 < 1_000);
    }

    #[test]
    fn windows_are_padded_and_stitched_back_onto_the_line() {
        // 14 s: two 7 s pieces, each padded to 10 s.
        let samples = vec![0.1f32; 14 * 16_000];
        let (pieces, windows) = line_windows(&samples);
        assert_eq!(pieces, vec![0..112_000, 112_000..224_000]);
        assert!(windows.iter().all(|w| w.len() == WINDOW_SAMPLES));
        assert_eq!(windows[0][111_999], 0.1);
        assert_eq!(windows[0][112_000], 0.0);
        // Overlap in frames 100..=199 of the first piece and in the
        // padding of the second (ignored), plus frames 0..10 of it.
        let mut first = vec![0.0f32; 589];
        first[100..200].iter_mut().for_each(|p| *p = 0.8);
        let mut second = vec![0.0f32; 589];
        second[..10].iter_mut().for_each(|p| *p = 0.7);
        second[500..].iter_mut().for_each(|p| *p = 0.9);
        let got = stitch(&pieces, &[first, second], 0.5);
        assert_eq!(
            got,
            vec![
                (frame_range(100).0, frame_range(199).1),
                (112_000, 112_000 + frame_range(9).1),
            ]
        );
        // Below the threshold: nothing.
        assert!(stitch(&pieces, &[vec![0.4; 589], vec![0.49; 589]], 0.5).is_empty());
    }

    #[test]
    fn a_line_needs_enough_overlap_to_keep_spans() {
        let ms = |a: u64, b: u64| ((a * 16) as usize, (b * 16) as usize);
        // 200 ms in a 10 s line: too little.
        assert!(line_spans(&[ms(1000, 1200)], 10_000).is_empty());
        // 400 ms in a 10 s line: over 300 ms but under 10 %.
        assert!(line_spans(&[ms(1000, 1400)], 10_000).is_empty());
        // 400 ms in a 3 s line: kept.
        assert_eq!(line_spans(&[ms(1000, 1400)], 3_000), vec![(1000, 1400)]);
        // Close spans join, far ones stay apart; clipped to the line.
        assert_eq!(
            line_spans(
                &[ms(0, 300), ms(450, 700), ms(2000, 2600), ms(2700, 3100)],
                3_000
            ),
            vec![(0, 700), (2000, 3000)]
        );
    }

    /// Fake model: overlap wherever the window's samples are loud (> 0.5).
    pub(crate) struct LoudIsOverlap;

    impl OverlapModel for LoudIsOverlap {
        fn frame_overlap(&mut self, windows: &[Vec<f32>]) -> Result<Vec<Vec<f32>>> {
            Ok(windows
                .iter()
                .map(|w| {
                    (0..589)
                        .map(|i| {
                            let c = (i * FRAME_STEP + RECEPTIVE_FIELD / 2).min(w.len() - 1);
                            if w[c].abs() > 0.5 {
                                0.9
                            } else {
                                0.0
                            }
                        })
                        .collect()
                })
                .collect())
        }
    }

    #[test]
    fn detect_finds_the_loud_stretch_of_a_line() {
        // 4 s line, loud from 1.5 s to 2.5 s.
        let mut samples = vec![0.1f32; 64_000];
        samples[24_000..40_000].iter_mut().for_each(|x| *x = 0.9);
        let spans = detect(&mut LoudIsOverlap, &samples).unwrap();
        assert_eq!(spans.len(), 1);
        let (a, b) = spans[0];
        assert!(
            a.abs_diff(1500) <= 20 && b.abs_diff(2500) <= 20,
            "{spans:?}"
        );
        // Short lines are not checked; quiet lines have none.
        assert!(detect(&mut LoudIsOverlap, &vec![0.9; 8_000])
            .unwrap()
            .is_empty());
        assert!(detect(&mut LoudIsOverlap, &vec![0.1; 64_000])
            .unwrap()
            .is_empty());
        assert_eq!(
            spans_at(10_000, &spans)[0],
            OverlapSpan {
                start_ms: 10_000 + a,
                end_ms: 10_000 + b,
                speaker_id: None
            }
        );
    }

    fn seg(id: u32, channel: Channel, start: u64, end: u64, speaker: Option<&str>) -> Segment {
        Segment {
            id,
            channel,
            start_ms: start,
            end_ms: end,
            speaker_id: speaker.map(str::to_string),
            text: format!("line {id}"),
            ..Default::default()
        }
    }

    fn span(a: u64, b: u64) -> OverlapSpan {
        OverlapSpan {
            start_ms: a,
            end_ms: b,
            speaker_id: None,
        }
    }

    fn seconds(f: &SegmentsFile, i: usize) -> Vec<Option<&str>> {
        f.segments[i]
            .overlap
            .iter()
            .map(|o| o.speaker_id.as_deref())
            .collect()
    }

    #[test]
    fn the_second_speaker_is_the_nearest_line_with_another_voice() {
        let mic = Channel::Mic;
        let mut f = SegmentsFile {
            segments: vec![
                seg(0, mic, 0, 4_000, Some("voice:2")),
                seg(1, mic, 4_000, 5_000, Some("voice:1")),
                // Overlap at its start (voice 2's line is the nearest
                // other voice) and at its end (voice 3 just after).
                seg(2, mic, 5_000, 12_000, Some("voice:1")),
                seg(3, mic, 12_500, 15_000, Some("voice:3")),
                // Not a candidate: no speaker; another channel.
                seg(4, mic, 11_000, 11_500, None),
                seg(5, Channel::Remote, 11_000, 12_000, Some("voice:4")),
            ],
            ..Default::default()
        };
        f.segments[2].overlap = vec![span(5_000, 5_600), span(11_500, 12_000)];
        assert_eq!(assign_second_speakers(&mut f), 2);
        assert_eq!(seconds(&f, 2), [Some("voice:2"), Some("voice:3")]);
        assert_eq!(second_speakers(&f.segments[2]), ["voice:2", "voice:3"]);
        // Idempotent.
        assert_eq!(assign_second_speakers(&mut f), 0);

        // The neighbour moves to the line's own voice: the answer follows.
        f.segments[3].speaker_id = Some("voice:1".into());
        assert_eq!(assign_second_speakers(&mut f), 1);
        assert_eq!(seconds(&f, 2), [Some("voice:2"), Some("voice:2")]);
        assert_eq!(second_speakers(&f.segments[2]), ["voice:2"]);
    }

    #[test]
    fn ties_go_to_the_earlier_line_and_far_lines_give_none() {
        let mic = Channel::File;
        let mut f = SegmentsFile {
            segments: vec![
                seg(0, mic, 0, 2_000, Some("voice:2")),
                seg(1, mic, 2_000, 6_000, Some("voice:1")),
                seg(2, mic, 6_000, 8_000, Some("voice:3")),
            ],
            ..Default::default()
        };
        // Equally far from both neighbours (1 s each way).
        f.segments[1].overlap = vec![span(3_000, 5_000)];
        assign_second_speakers(&mut f);
        assert_eq!(seconds(&f, 1), [Some("voice:2")]);

        // A monologue: nobody else within a minute.
        let mut solo = SegmentsFile {
            segments: vec![
                seg(0, mic, 0, 5_000, Some("voice:1")),
                seg(1, mic, 100_000, 104_000, Some("voice:2")),
            ],
            ..Default::default()
        };
        solo.segments[0].overlap = vec![span(1_000, 2_000)];
        assert_eq!(assign_second_speakers(&mut solo), 0);
        assert_eq!(seconds(&solo, 0), [None]);
        assert!(second_speakers(&solo.segments[0]).is_empty());
    }

    #[test]
    fn overlap_is_omitted_from_json_when_empty_and_old_files_read() {
        let line = seg(0, Channel::Mic, 0, 1_000, Some("voice:1"));
        let json = serde_json::to_string(&line).unwrap();
        assert!(!json.contains("overlap"), "{json}");
        // A segments.json from before #244.
        let old: Segment = serde_json::from_str(
            r#"{"id":3,"channel":"mic","start_ms":0,"end_ms":900,"text":"ciao"}"#,
        )
        .unwrap();
        assert!(old.overlap.is_empty() && !is_overlapped(&old));
        // Round trip with spans; a span without a second speaker omits it.
        let mut with = line.clone();
        with.overlap = vec![
            span(100, 400),
            OverlapSpan {
                speaker_id: Some("voice:2".into()),
                ..span(500, 900)
            },
        ];
        let json = serde_json::to_string(&with).unwrap();
        assert!(
            json.contains(r#""overlap":[{"start_ms":100,"end_ms":400},"#),
            "{json}"
        );
        let back: Segment = serde_json::from_str(&json).unwrap();
        assert_eq!(back, with);
        assert!(is_overlapped(&back));
    }
}
