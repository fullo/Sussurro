//! Engine tests: pure helpers, the whole pipeline with fake STT/cleanup on
//! synthetic audio, and an `#[ignore]`d end-to-end run with a real model.

use super::*;
use crate::archive::Word;
use crate::sources::{Clock, Frame};
use segmenter::EnergyDetector;

/// A source replaying synthetic audio in fixed-size chunks.
struct VecSource {
    audio: Vec<f32>,
    pos: usize,
    chunk: usize,
    clock: Clock,
    channel: Channel,
}

impl VecSource {
    fn new(audio: Vec<f32>, channel: Channel) -> Self {
        Self {
            audio,
            pos: 0,
            chunk: 4_000,
            clock: Clock::default(),
            channel,
        }
    }
}

impl Source for VecSource {
    fn channel(&self) -> Channel {
        self.channel
    }
    fn total_samples(&self) -> Option<u64> {
        Some(self.audio.len() as u64)
    }
    fn next_frame(&mut self) -> Result<Option<Frame>> {
        if self.pos >= self.audio.len() {
            return Ok(None);
        }
        let end = (self.pos + self.chunk).min(self.audio.len());
        let chunk = self.audio[self.pos..end].to_vec();
        self.pos = end;
        Ok(Some(self.clock.stamp(self.channel, chunk)))
    }
}

/// "Transcribes" each segment as `word<n> …`, one word per second of audio,
/// without timings (so the engine must estimate them).
struct FakeStt {
    calls: usize,
    fail_on: Option<usize>,
}

impl SegmentStt for FakeStt {
    fn transcribe(&mut self, samples: &[f32]) -> Result<TimedTranscript> {
        self.calls += 1;
        if self.fail_on == Some(self.calls) {
            anyhow::bail!("model not downloaded");
        }
        let n = (samples.len() / 16_000).max(1);
        let text = (0..n)
            .map(|i| format!("w{}x{i}", self.calls))
            .collect::<Vec<_>>()
            .join(" ");
        Ok(TimedTranscript {
            text,
            language: Some("it".into()),
            ..Default::default()
        })
    }
}

/// Uppercases the segment and records the context it was given.
#[derive(Default)]
struct FakeCleaner {
    seen: Mutex<Vec<(Option<String>, String)>>,
}

impl Cleaner for FakeCleaner {
    fn clean(&self, previous: Option<&str>, raw: &str) -> String {
        self.seen
            .lock()
            .unwrap()
            .push((previous.map(str::to_string), raw.to_string()));
        raw.to_uppercase()
    }
}

#[derive(Default)]
struct VecSink(Mutex<Vec<EngineEvent>>);

impl EngineSink for VecSink {
    fn emit(&self, event: &EngineEvent) {
        self.0.lock().unwrap().push(event.clone());
    }
}

/// Speech-like bursts (a loud tone) separated by silence.
fn bursts(pattern: &[(bool, f32)]) -> Vec<f32> {
    let mut out = Vec::new();
    for &(speech, secs) in pattern {
        let n = (secs * 16_000.0) as usize;
        out.extend((0..n).map(|i| {
            if speech {
                0.2 * ((i as f32) * 0.07).sin()
            } else {
                0.0
            }
        }));
    }
    out
}

fn job(dir: &std::path::Path, audio: Vec<f32>, policy: Policy) -> Job {
    Job {
        session_id: 7,
        source: Box::new(VecSource::new(audio, Channel::File)),
        detector: Box::new(EnergyDetector::default()),
        params: SegmenterParams::default(),
        policy,
        spool_path: dir.join("spool.f32"),
        defer: false,
        cancel: Arc::new(AtomicBool::new(false)),
        voice_commands: false,
        archive_dir: dir.join("archive"),
        index_db: Some(dir.join("index.sqlite")),
        journal: Some(dir.join(checkpoint::JOURNAL_FILE)),
        meta: ItemMeta {
            item_type: ItemType::Transcription,
            source: "file:test.wav".into(),
            language: "auto".into(),
            engine: "fake".into(),
            ..Default::default()
        },
    }
}

