//! Lock-free timing history (#101): a fixed ring of the last
//! [`CAPACITY`] samples, written by the dictation thread and the engine
//! workers, read by the Diagnostics panel once a second.
//!
//! Each slot is a seqlock: the writer claims the next index with one
//! `fetch_add`, marks the slot odd (being written), stores the fields and
//! marks it even again with a sequence number derived from the index. A
//! reader keeps a slot only if it saw the same even sequence before and
//! after reading it — a slot being written or already overwritten by a
//! newer sample is simply skipped. No locks, no allocation on the write
//! path: recording a timing costs a handful of atomic stores.
//!
//! A writer claims its slot with a compare-and-swap from even to odd, so
//! two writers never interleave in one slot: if more than [`CAPACITY`]
//! samples are pushed while one writer is still in its slot (not something
//! the app does — one dictation at a time, a few engine workers), the
//! later sample is dropped, never torn.

use serde::Serialize;
use std::sync::atomic::{fence, AtomicU64, Ordering};

/// Samples kept per ring (the "last ~20").
pub const CAPACITY: usize = 20;

/// Fields per sample, see [`Sample::to_words`].
const FIELDS: usize = 8;

/// "Not measured" in the packed form.
const NONE: u64 = u64::MAX;

/// One timed run: a hotkey dictation or a long-form segment. Durations in
/// ms; `None` = that phase did not run (no cleanup, nothing pasted…).
/// Numbers only — never text.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize)]
pub struct Sample {
    /// When it was recorded, Unix ms.
    pub at_ms: u64,
    /// Audio length: the recording (dictation) or the segment.
    pub audio_ms: Option<u64>,
    /// Waiting for the model, loading it included (first use, idle unload).
    pub load_ms: Option<u64>,
    /// Speech-to-text inference.
    pub stt_ms: Option<u64>,
    /// LLM cleanup.
    pub cleanup_ms: Option<u64>,
    /// Typing / pasting the text (or appending it to the output file).
    pub paste_ms: Option<u64>,
    /// Dictation: Finish → Idle. Segment: STT + cleanup.
    pub total_ms: Option<u64>,
    /// The run ended in an error.
    pub failed: bool,
}

fn pack(v: Option<u64>) -> u64 {
    v.map_or(NONE, |v| v.min(NONE - 1))
}

fn unpack(v: u64) -> Option<u64> {
    (v != NONE).then_some(v)
}

impl Sample {
    fn to_words(self) -> [u64; FIELDS] {
        [
            self.at_ms,
            pack(self.audio_ms),
            pack(self.load_ms),
            pack(self.stt_ms),
            pack(self.cleanup_ms),
            pack(self.paste_ms),
            pack(self.total_ms),
            self.failed as u64,
        ]
    }

    fn from_words(w: [u64; FIELDS]) -> Self {
        Self {
            at_ms: w[0],
            audio_ms: unpack(w[1]),
            load_ms: unpack(w[2]),
            stt_ms: unpack(w[3]),
            cleanup_ms: unpack(w[4]),
            paste_ms: unpack(w[5]),
            total_ms: unpack(w[6]),
            failed: w[7] != 0,
        }
    }
}

struct Slot {
    /// `2n + 1` while sample `n` is written, `2n + 2` once it is complete,
    /// 0 when never written.
    seq: AtomicU64,
    words: [AtomicU64; FIELDS],
}

impl Slot {
    const fn new() -> Self {
        Self {
            seq: AtomicU64::new(0),
            words: [const { AtomicU64::new(0) }; FIELDS],
        }
    }
}

/// The ring. `const`-constructible so it can be a plain `static`.
pub struct TimingRing {
    /// Samples ever pushed; the next one goes to `head % CAPACITY`.
    head: AtomicU64,
    slots: [Slot; CAPACITY],
}

impl Default for TimingRing {
    fn default() -> Self {
        Self::new()
    }
}

impl TimingRing {
    pub const fn new() -> Self {
        Self {
            head: AtomicU64::new(0),
            slots: [const { Slot::new() }; CAPACITY],
        }
    }