#[test]
fn pipeline_segments_cleans_with_context_and_writes_an_item() {
    let dir = tempfile::tempdir().unwrap();
    let audio = bursts(&[
        (false, 1.0),
        (true, 6.0),
        (false, 3.0),
        (true, 2.0),
        (false, 3.0),
        (true, 65.0), // capped at 30 s → split
        (false, 1.0),
    ]);
    let sink = Arc::new(VecSink::default());
    let cleaner = FakeCleaner::default();
    let mut stt = FakeStt {
        calls: 0,
        fail_on: None,
    };
    let r = run(
        job(dir.path(), audio, Policy::Block { max_queued: 1 }),
        &mut stt,
        &cleaner,
        sink.clone(),
    )
    .unwrap();

    // 2 short segments + the 65 s burst in ≥ 3 capped pieces.
    assert!(r.segments >= 5, "got {} segments", r.segments);
    let item = archive::read_item(&dir.path().join("archive"), &r.item_id).unwrap();
    let segs = &item.segments.segments;
    assert_eq!(segs.len(), r.segments);
    for (i, s) in segs.iter().enumerate() {
        assert_eq!(s.id as usize, i);
        assert_eq!(s.channel, Channel::File);
        assert!(s.end_ms - s.start_ms <= 30_000);
        // Raw and cleaned both kept.
        assert_eq!(s.text, s.raw.to_uppercase());
        // No timings from the fake engine: estimated, inside the segment.
        assert!(s.words_estimated);
        assert!(!s.words.is_empty());
        assert_eq!(s.words[0].start_ms, s.start_ms);
        assert_eq!(s.words.last().unwrap().end_ms, s.end_ms);
    }
    for w in segs.windows(2) {
        assert!(w[0].end_ms <= w[1].start_ms);
    }
    // Chunked cleanup: each segment saw only the previous cleaned segment.
    let seen = cleaner.seen.lock().unwrap();
    assert_eq!(seen[0].0, None);
    for i in 1..seen.len() {
        assert_eq!(seen[i].0.as_deref(), Some(segs[i - 1].text.as_str()));
    }
    // Frontmatter: type from the caller, language detected, duration filled.
    assert_eq!(item.meta.item_type, ItemType::Transcription);
    assert_eq!(item.meta.source, "file:test.wav");
    assert_eq!(item.meta.language, "it");
    assert_eq!(item.meta.duration.as_deref(), Some("00:01:21"));
    assert!(!item.meta.title.is_empty());
    // Finalized (#153): no in-progress marker, nothing left to recover, and
    // the folder renamed from `…-untitled` after the auto title.
    assert!(!item.recording && !item.interrupted);
    assert!(!item.meta.extra.contains_key(archive::SESSION_KEY));
    assert!(checkpoint::journal_entries(&dir.path().join(checkpoint::JOURNAL_FILE)).is_empty());
    assert!(!r.item_id.ends_with("-untitled"), "{}", r.item_id);
    assert_eq!(archive::list_items(&dir.path().join("archive")).len(), 1);
    assert!(item.body.contains("W1X0"));
    // Indexed: searchable right away.
    let hits = archive::with_index(
        &dir.path().join("archive"),
        &dir.path().join("index.sqlite"),
        |idx| idx.search("w1x0", &Default::default()),
    )
    .unwrap();
    assert_eq!(hits.len(), 1);

    // Events: one segment event per segment, progress, then done last.
    let events = sink.0.lock().unwrap();
    let n_seg = events
        .iter()
        .filter(|e| matches!(e, EngineEvent::Segment(_)))
        .count();
    assert_eq!(n_seg, r.segments);
    assert!(events.iter().any(|e| matches!(e, EngineEvent::Progress(_))));
    match events.last().unwrap() {
        EngineEvent::Done(d) => {
            assert_eq!(d.session_id, 7);
            assert_eq!(d.item_id, r.item_id);
            assert_eq!(d.text, r.text);
            assert!(d.text.starts_with("W1X0"));
        }
        other => panic!("last event {other:?}"),
    }
    // The final progress reports everything processed, nothing queued.
    let last_progress = events
        .iter()
        .rev()
        .find_map(|e| match e {
            EngineEvent::Progress(p) => Some(p.clone()),
            _ => None,
        })
        .unwrap();
    assert_eq!(last_progress.queue_len, 0);
    assert_eq!(last_progress.backlog_s, 0.0);
    assert_eq!(last_progress.total_s, 81.0);
}

#[test]
fn deferred_mic_run_spills_and_still_writes_everything() {
    let dir = tempfile::tempdir().unwrap();
    let audio = bursts(&[(true, 2.0), (false, 2.5)].repeat(8));
    let mut j = job(dir.path(), Vec::new(), Policy::Spill { max_in_ram: 1 });
    j.source = Box::new(VecSource::new(audio, Channel::Mic));
    j.defer = true;
    j.meta.item_type = ItemType::Note;
    j.meta.source = "mic".into();
    /// Records the spool file's size when STT first runs: in deferred mode
    /// every segment is queued by then.
    struct SpoolWatch {
        inner: FakeStt,
        spool: std::path::PathBuf,
        spool_bytes_at_first_call: Option<u64>,
        samples: Vec<usize>,
    }
    impl SegmentStt for SpoolWatch {
        fn transcribe(&mut self, samples: &[f32]) -> Result<TimedTranscript> {
            if self.samples.is_empty() {
                self.spool_bytes_at_first_call =
                    Some(std::fs::metadata(&self.spool).map(|m| m.len()).unwrap_or(0));
            }
            self.samples.push(samples.len());
            self.inner.transcribe(samples)
        }
    }
    let mut stt = SpoolWatch {
        inner: FakeStt {
            calls: 0,
            fail_on: None,
        },
        spool: dir.path().join("spool.f32"),
        spool_bytes_at_first_call: None,
        samples: Vec::new(),
    };
    let r = run(
        j,
        &mut stt,
        &FakeCleaner::default(),
        Arc::new(VecSink::default()),
    )
    .unwrap();
    assert_eq!(r.segments, 8);
    // The cap held: one segment in RAM, the other seven went to disk…
    assert_eq!(r.queue.peak_in_ram, 1, "{:?}", r.queue);
    assert_eq!(r.queue.spilled, 7, "{:?}", r.queue);
    assert_eq!(r.queue.peak_len, 8, "deferred: all queued before STT");
    assert_eq!(r.queue.peak_ram_samples, stt.samples[0] as u64);
    // …really on disk: the spool held exactly segments 2..=8 as f32.
    let spilled: usize = stt.samples[1..].iter().sum();
    assert_eq!(stt.spool_bytes_at_first_call, Some(spilled as u64 * 4));
    let item = archive::read_item(&dir.path().join("archive"), &r.item_id).unwrap();
    assert!(item
        .segments
        .segments
        .iter()
        .all(|s| s.channel == Channel::Mic));
    assert_eq!(item.meta.item_type, ItemType::Note);
    assert!(!dir.path().join("spool.f32").exists(), "spool cleaned up");
}

#[test]
fn stt_failure_on_one_segment_is_marked_and_the_run_continues() {
    let dir = tempfile::tempdir().unwrap();
    let audio = bursts(&[(true, 3.0), (false, 2.5)].repeat(5));
    let sink = Arc::new(VecSink::default());
    let mut stt = FakeStt {
        calls: 0,
        fail_on: Some(2),
    };
    let cleaner = FakeCleaner::default();
    let r = run(
        job(dir.path(), audio, Policy::Block { max_queued: 1 }),
        &mut stt,
        &cleaner,
        sink.clone(),
    )
    .unwrap();
    assert_eq!(r.segments, 5);
    let item = archive::read_item(&dir.path().join("archive"), &r.item_id).unwrap();
    let segs = &item.segments.segments;
    let failed = &segs[1];
    assert!(failed.text.is_empty() && failed.raw.is_empty() && failed.words.is_empty());
    assert!(failed
        .stt_error
        .as_deref()
        .unwrap()
        .contains("model not downloaded"));
    assert!(failed.end_ms > failed.start_ms, "keeps its time range");
    assert!(segs
        .iter()
        .enumerate()
        .all(|(i, s)| (i == 1) == s.stt_error.is_some()));
    // Cleanup context skips the hole: segment 2 saw segment 0's text.
    let seen = cleaner.seen.lock().unwrap();
    assert_eq!(seen[1].0.as_deref(), Some(segs[0].text.as_str()));
    assert!(!item.body.contains("model not downloaded"));
    assert!(!item.interrupted);
    // The UI hears about the failed segment too.
    let events = sink.0.lock().unwrap();
    assert!(events.iter().any(|e| matches!(e,
        EngineEvent::Segment(p) if p.segment.stt_error.is_some())));
}

#[test]
fn stt_that_never_works_fails_fast_and_writes_nothing() {
    struct AlwaysFail(usize);
    impl SegmentStt for AlwaysFail {
        fn transcribe(&mut self, _: &[f32]) -> Result<TimedTranscript> {
            self.0 += 1;
            anyhow::bail!("model not downloaded")
        }
    }
    let dir = tempfile::tempdir().unwrap();
    let audio = bursts(&[(true, 3.0), (false, 2.5)].repeat(10));
    let sink = Arc::new(VecSink::default());
    let mut stt = AlwaysFail(0);
    let err = run(
        job(dir.path(), audio, Policy::Block { max_queued: 1 }),
        &mut stt,
        &FakeCleaner::default(),
        sink.clone(),
    )
    .unwrap_err();
    assert!(format!("{err:#}").contains("model not downloaded"));
    assert_eq!(stt.0, MAX_STT_FAILURES_UP_FRONT, "stops early");
    assert!(archive::list_items(&dir.path().join("archive")).is_empty());
    assert!(checkpoint::journal_entries(&dir.path().join(checkpoint::JOURNAL_FILE)).is_empty());
    match sink.0.lock().unwrap().last().unwrap() {
        EngineEvent::Error(e) => {
            assert!(e.error.contains("model not downloaded"));
            assert_eq!(e.item_id, None);
            assert!(EngineEvent::Error(e.clone())
                .payload()
                .get("item_id")
                .is_none());
        }
        other => panic!("last event {other:?}"),
    }

    // A short file whose only segment fails: the STT error, not "no speech".
    let dir = tempfile::tempdir().unwrap();
    let err = run(
        job(
            dir.path(),
            bursts(&[(true, 3.0)]),
            Policy::Block { max_queued: 1 },
        ),
        &mut AlwaysFail(0),
        &FakeCleaner::default(),
        Arc::new(VecSink::default()),
    )
    .unwrap_err();
    assert!(format!("{err:#}").contains("model not downloaded"));
    assert!(archive::list_items(&dir.path().join("archive")).is_empty());
}

#[test]
fn source_failure_mid_run_keeps_the_segments_done_as_interrupted() {
    /// Replays audio, then fails like an unplugged mic or a corrupt file.
    struct Breaks {
        inner: VecSource,
        after: usize,
    }
    impl Source for Breaks {
        fn channel(&self) -> Channel {
            self.inner.channel()
        }
        fn total_samples(&self) -> Option<u64> {
            None
        }
        fn next_frame(&mut self) -> Result<Option<Frame>> {
            if self.inner.pos >= self.after {
                anyhow::bail!("decode error at packet 42");
            }
            self.inner.next_frame()
        }
    }
    let dir = tempfile::tempdir().unwrap();
    let audio = bursts(&[(true, 3.0), (false, 2.5)].repeat(6));
    let mut j = job(dir.path(), Vec::new(), Policy::Block { max_queued: 4 });
    // Three bursts get through before the source breaks.
    j.source = Box::new(Breaks {
        inner: VecSource::new(audio, Channel::File),
        after: (16_000.0 * 5.5 * 3.0) as usize,
    });
    let sink = Arc::new(VecSink::default());
    let mut stt = FakeStt {
        calls: 0,
        fail_on: None,
    };
    let err = run(j, &mut stt, &FakeCleaner::default(), sink.clone()).unwrap_err();
    assert!(format!("{err:#}").contains("decode error"));
    let archive_dir = dir.path().join("archive");
    let items = archive::list_items(&archive_dir);
    assert_eq!(items.len(), 1);
    assert!(items[0].interrupted && !items[0].recording);
    let item = archive::read_item(&archive_dir, &items[0].id).unwrap();
    assert!(
        !item.segments.segments.is_empty(),
        "segments before the failure kept"
    );
    assert!(item.body.contains("W1X0"));
    assert!(checkpoint::journal_entries(&dir.path().join(checkpoint::JOURNAL_FILE)).is_empty());
    // Indexed right away.
    let hits = archive::with_index(&archive_dir, &dir.path().join("index.sqlite"), |i| {
        i.search("w1x0", &Default::default())
    })
    .unwrap();
    assert!(hits[0].interrupted);
    let events = sink.0.lock().unwrap();
    assert!(matches!(events.first().unwrap(), EngineEvent::Started(s)
        if s.item_id == items[0].id && s.session_id == 7));
    match events.last().unwrap() {
        EngineEvent::Error(e) => {
            assert_eq!(e.item_id.as_deref(), Some(items[0].id.as_str()));
            assert_eq!(
                EngineEvent::Error(e.clone()).payload()["item_id"],
                items[0].id
            );
        }
        other => panic!("last event {other:?}"),
    }
}