    /// Record one sample. Lock-free: never waits.
    pub fn push(&self, sample: Sample) {
        let n = self.head.fetch_add(1, Ordering::Relaxed);
        let slot = &self.slots[(n % CAPACITY as u64) as usize];
        // Claim the slot: only one writer between odd and even. A slot
        // still being written by a writer [`CAPACITY`] samples behind
        // means this sample is dropped rather than waiting.
        let cur = slot.seq.load(Ordering::Relaxed);
        if cur % 2 == 1
            || slot
                .seq
                .compare_exchange(cur, 2 * n + 1, Ordering::Relaxed, Ordering::Relaxed)
                .is_err()
        {
            return;
        }
        fence(Ordering::Release);
        for (dst, v) in slot.words.iter().zip(sample.to_words()) {
            dst.store(v, Ordering::Relaxed);
        }
        slot.seq.store(2 * n + 2, Ordering::Release);
    }

    /// The samples still in the ring, oldest first. Samples being written
    /// right now are left out (they show on the next read).
    pub fn samples(&self) -> Vec<Sample> {
        let head = self.head.load(Ordering::Acquire);
        let first = head.saturating_sub(CAPACITY as u64);
        let mut out = Vec::with_capacity((head - first) as usize);
        for n in first..head {
            let slot = &self.slots[(n % CAPACITY as u64) as usize];
            let before = slot.seq.load(Ordering::Acquire);
            if before != 2 * n + 2 {
                continue; // still being written, or already a newer sample
            }
            let mut words = [0u64; FIELDS];
            for (w, src) in words.iter_mut().zip(&slot.words) {
                *w = src.load(Ordering::Relaxed);
            }
            fence(Ordering::Acquire);
            if slot.seq.load(Ordering::Relaxed) == before {
                out.push(Sample::from_words(words));
            }
        }
        out
    }

    /// The newest complete sample.
    pub fn last(&self) -> Option<Sample> {
        self.samples().pop()
    }

    /// Samples pushed since the app started (the ring keeps the last
    /// [`CAPACITY`]).
    pub fn pushed(&self) -> u64 {
        self.head.load(Ordering::Relaxed)
    }
}

/// Median and 90th percentile of one phase.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Stat {
    pub median: f64,
    pub p90: f64,
    /// Samples the figures come from.
    pub count: usize,
}

/// Nearest-rank percentile of sorted values (`p` in 0..=100): the smallest
/// value with at least `p` % of the values at or below it. Pure.
pub fn percentile(sorted: &[f64], p: f64) -> Option<f64> {
    if sorted.is_empty() {
        return None;
    }
    let rank = ((p / 100.0) * sorted.len() as f64).ceil() as usize;
    Some(sorted[rank.clamp(1, sorted.len()) - 1])
}

/// Median + p90 of `values` (any order; NaN dropped). `None` when empty.
pub fn stat(values: impl IntoIterator<Item = f64>) -> Option<Stat> {
    let mut v: Vec<f64> = values.into_iter().filter(|x| !x.is_nan()).collect();
    v.sort_by(|a, b| a.partial_cmp(b).expect("no NaN"));
    Some(Stat {
        median: percentile(&v, 50.0)?,
        p90: percentile(&v, 90.0)?,
        count: v.len(),
    })
}

/// Per-phase figures over a ring's samples. Failed runs count only for
/// the phases they completed.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Summary {
    pub count: usize,
    pub total: Option<Stat>,
    pub load: Option<Stat>,
    pub stt: Option<Stat>,
    pub cleanup: Option<Stat>,
    pub paste: Option<Stat>,
    /// STT time / audio time (0.1 = ten times faster than real time).
    pub realtime_factor: Option<Stat>,
}