#[test]
fn unwritable_archive_fails_before_capturing_anything() {
    let dir = tempfile::tempdir().unwrap();
    let blocked = dir.path().join("not-a-folder");
    std::fs::write(&blocked, "x").unwrap();
    let mut j = job(
        dir.path(),
        bursts(&[(true, 3.0)]),
        Policy::Block { max_queued: 1 },
    );
    j.archive_dir = blocked.join("Sussurro");
    let mut stt = FakeStt {
        calls: 0,
        fail_on: None,
    };
    let sink = Arc::new(VecSink::default());
    let err = run(j, &mut stt, &FakeCleaner::default(), sink.clone()).unwrap_err();
    assert!(format!("{err:#}").contains("cannot write to the archive folder"));
    assert_eq!(stt.calls, 0, "no audio was processed");
    assert!(matches!(
        sink.0.lock().unwrap().last().unwrap(),
        EngineEvent::Error(_)
    ));
}

#[test]
fn a_crash_mid_run_leaves_a_recoverable_item() {
    let dir = tempfile::tempdir().unwrap();
    let archive_dir = dir.path().join("archive");
    let journal = dir.path().join(checkpoint::JOURNAL_FILE);
    let audio = bursts(&[(true, 3.0), (false, 2.5)].repeat(6));

    // An STT that looks at the archive while the run is going, then "dies"
    // (a panic unwinds the worker like a killed process never finishing).
    struct Observer {
        archive: std::path::PathBuf,
        calls: usize,
        seen: Arc<Mutex<Vec<usize>>>,
    }
    impl SegmentStt for Observer {
        fn transcribe(&mut self, _: &[f32]) -> Result<TimedTranscript> {
            self.calls += 1;
            let items = archive::list_items(&self.archive);
            assert_eq!(items.len(), 1, "item exists from the start");
            assert!(items[0].recording);
            let saved = archive::read_item(&self.archive, &items[0].id)
                .unwrap()
                .segments
                .segments
                .len();
            self.seen.lock().unwrap().push(saved);
            if self.calls == 4 {
                panic!("simulated crash");
            }
            Ok(TimedTranscript {
                text: format!("parte{}", self.calls),
                ..Default::default()
            })
        }
    }
    let seen = Arc::new(Mutex::new(Vec::new()));
    let mut j = job(dir.path(), audio, Policy::Block { max_queued: 1 });
    j.meta.title = "Crash".into();
    let mut stt = Observer {
        archive: archive_dir.clone(),
        calls: 0,
        seen: seen.clone(),
    };
    let crashed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        run(
            j,
            &mut stt,
            &FakeCleaner::default(),
            Arc::new(VecSink::default()),
        )
    }));
    assert!(crashed.is_err());
    // Before each STT call, every earlier segment was already on disk.
    assert_eq!(*seen.lock().unwrap(), vec![0, 1, 2, 3]);
    let items = archive::list_items(&archive_dir);
    assert!(items[0].recording, "left in progress by the crash");

    let data = dir.path().join("data");
    let r = checkpoint::recover(&journal, Some(&archive_dir), None, &data);
    assert_eq!(r.interrupted, vec![items[0].id.clone()]);
    let item = archive::read_item(&archive_dir, &items[0].id).unwrap();
    assert!(item.interrupted);
    assert_eq!(item.segments.segments.len(), 3);
    assert!(
        item.body.contains("PARTE1") && item.body.contains("PARTE3"),
        "{}",
        item.body
    );
}

#[test]
fn cancel_stops_the_run_without_writing() {
    let dir = tempfile::tempdir().unwrap();
    let audio = bursts(&[(true, 3.0), (false, 2.5)].repeat(10));
    let j = job(dir.path(), audio, Policy::Block { max_queued: 1 });
    let cancel = j.cancel.clone();

    struct CancelAfterFirst(Arc<AtomicBool>);
    impl SegmentStt for CancelAfterFirst {
        fn transcribe(&mut self, _: &[f32]) -> Result<TimedTranscript> {
            self.0.store(true, Ordering::Relaxed);
            Ok(TimedTranscript {
                text: "hello".into(),
                ..Default::default()
            })
        }
    }
    let err = run(
        j,
        &mut CancelAfterFirst(cancel),
        &FakeCleaner::default(),
        Arc::new(VecSink::default()),
    )
    .unwrap_err();
    assert!(format!("{err:#}").contains("cancelled"));
    assert!(archive::list_items(&dir.path().join("archive")).is_empty());
}

#[test]
fn silence_only_input_is_an_error_not_an_empty_item() {
    let dir = tempfile::tempdir().unwrap();
    let audio = bursts(&[(false, 5.0)]);
    let mut stt = FakeStt {
        calls: 0,
        fail_on: None,
    };
    let err = run(
        job(dir.path(), audio, Policy::Block { max_queued: 2 }),
        &mut stt,
        &FakeCleaner::default(),
        Arc::new(VecSink::default()),
    )
    .unwrap_err();
    assert!(format!("{err:#}").contains("no speech"));
    assert_eq!(stt.calls, 0, "silence never reaches STT");
}