/// Summarise samples. Pure.
pub fn summarize(samples: &[Sample]) -> Summary {
    let ms = |f: fn(&Sample) -> Option<u64>| stat(samples.iter().filter_map(f).map(|v| v as f64));
    Summary {
        count: samples.len(),
        total: ms(|s| if s.failed { None } else { s.total_ms }),
        load: ms(|s| s.load_ms),
        stt: ms(|s| s.stt_ms),
        cleanup: ms(|s| s.cleanup_ms),
        paste: ms(|s| s.paste_ms),
        realtime_factor: stat(samples.iter().filter_map(|s| match (s.stt_ms, s.audio_ms) {
            (Some(stt), Some(audio)) if audio > 0 => Some(stt as f64 / audio as f64),
            _ => None,
        })),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(total: u64) -> Sample {
        Sample {
            at_ms: total,
            total_ms: Some(total),
            ..Default::default()
        }
    }

    #[test]
    fn empty_ring_has_no_samples() {
        let r = TimingRing::new();
        assert!(r.samples().is_empty());
        assert_eq!(r.last(), None);
        assert_eq!(summarize(&r.samples()), Summary::default());
    }

    #[test]
    fn ring_keeps_the_last_capacity_samples_oldest_first() {
        let r = TimingRing::new();
        for i in 0..(CAPACITY as u64 + 5) {
            r.push(s(i));
        }
        let got: Vec<u64> = r.samples().iter().map(|s| s.at_ms).collect();
        let want: Vec<u64> = (5..CAPACITY as u64 + 5).collect();
        assert_eq!(got, want);
        assert_eq!(r.last().unwrap().at_ms, CAPACITY as u64 + 4);
        assert_eq!(r.pushed(), CAPACITY as u64 + 5);
    }

    #[test]
    fn optional_phases_round_trip() {
        let r = TimingRing::new();
        let sample = Sample {
            at_ms: 1,
            audio_ms: Some(3_000),
            load_ms: Some(0),
            stt_ms: Some(812),
            cleanup_ms: None,
            paste_ms: Some(40),
            total_ms: Some(900),
            failed: true,
        };
        r.push(sample);
        assert_eq!(r.last(), Some(sample));
    }

    #[test]
    fn concurrent_writers_never_produce_torn_samples() {
        let r = std::sync::Arc::new(TimingRing::new());
        let writers: Vec<_> = (0..4u64)
            .map(|t| {
                let r = r.clone();
                std::thread::spawn(move || {
                    for i in 0..5_000u64 {
                        let v = t * 1_000_000 + i;
                        // Every field carries the same value: a torn read
                        // would mix two.
                        r.push(Sample {
                            at_ms: v,
                            audio_ms: Some(v),
                            load_ms: Some(v),
                            stt_ms: Some(v),
                            cleanup_ms: Some(v),
                            paste_ms: Some(v),
                            total_ms: Some(v),
                            failed: false,
                        });
                    }
                })
            })
            .collect();
        for _ in 0..2_000 {
            for s in r.samples() {
                assert_eq!(Some(s.at_ms), s.stt_ms);
                assert_eq!(s.total_ms, s.audio_ms);
                assert_eq!(s.cleanup_ms, s.paste_ms);
            }
        }
        for w in writers {
            w.join().unwrap();
        }
        // Under this contention a few samples may be dropped, never torn.
        for s in r.samples() {
            assert_eq!(Some(s.at_ms), s.stt_ms);
        }
    }

    #[test]
    fn nearest_rank_percentiles() {
        let v: Vec<f64> = (1..=10).map(f64::from).collect();
        assert_eq!(percentile(&v, 50.0), Some(5.0));
        assert_eq!(percentile(&v, 90.0), Some(9.0));
        assert_eq!(percentile(&v, 100.0), Some(10.0));
        assert_eq!(percentile(&v, 0.0), Some(1.0));
        assert_eq!(percentile(&[7.0], 90.0), Some(7.0));
        assert_eq!(percentile(&[], 50.0), None);
    }

    #[test]
    fn stat_sorts_its_input() {
        let st = stat([900.0, 100.0, 500.0, 300.0, 700.0]).unwrap();
        assert_eq!((st.median, st.p90, st.count), (500.0, 900.0, 5));
    }

    #[test]
    fn summary_skips_phases_that_did_not_run_and_failed_totals() {
        let samples = [
            Sample {
                audio_ms: Some(2_000),
                stt_ms: Some(200),
                cleanup_ms: Some(400),
                total_ms: Some(700),
                ..Default::default()
            },
            Sample {
                audio_ms: Some(4_000),
                stt_ms: Some(800),
                cleanup_ms: None,
                total_ms: Some(900),
                ..Default::default()
            },
            Sample {
                stt_ms: Some(100),
                total_ms: Some(50_000),
                failed: true,
                ..Default::default()
            },
        ];
        let sum = summarize(&samples);
        assert_eq!(sum.count, 3);
        assert_eq!(sum.cleanup.unwrap().count, 1);
        assert_eq!(sum.stt.unwrap().count, 3);
        // The failed run's 50 s is not a Finish → Idle latency.
        assert_eq!(sum.total.unwrap().p90, 900.0);
        // 0.1 and 0.2; the run without audio length has no factor.
        let rtf = sum.realtime_factor.unwrap();
        assert_eq!(rtf.count, 2);
        assert_eq!(rtf.median, 0.1);
        assert_eq!(rtf.p90, 0.2);
        assert!(sum.paste.is_none());
    }
}