#[test]
fn build_segment_uses_engine_timings_and_skips_non_speech() {
    let audio = SegmentAudio {
        start: 32_000, // 2 s into the recording
        samples: vec![0.0; 16_000],
    };
    let t = TimedTranscript {
        text: " ciao mondo ".into(),
        words: vec![
            Word {
                w: "ciao".into(),
                start_ms: 100,
                end_ms: 400,
            },
            Word {
                w: "mondo".into(),
                start_ms: 450,
                end_ms: 1_100,
            },
        ],
        ..Default::default()
    };
    let s = build_segment(
        3,
        Channel::Mic,
        &audio,
        t,
        Some("prima"),
        false,
        &FakeCleaner::default(),
    )
    .unwrap();
    assert_eq!((s.id, s.start_ms, s.end_ms), (3, 2_000, 3_000));
    assert_eq!(
        (s.raw.as_str(), s.text.as_str()),
        ("ciao mondo", "CIAO MONDO")
    );
    assert!(!s.words_estimated);
    assert_eq!((s.words[0].start_ms, s.words[0].end_ms), (2_100, 2_400));
    assert_eq!(s.words[1].end_ms, 3_000, "clamped to the segment");

    let blank = TimedTranscript {
        text: "[BLANK_AUDIO]".into(),
        ..Default::default()
    };
    assert!(build_segment(
        0,
        Channel::Mic,
        &audio,
        blank,
        None,
        false,
        &FakeCleaner::default()
    )
    .is_none());
}

#[test]
fn build_segment_applies_voice_commands_before_cleanup() {
    let audio = SegmentAudio {
        start: 0,
        samples: vec![0.0; 16_000],
    };
    let t = TimedTranscript {
        text: "first new line second".into(),
        ..Default::default()
    };
    let cleaner = FakeCleaner::default();
    let s = build_segment(0, Channel::Mic, &audio, t.clone(), None, true, &cleaner).unwrap();
    assert_eq!(s.raw, "first new line second", "raw stays as transcribed");
    assert_eq!(
        cleaner.seen.lock().unwrap()[0].1,
        crate::voice_commands::apply_basic_commands("first new line second")
    );
}

#[test]
fn non_speech_annotations_are_recognized() {
    for s in [
        "[BLANK_AUDIO]",
        " (upbeat music) ",
        "[Music] [Applause]",
        "*laughs*",
        "[MUSIC].",
    ] {
        assert!(is_non_speech(s), "{s:?}");
    }
    for s in ["hello", "[Music] hello", "(a) b", "[unclosed"] {
        assert!(!is_non_speech(s), "{s:?}");
    }
}

#[test]
fn transcript_text_breaks_paragraphs_on_long_pauses() {
    let seg = |start_ms, end_ms, text: &str| Segment {
        start_ms,
        end_ms,
        text: text.into(),
        ..Default::default()
    };
    let segs = vec![
        seg(0, 1_000, "One."),
        seg(1_500, 2_000, "Two."),
        seg(5_000, 6_000, "Three."),
        seg(6_100, 6_200, "  "),
    ];
    assert_eq!(transcript_text(&segs), "One. Two.\n\nThree.");
}

#[test]
fn finalize_meta_fills_duration_title_and_language() {
    let segs = vec![Segment {
        text: "Oggi parliamo del rilascio della versione zero sette, finalmente.".into(),
        ..Default::default()
    }];
    let m = finalize_meta(
        ItemMeta {
            language: "auto".into(),
            ..Default::default()
        },
        &segs,
        3_723_000,
        Some("it"),
    );
    assert_eq!(m.duration.as_deref(), Some("01:02:03"));
    assert_eq!(m.language, "it");
    assert_eq!(
        m.title,
        "Oggi parliamo del rilascio della versione zero sette"
    );
    // Caller-set values win.
    let m = finalize_meta(
        ItemMeta {
            title: "Mine".into(),
            language: "en".into(),
            ..Default::default()
        },
        &segs,
        0,
        Some("it"),
    );
    assert_eq!((m.title.as_str(), m.language.as_str()), ("Mine", "en"));
    // Nothing detected, nothing to title from.
    let m = finalize_meta(ItemMeta::default(), &[], 0, None);
    assert_eq!((m.title.as_str(), m.language.as_str()), ("Note", "auto"));
}

#[test]
fn event_names_and_payloads() {
    let b = Backlog {
        ingested: 160_000,
        open_start: Some(96_000),
        oldest_queued: Some(48_000),
        in_flight: Some(32_000),
    };
    let p = progress_payload(1, &b, Some(320_000), 2, 5);
    assert_eq!(
        (p.processed_s, p.ingested_s, p.total_s, p.backlog_s),
        (2.0, 10.0, 20.0, 8.0)
    );
    let e = EngineEvent::Progress(p);
    assert_eq!(e.name(), "engine-progress");
    let json = e.payload();
    assert_eq!(json["session_id"], 1);
    assert_eq!(json["queue_len"], 2);
    assert_eq!(json["segments_done"], 5);
    // Mic: no known total → what was ingested.
    assert_eq!(progress_payload(1, &b, None, 0, 0).total_s, 10.0);

    let seg = EngineEvent::Segment(SegmentPayload {
        session_id: 1,
        segment: Segment {
            raw: "r".into(),
            text: "t".into(),
            ..Default::default()
        },
    });
    assert_eq!(seg.name(), "engine-segment");
    assert_eq!(seg.payload()["segment"]["raw"], "r");
    assert_eq!(seg.payload()["segment"]["text"], "t");
    let done = EngineEvent::Done(DonePayload {
        session_id: 1,
        item_id: "2026/09/x".into(),
        item_type: ItemType::Transcription,
        title: "T".into(),
        text: "t".into(),
        segments: 1,
        duration_s: 1.5,
    });
    assert_eq!(done.name(), "engine-done");
    assert_eq!(done.payload()["item_type"], "transcription");
    let err = EngineEvent::Error(ErrorPayload {
        session_id: 1,
        error: "boom".into(),
        item_id: None,
    });
    assert_eq!(err.name(), "engine-error");
    assert_eq!(err.payload()["error"], "boom");
    let started = EngineEvent::Started(StartedPayload {
        session_id: 3,
        item_id: "2026/09/2026-09-24-untitled".into(),
        item_type: ItemType::Note,
        title: String::new(),
        source: "mic".into(),
    });
    assert_eq!(started.name(), "engine-started");
    assert_eq!(started.payload()["session_id"], 3);
    assert_eq!(started.payload()["item_type"], "note");
}

/// End to end with a real whisper model: a WAV (speech, a pause, the same
/// speech again) decoded from its path, VAD-segmented, transcribed with
/// token timings, "cleaned" (identity — no LLM in tests) and written as an
/// archive item into a tempdir. Needs a model:
///   SUSSURRO_TEST_MODEL=/path/ggml-base.en.bin
///   SUSSURRO_TEST_WAV=/path/speech.wav        (16 kHz+ speech, e.g. jfk.wav)
///   SUSSURRO_TEST_VAD_MODEL=/path/ggml-silero-v5.1.2.bin   (optional;
///     without it the energy segmenter is used)
///   cargo test engine_end_to_end -- --ignored --nocapture
#[test]
#[ignore]
fn engine_end_to_end_with_a_real_model() {
    use crate::stt::whisper::Transcriber;
    let model = std::env::var("SUSSURRO_TEST_MODEL").expect("set SUSSURRO_TEST_MODEL");
    let wav = std::env::var("SUSSURRO_TEST_WAV").expect("set SUSSURRO_TEST_WAV");
    let speech = crate::audio::decode::decode_to_16k_mono(std::path::Path::new(&wav)).unwrap();

    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("input.wav");
    let mut audio = speech.clone();
    audio.extend(vec![0.0; 32_000]);
    audio.extend(&speech);
    crate::audio::decode::write_wav_i16(&input, 16_000, 1, &audio);

    struct WhisperStt(Transcriber);
    impl SegmentStt for WhisperStt {
        fn transcribe(&mut self, samples: &[f32]) -> Result<TimedTranscript> {
            self.0.transcribe_timed(samples, None, "auto")
        }
    }
    struct Identity;
    impl Cleaner for Identity {
        fn clean(&self, _: Option<&str>, raw: &str) -> String {
            raw.to_string()
        }
    }
    let detector: Box<dyn SpeechDetector> = match std::env::var("SUSSURRO_TEST_VAD_MODEL") {
        Ok(p) => Box::new(vad::SileroDetector::load(std::path::Path::new(&p)).unwrap()),
        Err(_) => Box::new(EnergyDetector::default()),
    };
    println!("VAD: {}", detector.name());
    let archive_dir = dir.path().join("Sussurro");
    let job = Job {
        session_id: 1,
        source: Box::new(crate::sources::file::FileSource::open(&input).unwrap()),
        detector,
        params: SegmenterParams::default(),
        policy: Policy::Block {
            max_queued: FILE_MAX_QUEUED,
        },
        spool_path: dir.path().join("spool"),
        defer: false,
        cancel: Arc::new(AtomicBool::new(false)),
        voice_commands: false,
        archive_dir: archive_dir.clone(),
        index_db: Some(dir.path().join("index.sqlite")),
        journal: None,
        meta: ItemMeta {
            item_type: ItemType::Transcription,
            source: crate::sources::file::source_label(&input),
            language: "auto".into(),
            engine: "whisper-test".into(),
            ..Default::default()
        },
    };
    let mut stt = WhisperStt(Transcriber::load(std::path::Path::new(&model)).unwrap());
    let sink = Arc::new(VecSink::default());
    let started = Instant::now();
    let r = run(job, &mut stt, &Identity, sink.clone()).unwrap();
    println!(
        "{} segments, {:.1} s of audio in {:.1} s\n{}",
        r.segments,
        r.duration_ms as f64 / 1000.0,
        started.elapsed().as_secs_f64(),
        r.text
    );
    assert!(r.segments >= 2, "the pause splits the two copies");
    let item = archive::read_item(&archive_dir, &r.item_id).unwrap();
    assert_eq!(item.meta.source, "file:input.wav");
    let item_dir = archive::paths::item_dir(&archive_dir, &r.item_id).unwrap();
    assert!(item_dir.join(".sussurro").join("segments.json").is_file());
    assert!(item_dir.join("transcript.md").is_file());
    for s in &item.segments.segments {
        println!(
            "[{} → {}] {} words, first {:?}",
            s.start_ms,
            s.end_ms,
            s.words.len(),
            s.words.first()
        );
        assert!(!s.raw.is_empty());
        assert!(!s.words.is_empty());
        assert!(!s.words_estimated, "whisper gives real token timings");
        assert!(s
            .words
            .iter()
            .all(|w| w.start_ms >= s.start_ms && w.end_ms <= s.end_ms));
    }
    assert!(item
        .body
        .contains(r.text.split_whitespace().next().unwrap()));
}

/// Memory check on a long file (plan §11) with a real model: asserts the
/// segment queue's high-water marks stay within the module-doc bound
/// whatever the length (a whole-file decode of 60 min is ~230 MB of f32).
/// For the process as a whole, also run it under `/usr/bin/time -l`
/// (macOS) or `-v` (Linux) and compare the peak RSS with the model alone.
///   SUSSURRO_TEST_MODEL=/path/ggml-tiny.en.bin
///   SUSSURRO_TEST_LONG_WAV=/path/60-minutes.wav
///   cargo test engine_long_file -- --ignored --nocapture
#[test]
#[ignore]
fn engine_long_file_streams_with_bounded_memory() {
    use crate::stt::whisper::Transcriber;
    let model = std::env::var("SUSSURRO_TEST_MODEL").expect("set SUSSURRO_TEST_MODEL");
    let wav = std::env::var("SUSSURRO_TEST_LONG_WAV").expect("set SUSSURRO_TEST_LONG_WAV");
    struct WhisperStt(Transcriber);
    impl SegmentStt for WhisperStt {
        fn transcribe(&mut self, samples: &[f32]) -> Result<TimedTranscript> {
            self.0.transcribe_timed(samples, None, "en")
        }
    }
    struct Identity;
    impl Cleaner for Identity {
        fn clean(&self, _: Option<&str>, raw: &str) -> String {
            raw.to_string()
        }
    }
    let dir = tempfile::tempdir().unwrap();
    let path = std::path::Path::new(&wav);
    let job = Job {
        session_id: 1,
        source: Box::new(crate::sources::file::FileSource::open(path).unwrap()),
        detector: match std::env::var("SUSSURRO_TEST_VAD_MODEL") {
            Ok(p) => Box::new(vad::SileroDetector::load(std::path::Path::new(&p)).unwrap()),
            Err(_) => Box::new(EnergyDetector::default()),
        },
        params: SegmenterParams::default(),
        policy: Policy::Block {
            max_queued: FILE_MAX_QUEUED,
        },
        spool_path: dir.path().join("spool"),
        defer: false,
        cancel: Arc::new(AtomicBool::new(false)),
        voice_commands: false,
        archive_dir: dir.path().join("Sussurro"),
        index_db: None,
        journal: None,
        meta: ItemMeta {
            item_type: ItemType::Transcription,
            source: crate::sources::file::source_label(path),
            ..Default::default()
        },
    };
    let mut stt = WhisperStt(Transcriber::load(std::path::Path::new(&model)).unwrap());
    let started = Instant::now();
    let r = run(job, &mut stt, &Identity, Arc::new(VecSink::default())).unwrap();
    println!(
        "{} segments, {:.0} s of audio in {:.1} s, queue {:?}",
        r.segments,
        r.duration_ms as f64 / 1000.0,
        started.elapsed().as_secs_f64(),
        r.queue
    );
    // The audio waiting in RAM stayed within the module-doc bound whatever
    // the file's length (a whole-file decode would be ~230 MB per hour).
    assert!(
        r.segments > FILE_MAX_QUEUED,
        "a long file: {} segments",
        r.segments
    );
    assert_bounded_file_queue(&r.queue);
}

/// The file policy's memory bound: at most `FILE_MAX_QUEUED` segments of at
/// most `max_segment_ms` each waiting, none spilled.
fn assert_bounded_file_queue(q: &queue::QueueStats) {
    let max_segment = segmenter::ms_to_samples(SegmenterParams::default().max_segment_ms) as u64;
    assert!(q.peak_len <= FILE_MAX_QUEUED, "{q:?}");
    assert!(q.peak_in_ram <= FILE_MAX_QUEUED, "{q:?}");
    assert!(
        q.peak_ram_samples <= FILE_MAX_QUEUED as u64 * max_segment,
        "{q:?}"
    );
    assert_eq!(q.spilled, 0, "files block, never spill: {q:?}");
}

/// Continuous speech generated as it is read (the test holds no copy), that
/// records how far the decoder ever ran ahead of the STT.
struct LongSpeech {
    total: u64,
    pos: u64,
    clock: Clock,
    transcribed: Arc<std::sync::atomic::AtomicU64>,
    max_lead: Arc<std::sync::atomic::AtomicU64>,
}

impl Source for LongSpeech {
    fn channel(&self) -> Channel {
        Channel::File
    }
    fn total_samples(&self) -> Option<u64> {
        Some(self.total)
    }
    fn next_frame(&mut self) -> Result<Option<Frame>> {
        if self.pos >= self.total {
            return Ok(None);
        }
        let n = 4_000.min(self.total - self.pos);
        let chunk = (self.pos..self.pos + n)
            .map(|i| 0.2 * ((i as f32) * 0.07).sin())
            .collect();
        self.pos += n;
        let lead = self.pos - self.transcribed.load(Ordering::SeqCst);
        self.max_lead.fetch_max(lead, Ordering::SeqCst);
        Ok(Some(self.clock.stamp(Channel::File, chunk)))
    }
}

/// Deterministic twin of `engine_long_file_streams_with_bounded_memory`:
/// 6 minutes of speech (12 capped segments), with the first STT call held
/// until the decoder is as far ahead as it can get — so the bound is
/// reached, not just respected by an STT that happened to keep up.
#[test]
fn long_file_decoding_stays_a_bounded_distance_ahead_of_stt() {
    use std::sync::atomic::AtomicU64;
    let dir = tempfile::tempdir().unwrap();
    let max_segment = segmenter::ms_to_samples(SegmenterParams::default().max_segment_ms) as u64;
    let transcribed = Arc::new(AtomicU64::new(0));
    let max_lead = Arc::new(AtomicU64::new(0));
    let mut j = job(
        dir.path(),
        Vec::new(),
        Policy::Block {
            max_queued: FILE_MAX_QUEUED,
        },
    );
    j.source = Box::new(LongSpeech {
        total: 12 * max_segment,
        pos: 0,
        clock: Clock::default(),
        transcribed: transcribed.clone(),
        max_lead: max_lead.clone(),
    });
    j.index_db = None;

    struct SlowFirst {
        inner: FakeStt,
        transcribed: Arc<AtomicU64>,
        max_lead: Arc<AtomicU64>,
        fill: u64,
    }
    impl SegmentStt for SlowFirst {
        fn transcribe(&mut self, samples: &[f32]) -> Result<TimedTranscript> {
            if self.inner.calls == 0 {
                // In flight + a full queue behind it.
                let deadline = Instant::now() + Duration::from_secs(10);
                while self.max_lead.load(Ordering::SeqCst) < self.fill {
                    assert!(
                        Instant::now() < deadline,
                        "the decoder never filled the queue"
                    );
                    std::thread::sleep(Duration::from_millis(1));
                }
            }
            let r = self.inner.transcribe(samples);
            self.transcribed
                .fetch_add(samples.len() as u64, Ordering::SeqCst);
            r
        }
    }
    let mut stt = SlowFirst {
        inner: FakeStt {
            calls: 0,
            fail_on: None,
        },
        transcribed: transcribed.clone(),
        max_lead: max_lead.clone(),
        fill: (FILE_MAX_QUEUED as u64 + 1) * max_segment,
    };
    let r = run(
        j,
        &mut stt,
        &FakeCleaner::default(),
        Arc::new(VecSink::default()),
    )
    .unwrap();
    assert!(r.segments >= 12, "{} segments", r.segments);
    // Everything but the detector's onset reached STT (the few frames it
    // missed only make the measured lead below more pessimistic).
    let missed = 12 * max_segment - transcribed.load(Ordering::SeqCst);
    assert!(missed < 16_000, "{missed} samples never transcribed");
    assert_bounded_file_queue(&r.queue);
    assert_eq!(r.queue.peak_len, FILE_MAX_QUEUED, "the bound was reached");
    // Ahead of STT: the segment in flight, the queue, the segment waiting
    // to be queued plus what is left open after it (≤ one cap together),
    // the VAD batch being fed, the aligner's rest and one source chunk —
    // a fixed amount, never the whole file (12 caps here).
    let bound = (FILE_MAX_QUEUED as u64 + 2) * max_segment
        + 2 * (VAD_BATCH_FRAMES * segmenter::FRAME) as u64
        + 4_000;
    let lead = max_lead.load(Ordering::SeqCst);
    assert!(
        lead <= bound,
        "decoder ran {lead} samples ahead (bound {bound})"
    );
}

/// The app's STT under test conditions: every segment takes the shared
/// model through the dictation gate, like `session::AppStt`.
struct GatedStt {
    gate: Arc<priority::DictationGate>,
    model: Arc<Mutex<Vec<String>>>,
    calls: usize,
    /// Run inside the first segment, with the model held.
    during_first: Option<Box<dyn FnOnce() + Send>>,
}

impl SegmentStt for GatedStt {
    fn transcribe(&mut self, _samples: &[f32]) -> Result<TimedTranscript> {
        let mut model = priority::acquire_yielding(&self.gate, || Ok(self.model.lock().unwrap()))?;
        self.calls += 1;
        model.push(format!("segment {}", self.calls));
        if let Some(f) = self.during_first.take() {
            f();
        }
        Ok(TimedTranscript {
            text: format!("word{}", self.calls),
            ..Default::default()
        })
    }
}

/// #154: a hotkey dictation pressed while segment 1 is transcribing waits
/// for that segment only, and is served before segment 2.
#[test]
fn a_dictation_during_a_run_is_served_before_the_next_segment() {
    let dir = tempfile::tempdir().unwrap();
    let audio = bursts(&[(true, 2.0), (false, 2.5)].repeat(4));
    let gate = Arc::new(priority::DictationGate::default());
    let model = Arc::new(Mutex::new(Vec::new()));
    let dictation = Arc::new(Mutex::new(None));
    let during_first: Box<dyn FnOnce() + Send> = {
        let (gate, model, dictation) = (gate.clone(), model.clone(), dictation.clone());
        Box::new(move || {
            gate.begin(); // hotkey pressed: recording starts
            *dictation.lock().unwrap() = Some(std::thread::spawn(move || {
                // Recording stops once the engine is parked on the gate —
                // i.e. segment 1 is done and segment 2 was not started.
                let deadline = Instant::now() + Duration::from_secs(10);
                while gate.waiting() == 0 {
                    assert!(Instant::now() < deadline, "the engine never yielded");
                    std::thread::yield_now();
                }
                model.lock().unwrap().push("dictation".to_string());
                gate.end(); // final pass done
            }));
        })
    };
    let mut stt = GatedStt {
        gate: gate.clone(),
        model: model.clone(),
        calls: 0,
        during_first: Some(during_first),
    };
    let r = run(
        job(dir.path(), audio, Policy::Block { max_queued: 4 }),
        &mut stt,
        &FakeCleaner::default(),
        Arc::new(VecSink::default()),
    )
    .unwrap();
    let t = dictation.lock().unwrap().take().unwrap();
    t.join().unwrap();
    assert_eq!(r.segments, 4);
    assert_eq!(
        *model.lock().unwrap(),
        [
            "segment 1",
            "dictation",
            "segment 2",
            "segment 3",
            "segment 4"
        ]
    );
    assert!(!gate.is_pending());
}
