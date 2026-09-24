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
        external_cleanup: None,
        speakers: None,
        write_subtitles: false,
        save_audio: false,
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

/// #122: the external-cleanup entry a session carries.
fn external_entry() -> archive::external::ExternalSend {
    archive::external::ExternalSend {
        date: "2026-09-24T12:00:00+02:00".into(),
        host: "api.example.com".into(),
        profile: "Work".into(),
        model: "gpt".into(),
        kind: archive::external::SendKind::Cleanup,
        recipe: String::new(),
    }
}

/// A fake external LLM cleaner: on every call it notes how many
/// external-send entries the live item's log already holds, and it can
/// cancel the run after its first call.
struct ExternalLlm {
    archive: PathBuf,
    logged_at_call: Mutex<Vec<usize>>,
    cancel_after_first: Option<Arc<AtomicBool>>,
}

impl Cleaner for ExternalLlm {
    fn clean(&self, _: Option<&str>, raw: &str) -> String {
        let items = archive::list_items(&self.archive);
        let logged = items
            .first()
            .map(|i| archive::external::read_log(&self.archive, &i.id).unwrap().len())
            .unwrap_or(0);
        self.logged_at_call.lock().unwrap().push(logged);
        if let Some(c) = &self.cancel_after_first {
            c.store(true, Ordering::Relaxed);
        }
        raw.to_uppercase()
    }
}

/// #122: a run that fails after its cleanup already went to an external
/// host keeps the send in its log (recorded before the first segment was
/// sent, once per run), so the kept item is marked.
#[test]
fn external_cleanup_is_logged_before_the_first_send_even_if_the_run_fails() {
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
    let archive_dir = dir.path().join("archive");
    let mut j = job(dir.path(), Vec::new(), Policy::Block { max_queued: 4 });
    j.source = Box::new(Breaks {
        inner: VecSource::new(bursts(&[(true, 3.0), (false, 2.5)].repeat(6)), Channel::File),
        after: (16_000.0 * 5.5 * 3.0) as usize,
    });
    j.external_cleanup = Some(external_entry());
    let llm = ExternalLlm { archive: archive_dir.clone(), logged_at_call: Mutex::new(Vec::new()), cancel_after_first: None };
    let mut stt = FakeStt { calls: 0, fail_on: None };
    let err = run(j, &mut stt, &llm, Arc::new(VecSink::default())).unwrap_err();
    assert!(format!("{err:#}").contains("decode error"));
    let calls = llm.logged_at_call.lock().unwrap().clone();
    assert!(!calls.is_empty(), "a segment was cleaned before the failure");
    assert!(calls.iter().all(|&n| n == 1), "logged before the first send, once per run: {calls:?}");

    let items = archive::list_items(&archive_dir);
    assert_eq!(items.len(), 1);
    assert!(items[0].interrupted);
    assert_eq!(items[0].external_hosts, ["api.example.com"]);
    let log = archive::external::read_log(&archive_dir, &items[0].id).unwrap();
    assert_eq!(log, [external_entry()]);
}

/// #122: a run cancelled right after its first external cleanup call had
/// already recorded the send when that call went out.
#[test]
fn external_cleanup_is_logged_before_the_first_send_even_if_the_run_is_cancelled() {
    let dir = tempfile::tempdir().unwrap();
    let archive_dir = dir.path().join("archive");
    let mut j = job(dir.path(), bursts(&[(true, 3.0), (false, 2.5)].repeat(10)), Policy::Block { max_queued: 1 });
    j.external_cleanup = Some(external_entry());
    let llm = ExternalLlm {
        archive: archive_dir.clone(),
        logged_at_call: Mutex::new(Vec::new()),
        cancel_after_first: Some(j.cancel.clone()),
    };
    let mut stt = FakeStt { calls: 0, fail_on: None };
    let err = run(j, &mut stt, &llm, Arc::new(VecSink::default())).unwrap_err();
    assert!(format!("{err:#}").contains("cancelled"));
    let calls = llm.logged_at_call.lock().unwrap().clone();
    assert_eq!(calls.first(), Some(&1), "the entry was on disk when the first text went out: {calls:?}");
    assert!(calls.iter().all(|&n| n == 1), "one entry per run: {calls:?}");
}

/// A finished run with several externally cleaned segments logs one entry.
#[test]
fn external_cleanup_is_logged_once_per_run() {
    let dir = tempfile::tempdir().unwrap();
    let archive_dir = dir.path().join("archive");
    let mut j = job(dir.path(), bursts(&[(true, 3.0), (false, 2.5)].repeat(3)), Policy::Block { max_queued: 2 });
    j.external_cleanup = Some(external_entry());
    let llm = ExternalLlm { archive: archive_dir.clone(), logged_at_call: Mutex::new(Vec::new()), cancel_after_first: None };
    let r = run(j, &mut FakeStt { calls: 0, fail_on: None }, &llm, Arc::new(VecSink::default())).unwrap();
    let calls = llm.logged_at_call.lock().unwrap().clone();
    assert!(calls.len() >= 2 && calls.iter().all(|&n| n == 1), "{calls:?}");
    assert_eq!(archive::external::read_log(&archive_dir, &r.item_id).unwrap(), [external_entry()]);
    assert_eq!(archive::read_item(&archive_dir, &r.item_id).unwrap().external_hosts, ["api.example.com"]);
}

/// Without an external cleanup nothing is logged.
#[test]
fn local_cleanup_logs_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let archive_dir = dir.path().join("archive");
    let j = job(dir.path(), bursts(&[(true, 3.0), (false, 2.5)].repeat(2)), Policy::Block { max_queued: 2 });
    let llm = ExternalLlm { archive: archive_dir.clone(), logged_at_call: Mutex::new(Vec::new()), cancel_after_first: None };
    let r = run(j, &mut FakeStt { calls: 0, fail_on: None }, &llm, Arc::new(VecSink::default())).unwrap();
    assert!(llm.logged_at_call.lock().unwrap().iter().all(|&n| n == 0));
    assert!(archive::read_item(&archive_dir, &r.item_id).unwrap().external_hosts.is_empty());
}

/// The subtitles setting *Always* (#133): the finished item gets its
/// `transcript.srt`; a note never does (P10), and *On request* writes none.
#[test]
fn always_subtitles_write_the_sidecar_at_the_end_of_a_run() {
    for (item_type, write, expect) in [
        (ItemType::Transcription, true, true),
        (ItemType::Meeting, true, true),
        (ItemType::Note, true, false),
        (ItemType::Transcription, false, false),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let audio = bursts(&[(false, 1.0), (true, 3.0), (false, 2.0), (true, 2.0), (false, 1.0)]);
        let mut j = job(dir.path(), audio, Policy::Block { max_queued: 2 });
        j.meta.item_type = item_type;
        j.write_subtitles = write;
        let mut stt = FakeStt {
            calls: 0,
            fail_on: None,
        };
        let r = run(
            j,
            &mut stt,
            &FakeCleaner::default(),
            Arc::new(VecSink::default()),
        )
        .unwrap();
        let srt = dir
            .path()
            .join("archive")
            .join(&r.item_id)
            .join(archive::export::SUBTITLES_FILE);
        assert_eq!(srt.is_file(), expect, "{item_type:?} write={write}");
        if expect {
            let text = std::fs::read_to_string(&srt).unwrap();
            assert!(text.starts_with("1\n00:00:0"), "{text}");
            assert!(text.contains(" --> "), "{text}");
        }
    }
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
        channel: Channel::Mic,
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
        channel: Channel::Mic,
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
    let dl = EngineEvent::Download(DownloadPayload {
        session_id: 4,
        via: crate::sources::url::Via::YtDlp,
        downloaded_bytes: 10,
        total_bytes: None,
        title: Some("T".into()),
    });
    assert_eq!(dl.name(), "engine-download");
    assert_eq!(dl.payload()["via"], "yt-dlp");
    assert_eq!(dl.payload()["total_bytes"], serde_json::Value::Null);
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
        external_cleanup: None,
        speakers: None,
        write_subtitles: false,
        save_audio: false,
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
        external_cleanup: None,
        speakers: None,
        write_subtitles: false,
        save_audio: false,
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
/// model through the dictation gate, like `session::app_transcriber`.
struct GatedStt {
    gate: Arc<priority::DictationGate>,
    /// The run's cancel flag, as `session::app_transcriber` holds it.
    cancel: Arc<AtomicBool>,
    model: Arc<Mutex<Vec<String>>>,
    calls: usize,
    /// Run inside the first segment, with the model held.
    during_first: Option<Box<dyn FnOnce() + Send>>,
}

impl SegmentStt for GatedStt {
    fn transcribe(&mut self, _samples: &[f32]) -> Result<TimedTranscript> {
        let mut model = priority::acquire_yielding(&self.gate, &self.cancel, || {
            Ok(self.model.lock().unwrap())
        })?;
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
    let j = job(dir.path(), audio, Policy::Block { max_queued: 4 });
    let mut stt = GatedStt {
        gate: gate.clone(),
        cancel: j.cancel.clone(),
        model: model.clone(),
        calls: 0,
        during_first: Some(during_first),
    };
    let r = run(
        j,
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

/// #158 finding 4: Cancel pressed while the engine waits for a dictation
/// (one that outlives the run here) ends the run at once, with no failed
/// segment and nothing left in the archive.
#[test]
fn cancel_while_waiting_for_a_dictation_ends_the_run() {
    let dir = tempfile::tempdir().unwrap();
    let audio = bursts(&[(true, 2.0), (false, 2.5)].repeat(3));
    let gate = Arc::new(priority::DictationGate::default());
    gate.begin(); // the hotkey is held: every segment has to wait
    let j = job(dir.path(), audio, Policy::Block { max_queued: 4 });
    let cancel = j.cancel.clone();
    let mut stt = GatedStt {
        gate: gate.clone(),
        cancel: cancel.clone(),
        model: Arc::new(Mutex::new(Vec::new())),
        calls: 0,
        during_first: None,
    };
    let sink = Arc::new(VecSink::default());
    let (tx, rx) = std::sync::mpsc::channel();
    {
        let sink = sink.clone();
        std::thread::spawn(move || {
            let r = run(j, &mut stt, &FakeCleaner::default(), sink);
            tx.send((r.map(|_| ()).map_err(|e| e.to_string()), stt.calls))
                .unwrap();
        });
    }
    let deadline = Instant::now() + Duration::from_secs(10);
    while gate.waiting() == 0 {
        assert!(Instant::now() < deadline, "the engine never waited");
        std::thread::yield_now();
    }
    cancel.store(true, Ordering::Relaxed);
    let (r, calls) = rx
        .recv_timeout(Duration::from_secs(5))
        .expect("the engine ignored Cancel while waiting for the dictation");
    assert_eq!(r, Err("cancelled".to_string()));
    assert_eq!(calls, 0, "no segment was transcribed");
    let events = sink.0.lock().unwrap();
    assert!(
        !events.iter().any(|e| matches!(e, EngineEvent::Segment(_))),
        "a cancelled wait is not a failed segment"
    );
    assert!(archive::list_items(&dir.path().join("archive")).is_empty());
    assert!(gate.is_pending(), "the dictation itself is untouched");
}

/// #157, through the real `session::run_request_with` (#158 finding 9): a
/// session started with a language and a cleanup level from New transcribes
/// and cleans with them and records the language in the frontmatter; one
/// without overrides uses the dictation settings. The app's shared settings
/// are read once at the start and never written: a change the user makes
/// mid-run neither reaches the run nor gets overwritten by it.
#[test]
fn per_run_options_drive_stt_and_cleanup_without_touching_settings() {
    use crate::settings::{CleanupLevel, Settings};
    use crate::state::AppPaths;
    use session::{run_request_with, Request, RunOptions};

    let dictation = Settings {
        language: "it".into(),
        cleanup_level: CleanupLevel::Light,
        ..Default::default()
    };
    // What the user switches to in Settings while the run is going.
    let changed_mid_run = Settings {
        language: "fr".into(),
        cleanup_level: CleanupLevel::None,
        ..dictation.clone()
    };

    let run_with = |options: RunOptions| {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("appdata");
        let paths = AppPaths {
            settings_file: dir.path().join("settings.json"),
            models_dir: data.join("models"),
            history_file: data.join("history.jsonl"),
            stats_file: data.join("stats.json"),
            archive_index: data.join("index.sqlite"),
            documents_dir: Some(dir.path().join("Documents")),
            home_dir: Some(dir.path().to_path_buf()),
        };
        let shared = Mutex::new(dictation.clone());
        let languages = Mutex::new(Vec::<String>::new());
        let levels = Mutex::new(Vec::<CleanupLevel>::new());
        let fake_stt = |samples: &[f32], language: &str| -> Result<TimedTranscript> {
            languages.lock().unwrap().push(language.to_string());
            // The user changes the dictation settings during the run.
            *shared.lock().unwrap() = changed_mid_run.clone();
            Ok(TimedTranscript {
                text: format!("parole {}", samples.len()),
                // The engine "detects" Italian: an explicit language wins.
                language: Some("it".into()),
                ..Default::default()
            })
        };
        let fake_clean = |s: &Settings, _prev: Option<&str>, raw: &str| -> String {
            levels.lock().unwrap().push(s.cleanup_level.clone());
            raw.to_uppercase()
        };
        let req = Request {
            id: 7,
            cancel: Arc::new(AtomicBool::new(false)),
            source: Box::new(VecSource::new(
                bursts(&[(true, 2.0), (false, 2.5), (true, 2.0), (false, 1.0)]),
                Channel::File,
            )),
            policy: Policy::Block { max_queued: 2 },
            defer: false,
            item_type: ItemType::Note,
            title: "Opzioni".into(),
            source_label: "file:test.wav".into(),
            options,
        };
        let r = run_request_with(
            &shared,
            &paths,
            req,
            fake_stt,
            fake_clean,
            |_| Box::new(EnergyDetector::default()),
            Arc::new(VecSink::default()),
        )
        .unwrap();
        assert_eq!(r.segments, 2);
        let archive_dir = dir.path().join("Documents").join("Sussurro");
        let item = archive::read_item(&archive_dir, &r.item_id).unwrap();
        assert!(item
            .segments
            .segments
            .iter()
            .all(|s| s.text == s.raw.to_uppercase()));
        assert_eq!(
            *shared.lock().unwrap(),
            changed_mid_run,
            "the run never writes the shared settings"
        );
        (
            languages.into_inner().unwrap(),
            levels.into_inner().unwrap(),
            item.meta.language,
        )
    };

    // Overrides: every segment uses them; the frontmatter records them.
    let (langs, levels, recorded) = run_with(RunOptions {
        language: Some("en".into()),
        cleanup_level: Some(CleanupLevel::High),
        ..Default::default()
    });
    assert_eq!(langs, ["en", "en"]);
    assert_eq!(levels, [CleanupLevel::High, CleanupLevel::High]);
    assert_eq!(recorded, "en");

    // No overrides (a blank language counts as none): the dictation
    // settings as they were when the run started, not the mid-run change.
    let (langs, levels, recorded) = run_with(RunOptions {
        language: Some("  ".into()),
        ..Default::default()
    });
    assert_eq!(langs, ["it", "it"]);
    assert_eq!(levels, [CleanupLevel::Light, CleanupLevel::Light]);
    assert_eq!(recorded, "it");
}

/// #123: a link run through the real `session::run_link_with` — the WAV is
/// downloaded from a local test server (allowed explicitly), transcribed
/// into a `transcription` item whose source is the link and whose title
/// comes from the link, and the temporary download is gone afterwards.
/// Refused links (a local host not allowed, a platform link without
/// yt-dlp) end in `engine-error` with nothing in the archive.
#[test]
fn a_link_run_downloads_transcribes_and_cleans_up() {
    use crate::settings::Settings;
    use crate::sources::url::direct::tests::serve;
    use crate::sources::url::{self, Via};
    use crate::state::AppPaths;
    use session::{link_temp_dir, run_link_with, LinkRequest, RunOptions};

    let audio_dir = tempfile::tempdir().unwrap();
    let wav_path = audio_dir.path().join("talk.wav");
    crate::audio::decode::write_wav_i16(
        &wav_path,
        16_000,
        1,
        &bursts(&[(true, 2.0), (false, 2.5), (true, 2.0), (false, 1.0)]),
    );
    let wav = std::fs::read(&wav_path).unwrap();
    let srv = serve(vec![(
        "/talks/Keynote%202026.wav",
        (200, vec![("Content-Type", "audio/wav".into())], wav),
    )]);

    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("appdata");
    let paths = AppPaths {
        settings_file: dir.path().join("settings.json"),
        models_dir: data.join("models"),
        history_file: data.join("history.jsonl"),
        stats_file: data.join("stats.json"),
        archive_index: data.join("index.sqlite"),
        documents_dir: Some(dir.path().join("Documents")),
        home_dir: Some(dir.path().to_path_buf()),
    };
    let shared = Mutex::new(Settings::default());
    let archive_dir = dir.path().join("Documents").join("Sussurro");
    let count_items = || archive::list_items(&archive_dir).len();
    let run = |input: &str, allow_local: bool, sink: Arc<VecSink>| {
        let req = LinkRequest {
            id: 11,
            cancel: Arc::new(AtomicBool::new(false)),
            link: url::parse_link(input).unwrap(),
            allow_local,
            title: String::new(),
            options: RunOptions::default(),
        };
        run_link_with(
            &shared,
            &paths,
            req,
            url::MAX_DOWNLOAD_BYTES,
            &|| None,
            |samples: &[f32], _lang: &str| -> Result<TimedTranscript> {
                Ok(TimedTranscript {
                    text: format!("parole {}", samples.len()),
                    ..Default::default()
                })
            },
            |_: &crate::settings::Settings, _: Option<&str>, raw: &str| raw.to_string(),
            |_| Box::new(EnergyDetector::default()),
            sink,
        )
    };

    let link = format!("{}/talks/Keynote%202026.wav", srv.base);
    let sink = Arc::new(VecSink::default());
    let r = run(&link, true, sink.clone()).unwrap();
    assert_eq!(r.segments, 2);
    let item = archive::read_item(&archive_dir, &r.item_id).unwrap();
    assert_eq!(item.meta.item_type, ItemType::Transcription);
    assert_eq!(item.meta.source, format!("url:{link}"));
    assert_eq!(item.meta.title, "Keynote 2026");
    let events = sink.0.lock().unwrap().clone();
    let first_started = events
        .iter()
        .position(|e| matches!(e, EngineEvent::Started(_)))
        .unwrap();
    let downloads: Vec<&DownloadPayload> = events
        .iter()
        .filter_map(|e| match e {
            EngineEvent::Download(d) => Some(d),
            _ => None,
        })
        .collect();
    assert!(!downloads.is_empty());
    assert!(downloads
        .iter()
        .all(|d| d.via == Via::Direct && d.session_id == 11));
    let last_download = events
        .iter()
        .rposition(|e| matches!(e, EngineEvent::Download(_)))
        .unwrap();
    assert!(
        last_download < first_started,
        "download, then transcription"
    );
    assert!(matches!(events.last(), Some(EngineEvent::Done(_))));
    let temp = link_temp_dir(&paths);
    assert_eq!(
        std::fs::read_dir(&temp).unwrap().count(),
        0,
        "temp file removed"
    );
    let items = count_items();
    assert_eq!(items, 1);

    // Local host not allowed: an error event, nothing written.
    let sink = Arc::new(VecSink::default());
    let e = run(&link, false, sink.clone()).unwrap_err().to_string();
    assert!(e.contains("Allow local network addresses"), "{e}");
    let events = sink.0.lock().unwrap().clone();
    assert!(
        matches!(events.last(), Some(EngineEvent::Error(p)) if p.session_id == 11 && p.item_id.is_none())
            && !events.iter().any(|e| matches!(e, EngineEvent::Started(_))),
        "{events:?}"
    );
    assert_eq!(count_items(), items);

    // A video platform without yt-dlp: install instructions.
    let sink = Arc::new(VecSink::default());
    let e = run("https://www.youtube.com/watch?v=abc", false, sink)
        .unwrap_err()
        .to_string();
    assert!(e.contains("yt-dlp") && e.contains("Install"), "{e}");
    assert_eq!(count_items(), items);
    assert_eq!(std::fs::read_dir(&temp).unwrap().count(), 0);
}

/// #122: a long-form run cleaned on an external profile the user opted in
/// for is recorded in the item's external-send log (the Library marks it);
/// without the opt-in, or with cleanup None for this run, nothing is
/// recorded (cleanup then keeps the raw text, see `cleanup::ollama`).
#[test]
fn external_cleanup_marks_the_item_only_when_it_was_sent() {
    use crate::llm::LlmProfile;
    use crate::settings::{CleanupApi, CleanupLevel, Settings};
    use crate::state::AppPaths;
    use session::{run_request_with, Request, RunOptions};

    let run_with = |opt_in: &str, level: Option<CleanupLevel>| -> Vec<String> {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("appdata");
        let paths = AppPaths {
            settings_file: dir.path().join("settings.json"),
            models_dir: data.join("models"),
            history_file: data.join("history.jsonl"),
            stats_file: data.join("stats.json"),
            archive_index: data.join("index.sqlite"),
            documents_dir: Some(dir.path().join("Documents")),
            home_dir: Some(dir.path().to_path_buf()),
        };
        let mut work = LlmProfile::new("work", "Work", CleanupApi::Openai, "https://api.example.com/v1", "", "gpt");
        work.cleanup_opt_in = opt_in.into();
        let shared = Mutex::new(Settings {
            cleanup_level: CleanupLevel::Light,
            llm_profiles: vec![LlmProfile::default(), work],
            cleanup_profile: "work".into(),
            ..Default::default()
        });
        let req = Request {
            id: 9,
            cancel: Arc::new(AtomicBool::new(false)),
            source: Box::new(VecSource::new(bursts(&[(true, 2.0), (false, 1.0)]), Channel::File)),
            policy: Policy::Block { max_queued: 2 },
            defer: false,
            item_type: ItemType::Transcription,
            title: "Esterno".into(),
            source_label: "file:test.wav".into(),
            options: RunOptions {
                cleanup_level: level,
                ..Default::default()
            },
        };
        let r = run_request_with(
            &shared,
            &paths,
            req,
            |_: &[f32], _: &str| -> Result<TimedTranscript> {
                Ok(TimedTranscript { text: "ciao".into(), ..Default::default() })
            },
            |_: &Settings, _: Option<&str>, raw: &str| raw.to_string(),
            |_| Box::new(EnergyDetector::default()),
            Arc::new(VecSink::default()),
        )
        .unwrap();
        let archive_dir = dir.path().join("Documents").join("Sussurro");
        archive::read_item(&archive_dir, &r.item_id).unwrap().external_hosts
    };

    assert_eq!(run_with("api.example.com", None), ["api.example.com"]);
    assert!(run_with("", None).is_empty(), "no opt-in: nothing sent, nothing recorded");
    assert!(run_with("other.example", None).is_empty(), "an opt-in for another host doesn't count");
    assert!(run_with("api.example.com", Some(CleanupLevel::None)).is_empty(), "cleanup None sends nothing");
}

// ---- speakers (#130) -------------------------------------------------------

/// Bursts of a tone at a voice's level (0.2 → voice 0, 0.4 → voice 1, as
/// the fake embedder reads it) separated by silence (`None`).
fn voiced(pattern: &[(Option<usize>, f32)]) -> Vec<f32> {
    let mut out = Vec::new();
    for &(voice, secs) in pattern {
        let n = (secs * 16_000.0) as usize;
        let amp = voice.map_or(0.0, |v| 0.2 * (v as f32 + 1.0));
        out.extend((0..n).map(|i| amp * ((i as f32) * 0.07).sin()));
    }
    out
}

#[test]
fn a_run_with_speakers_labels_voices_and_stores_embeddings() {
    use crate::speakers::{tracker::tests::fake_loader, SpeakerOptions, Tracker};
    let dir = tempfile::tempdir().unwrap();
    let audio = voiced(&[
        (None, 1.0),
        (Some(0), 6.0),
        (None, 3.0),
        (Some(1), 6.0),
        (None, 3.0),
        (Some(0), 6.0),
        (None, 3.0),
        (Some(1), 6.0),
        (None, 1.0),
    ]);
    let mut j = job(dir.path(), audio, Policy::Block { max_queued: 1 });
    j.meta.item_type = ItemType::Meeting;
    let (load, calls) = fake_loader(5);
    j.speakers = Some(Tracker::new(SpeakerOptions::clustering(&[Channel::File]), load));
    let sink = Arc::new(VecSink::default());
    let mut stt = FakeStt {
        calls: 0,
        fail_on: None,
    };
    let r = run(j, &mut stt, &FakeCleaner::default(), sink.clone()).unwrap();
    assert!(calls.load(Ordering::Relaxed) > 0);

    let item = archive::read_item(&dir.path().join("archive"), &r.item_id).unwrap();
    let who: Vec<&str> = item
        .segments
        .segments
        .iter()
        .map(|s| s.speaker_id.as_deref().unwrap())
        .collect();
    assert_eq!(who, ["voice:1", "voice:2", "voice:1", "voice:2"]);
    let ids: Vec<&str> = item.segments.speakers.iter().map(|s| s.id.as_str()).collect();
    assert_eq!(ids, ["voice:1", "voice:2"]);
    assert_eq!(item.embedded_segments, 4);
    assert!(item.body.contains("] Voice 2:**"), "{}", item.body);

    // Live events carry the speaker but not the embedding.
    let events = sink.0.lock().unwrap();
    let segs: Vec<&Segment> = events
        .iter()
        .filter_map(|e| match e {
            EngineEvent::Segment(p) => Some(&p.segment),
            _ => None,
        })
        .collect();
    assert_eq!(segs.len(), 4);
    assert!(segs.iter().all(|s| s.embedding.is_none() && s.speaker_id.is_some()));
}

#[test]
fn a_run_without_speakers_stores_no_voices() {
    let dir = tempfile::tempdir().unwrap();
    let audio = voiced(&[(None, 1.0), (Some(0), 3.0), (None, 3.0), (Some(1), 3.0), (None, 1.0)]);
    let mut stt = FakeStt {
        calls: 0,
        fail_on: None,
    };
    let r = run(
        job(dir.path(), audio, Policy::Block { max_queued: 1 }),
        &mut stt,
        &FakeCleaner::default(),
        Arc::new(VecSink::default()),
    )
    .unwrap();
    let item = archive::read_item(&dir.path().join("archive"), &r.item_id).unwrap();
    assert!(item.segments.speakers.is_empty());
    assert_eq!(item.embedded_segments, 0);
    assert!(item.segments.segments.iter().all(|s| s.speaker_id.is_none()));
}

#[test]
fn a_short_voice_folds_into_its_neighbour_at_the_end_of_the_run() {
    use crate::speakers::{tracker::tests::fake_loader, SpeakerOptions, Tracker};
    let dir = tempfile::tempdir().unwrap();
    // Voice 2 (level 0.6) speaks 3 s only: at the end it folds into the
    // nearest of the two real voices, and nothing is numbered 3.
    let audio = voiced(&[
        (None, 1.0),
        (Some(0), 12.0),
        (None, 3.0),
        (Some(2), 3.0),
        (None, 3.0),
        (Some(1), 12.0),
        (None, 1.0),
    ]);
    let mut j = job(dir.path(), audio, Policy::Block { max_queued: 1 });
    let (load, _) = fake_loader(5);
    j.speakers = Some(Tracker::new(SpeakerOptions::clustering(&[Channel::File]), load));
    let mut stt = FakeStt {
        calls: 0,
        fail_on: None,
    };
    let r = run(j, &mut stt, &FakeCleaner::default(), Arc::new(VecSink::default())).unwrap();
    let item = archive::read_item(&dir.path().join("archive"), &r.item_id).unwrap();
    let ids: Vec<&str> = item.segments.speakers.iter().map(|s| s.id.as_str()).collect();
    assert_eq!(ids, ["voice:1", "voice:2"]);
    let who: Vec<&str> = item
        .segments
        .segments
        .iter()
        .map(|s| s.speaker_id.as_deref().unwrap())
        .collect();
    assert_eq!(who.first(), Some(&"voice:1"));
    assert_eq!(who.last(), Some(&"voice:2"));
}

/// A two-channel source (#126): `mic` from 0, `remote` joining 1 s late,
/// frames interleaved like the browser sends them.
struct TwoChannels {
    mic: VecSource,
    remote: VecSource,
    remote_offset: u64,
    turn: bool,
}

impl TwoChannels {
    fn next_remote(&mut self) -> Result<Option<Frame>> {
        let off = self.remote_offset;
        Ok(self.remote.next_frame()?.map(|mut f| {
            f.start += off;
            f
        }))
    }
}

impl Source for TwoChannels {
    fn channel(&self) -> Channel {
        Channel::Remote
    }
    fn total_samples(&self) -> Option<u64> {
        None
    }
    fn next_frame(&mut self) -> Result<Option<Frame>> {
        self.turn = !self.turn;
        if self.turn {
            match self.mic.next_frame()? {
                Some(f) => Ok(Some(f)),
                None => self.next_remote(),
            }
        } else {
            match self.next_remote()? {
                Some(f) => Ok(Some(f)),
                None => self.mic.next_frame(),
            }
        }
    }
}

/// Counts forks: each channel must get its own detector state.
struct CountingDetector(Arc<std::sync::atomic::AtomicUsize>);

impl segmenter::SpeechDetector for CountingDetector {
    fn probabilities(&mut self, samples: &[f32]) -> Result<Vec<f32>> {
        EnergyDetector::default().probabilities(samples)
    }
    fn name(&self) -> &'static str {
        "counting"
    }
    fn fork(&self) -> Result<Box<dyn segmenter::SpeechDetector>> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(Box::new(CountingDetector(self.0.clone())))
    }
}

#[test]
fn two_channels_segment_separately_on_one_clock_in_time_order() {
    let dir = tempfile::tempdir().unwrap();
    // Mic: a long 12 s sentence from 0.5 s. Remote (joins at 1 s): a short
    // reply at 2–4 s of the run — it closes first but starts later.
    let mic = bursts(&[(false, 0.5), (true, 12.0), (false, 3.0)]);
    let remote = bursts(&[(false, 1.0), (true, 2.0), (false, 11.5)]);
    let forks = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let mut j = job(dir.path(), Vec::new(), Policy::Spill { max_in_ram: 4 });
    j.source = Box::new(TwoChannels {
        mic: VecSource::new(mic, Channel::Mic),
        remote: VecSource::new(remote, Channel::Remote),
        remote_offset: 16_000,
        turn: false,
    });
    j.detector = Box::new(CountingDetector(forks.clone()));
    j.meta.item_type = ItemType::Meeting;
    let sink = Arc::new(VecSink::default());
    let r = run(
        j,
        &mut FakeStt {
            calls: 0,
            fail_on: None,
        },
        &FakeCleaner::default(),
        sink,
    )
    .unwrap();
    assert_eq!(forks.load(Ordering::SeqCst), 1, "one detector per channel");
    let item = archive::read_item(&dir.path().join("archive"), &r.item_id).unwrap();
    let segs = &item.segments.segments;
    let mic: Vec<_> = segs.iter().filter(|s| s.channel == Channel::Mic).collect();
    let remote: Vec<_> = segs.iter().filter(|s| s.channel == Channel::Remote).collect();
    assert_eq!((mic.len(), remote.len()), (1, 1), "{segs:?}");
    assert!(mic[0].start_ms < 700, "{}", mic[0].start_ms);
    // On the run's clock: 1 s offset + 1 s of silence.
    assert!((1_700..2_100).contains(&remote[0].start_ms), "{}", remote[0].start_ms);
    // The remote reply finished first, but the item is in time order.
    assert_eq!(segs[0].channel, Channel::Mic);
    assert!(segs.windows(2).all(|w| w[0].start_ms <= w[1].start_ms));
    let mut ids: Vec<u32> = segs.iter().map(|s| s.id).collect();
    ids.sort();
    assert_eq!(ids, [0, 1], "ids stay unique");
    // The run lasts as long as the furthest channel (15.5 s), not the sum.
    assert!((15_000..16_500).contains(&r.duration_ms), "{}", r.duration_ms);
}

/// #134: the "Identify voices" run option reaches the engine — through the
/// real `session::run_request_with_speakers`, for every item type × 0.9
/// flag × toggle: voices (and embeddings) exactly when the gating says so,
/// and with the toggle off the speaker model is never even loaded.
#[test]
fn identify_voices_run_option_reaches_the_engine() {
    use crate::settings::Settings;
    use crate::speakers::tracker::tests::fake_loader;
    use crate::state::AppPaths;
    use session::{run_request_with_speakers, Request, RunOptions};

    let run_with = |item_type: ItemType, flag: bool, identify: bool| -> (usize, usize, usize) {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("appdata");
        let paths = AppPaths {
            settings_file: dir.path().join("settings.json"),
            models_dir: data.join("models"),
            history_file: data.join("history.jsonl"),
            stats_file: data.join("stats.json"),
            archive_index: data.join("index.sqlite"),
            documents_dir: Some(dir.path().join("Documents")),
            home_dir: Some(dir.path().to_path_buf()),
        };
        let shared = Mutex::new(Settings {
            meetings_enabled: flag,
            ..Default::default()
        });
        let (load, calls) = fake_loader(5);
        let req = Request {
            id: 11,
            cancel: Arc::new(AtomicBool::new(false)),
            source: Box::new(VecSource::new(
                voiced(&[(None, 1.0), (Some(0), 3.0), (None, 3.0), (Some(1), 3.0), (None, 1.0)]),
                Channel::File,
            )),
            policy: Policy::Block { max_queued: 2 },
            defer: false,
            item_type,
            title: "Voci".into(),
            source_label: "file:voci.wav".into(),
            options: RunOptions {
                identify_voices: identify,
                ..Default::default()
            },
        };
        let r = run_request_with_speakers(
            &shared,
            &paths,
            req,
            |_: &[f32], _: &str| -> Result<TimedTranscript> {
                Ok(TimedTranscript {
                    text: "parole".into(),
                    ..Default::default()
                })
            },
            |_: &Settings, _: Option<&str>, raw: &str| raw.to_string(),
            |_| Box::new(EnergyDetector::default()),
            move |_| load,
            Arc::new(VecSink::default()),
        )
        .unwrap();
        let archive_dir = dir.path().join("Documents").join("Sussurro");
        let item = archive::read_item(&archive_dir, &r.item_id).unwrap();
        (
            item.embedded_segments,
            item.segments.speakers.len(),
            calls.load(Ordering::Relaxed),
        )
    };

    for flag in [false, true] {
        for identify in [false, true] {
            let expected = |t: ItemType| match t {
                ItemType::Note => false,
                ItemType::Transcription => identify,
                ItemType::Meeting => flag,
            };
            for t in [ItemType::Note, ItemType::Transcription, ItemType::Meeting] {
                let (embedded, speakers, calls) = run_with(t, flag, identify);
                let case = format!("{t:?}, flag {flag}, toggle {identify}");
                if expected(t) {
                    assert_eq!(embedded, 2, "{case}");
                    assert!(speakers >= 1, "{case}");
                    assert!(calls > 0, "{case}");
                } else {
                    assert_eq!(
                        (embedded, speakers, calls),
                        (0, 0, 0),
                        "{case}: nothing computed"
                    );
                }
            }
        }
    }
}

// ---- saved audio (#141) ----------------------------------------------------

/// Every `.wav` under `dir`, at any depth.
fn wavs_under(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return out;
    };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            out.extend(wavs_under(&p));
        } else if p.extension().is_some_and(|x| x == "wav") {
            out.push(p);
        }
    }
    out
}

/// Decode a saved WAV with the app's own decoder, as a player would.
fn decode_wav(path: &std::path::Path) -> Vec<f32> {
    let mut s = crate::audio::decode::FileStream::open(path).unwrap();
    let mut out = Vec::new();
    while let Some(chunk) = s.next_chunk().unwrap() {
        out.extend(chunk);
    }
    out
}

fn fake_stt() -> FakeStt {
    FakeStt {
        calls: 0,
        fail_on: None,
    }
}

/// The id of the item a run created (from `engine-started`).
fn started_id(sink: &VecSink) -> String {
    sink.0
        .lock()
        .unwrap()
        .iter()
        .find_map(|e| match e {
            EngineEvent::Started(s) => Some(s.item_id.clone()),
            _ => None,
        })
        .unwrap()
}

/// A mic that fails after `after` samples, like an unplugged device.
struct BreaksAfter {
    inner: VecSource,
    after: usize,
}

impl Source for BreaksAfter {
    fn channel(&self) -> Channel {
        Channel::Mic
    }
    fn total_samples(&self) -> Option<u64> {
        None
    }
    fn next_frame(&mut self) -> Result<Option<Frame>> {
        if self.inner.pos >= self.after {
            anyhow::bail!("microphone unplugged");
        }
        self.inner.next_frame()
    }
}

/// P9: with "Save audio" off (the default), a run writes no audio at all —
/// nothing in the item folder, the archive, or the app data dir.
#[test]
fn save_audio_off_writes_no_audio_anywhere() {
    let dir = tempfile::tempdir().unwrap();
    let audio = bursts(&[(true, 3.0), (false, 2.5)].repeat(3));
    let j = job(dir.path(), audio, Policy::Block { max_queued: 2 });
    assert!(!j.save_audio, "off unless asked");
    let r = run(
        j,
        &mut fake_stt(),
        &FakeCleaner::default(),
        Arc::new(VecSink::default()),
    )
    .unwrap();
    assert!(wavs_under(dir.path()).is_empty(), "{:?}", wavs_under(dir.path()));
    let item = archive::read_item(&dir.path().join("archive"), &r.item_id).unwrap();
    assert!(item.audio.is_empty());
    assert!(!item.meta.extra.contains_key(archive::audio::AUDIO_KEY));
    let listed = archive::list_items(&dir.path().join("archive"));
    assert_eq!(listed[0].audio_bytes, 0);
}

/// The same with a mic-like spilling queue and a failure mid-run: still no
/// audio file, even in the kept interrupted item.
#[test]
fn save_audio_off_writes_nothing_on_a_failed_run_either() {
    let dir = tempfile::tempdir().unwrap();
    let mut j = job(dir.path(), Vec::new(), Policy::Spill { max_in_ram: 1 });
    j.source = Box::new(BreaksAfter {
        inner: VecSource::new(bursts(&[(true, 3.0), (false, 2.5)].repeat(4)), Channel::Mic),
        after: 16_000 * 10,
    });
    assert!(run(
        j,
        &mut fake_stt(),
        &FakeCleaner::default(),
        Arc::new(VecSink::default())
    )
    .is_err());
    assert_eq!(archive::list_items(&dir.path().join("archive")).len(), 1);
    assert!(wavs_under(dir.path()).is_empty());
}

#[test]
fn save_audio_writes_the_runs_audio_as_one_wav_and_lists_it() {
    let dir = tempfile::tempdir().unwrap();
    let audio = bursts(&[
        (false, 0.5),
        (true, 3.0),
        (false, 2.5),
        (true, 2.0),
        (false, 1.0),
    ]);
    let mut j = job(dir.path(), audio.clone(), Policy::Block { max_queued: 2 });
    j.save_audio = true;
    let r = run(
        j,
        &mut fake_stt(),
        &FakeCleaner::default(),
        Arc::new(VecSink::default()),
    )
    .unwrap();
    let archive_dir = dir.path().join("archive");
    let item = archive::read_item(&archive_dir, &r.item_id).unwrap();
    // The folder was renamed after the title: the file moved with it.
    assert!(!r.item_id.ends_with("-untitled"));
    let wav = archive_dir.join(&r.item_id).join("audio.wav");
    assert_eq!(wavs_under(dir.path()), vec![wav.clone()]);
    assert_eq!(
        archive::audio::listed(&item.meta),
        vec!["audio.wav"],
        "recorded in the frontmatter"
    );
    let bytes = 44 + audio.len() as u64 * 2;
    assert_eq!(
        item.audio,
        vec![archive::audio::AudioFile {
            name: "audio.wav".into(),
            bytes
        }]
    );
    assert!(item.folder_bytes > bytes);
    assert_eq!(archive::list_items(&archive_dir)[0].audio_bytes, bytes);
    // Search rows carry it too.
    let hits = archive::with_index(&archive_dir, &dir.path().join("index.sqlite"), |i| {
        i.search("", &Default::default())
    })
    .unwrap();
    assert_eq!(hits[0].audio_bytes, bytes);
    // It plays back what the engine transcribed, sample for sample.
    let back = decode_wav(&wav);
    assert_eq!(back.len(), audio.len());
    assert!(audio.iter().zip(&back).all(|(a, b)| (a - b).abs() < 1e-3));
    // A UI saving stale metadata can't drop (or forge) the list.
    let mut m = item.meta.clone();
    m.extra.remove(archive::audio::AUDIO_KEY);
    m.tags = vec!["x".into()];
    let updated = archive::update_meta(&archive_dir, &r.item_id, &m).unwrap();
    assert_eq!(archive::audio::listed(&updated.meta), vec!["audio.wav"]);
    m.extra.insert(
        archive::audio::AUDIO_KEY.into(),
        serde_json::json!(["audio-evil.wav"]),
    );
    let updated = archive::update_meta(&archive_dir, &r.item_id, &m).unwrap();
    assert_eq!(archive::audio::listed(&updated.meta), vec!["audio.wav"]);
}

/// Two channels: one mono file each, both starting at the run's t = 0 (the
/// late channel padded with silence), so segment times are file positions.
#[test]
fn save_audio_writes_one_file_per_channel_on_one_clock() {
    let dir = tempfile::tempdir().unwrap();
    let mic = bursts(&[(false, 0.5), (true, 12.0), (false, 3.0)]);
    let remote = bursts(&[(false, 1.0), (true, 2.0), (false, 11.5)]);
    let mut j = job(dir.path(), Vec::new(), Policy::Spill { max_in_ram: 4 });
    j.source = Box::new(TwoChannels {
        mic: VecSource::new(mic.clone(), Channel::Mic),
        remote: VecSource::new(remote.clone(), Channel::Remote),
        remote_offset: 16_000,
        turn: false,
    });
    j.meta.item_type = ItemType::Meeting;
    j.save_audio = true;
    let r = run(
        j,
        &mut fake_stt(),
        &FakeCleaner::default(),
        Arc::new(VecSink::default()),
    )
    .unwrap();
    let folder = dir.path().join("archive").join(&r.item_id);
    let item = archive::read_item(&dir.path().join("archive"), &r.item_id).unwrap();
    assert_eq!(
        archive::audio::listed(&item.meta),
        vec!["audio-mic.wav", "audio-remote.wav"]
    );
    assert_eq!(item.audio.len(), 2);
    assert_eq!(wavs_under(dir.path()).len(), 2);
    let m = decode_wav(&folder.join("audio-mic.wav"));
    assert_eq!(m.len(), mic.len());
    let rem = decode_wav(&folder.join("audio-remote.wav"));
    assert_eq!(rem.len(), 16_000 + remote.len(), "padded to the run's t = 0");
    assert!(rem[..16_000].iter().all(|&s| s == 0.0));
    assert!(remote
        .iter()
        .zip(&rem[16_000..])
        .all(|(a, b)| (a - b).abs() < 1e-3));
    // The remote line's start is where its speech is in its file.
    let line = item
        .segments
        .segments
        .iter()
        .find(|s| s.channel == Channel::Remote)
        .unwrap();
    let at = (line.start_ms * 16) as usize;
    assert!(rem[at..at + 16_000].iter().any(|s| s.abs() > 0.1));
}

/// A cancelled run leaves no audio behind in the archive: its folder,
/// audio included, goes to the trash — never hard-deleted.
#[test]
fn a_cancelled_run_trashes_its_audio_with_the_item() {
    let dir = tempfile::tempdir().unwrap();
    let audio = bursts(&[(true, 3.0), (false, 2.5)].repeat(10));
    let mut j = job(dir.path(), audio, Policy::Block { max_queued: 1 });
    j.save_audio = true;
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
    let sink = Arc::new(VecSink::default());
    assert!(run(
        j,
        &mut CancelAfterFirst(cancel),
        &FakeCleaner::default(),
        sink.clone()
    )
    .is_err());
    let archive_dir = dir.path().join("archive");
    assert!(archive::list_items(&archive_dir).is_empty());
    assert!(wavs_under(dir.path()).is_empty());
    assert!(archive::store::test_trash::contains(
        &archive_dir.join(started_id(&sink))
    ));
}

/// Only silence was said, but the user asked for the audio: the item goes
/// to the trash (with its audio) instead of being removed outright.
#[test]
fn a_run_with_no_speech_trashes_rather_than_removes_saved_audio() {
    let dir = tempfile::tempdir().unwrap();
    let mut j = job(
        dir.path(),
        bursts(&[(false, 5.0)]),
        Policy::Block { max_queued: 2 },
    );
    j.save_audio = true;
    let sink = Arc::new(VecSink::default());
    let err = run(j, &mut fake_stt(), &FakeCleaner::default(), sink.clone()).unwrap_err();
    assert!(format!("{err:#}").contains("no speech"));
    assert!(archive::store::test_trash::contains(
        &dir.path().join("archive").join(started_id(&sink))
    ));
}

/// A run that fails after some segments keeps its item as interrupted —
/// and its audio, finished, listed, playable.
#[test]
fn a_failed_run_keeps_its_audio_with_the_interrupted_item() {
    let dir = tempfile::tempdir().unwrap();
    let mut j = job(dir.path(), Vec::new(), Policy::Spill { max_in_ram: 2 });
    j.source = Box::new(BreaksAfter {
        inner: VecSource::new(bursts(&[(true, 3.0), (false, 2.5)].repeat(4)), Channel::Mic),
        after: 16_000 * 11,
    });
    j.save_audio = true;
    assert!(run(
        j,
        &mut fake_stt(),
        &FakeCleaner::default(),
        Arc::new(VecSink::default())
    )
    .is_err());
    let archive_dir = dir.path().join("archive");
    let items = archive::list_items(&archive_dir);
    assert_eq!(items.len(), 1);
    assert!(items[0].interrupted);
    let item = archive::read_item(&archive_dir, &items[0].id).unwrap();
    assert_eq!(archive::audio::listed(&item.meta), vec!["audio.wav"]);
    let wav = archive_dir.join(&items[0].id).join("audio.wav");
    assert_eq!(decode_wav(&wav).len(), 16_000 * 11);
}

/// #153 + #141: a crash leaves a WAV whose header was patched only up to
/// some earlier point and whose buffer never reached the disk; the startup
/// recovery repairs it, names it and lists it on the interrupted item.
#[test]
fn crash_recovery_repairs_and_lists_the_saved_audio() {
    let dir = tempfile::tempdir().unwrap();
    let archive_dir = dir.path().join("archive");
    let journal = dir.path().join(checkpoint::JOURNAL_FILE);
    let meta = ItemMeta {
        item_type: ItemType::Note,
        title: "Crash".into(),
        date: "2026-09-24T10:00:00+02:00".into(),
        source: "mic".into(),
        ..Default::default()
    };
    let mut live = checkpoint::LiveItem::begin(&archive_dir, &meta, Some(&journal)).unwrap();
    let folder = archive_dir.join(live.id());
    let mut out = audio_out::AudioOut::new(&folder);
    let mut clock = crate::sources::Clock::default();
    // 13 s: the header is patched at 10 s, the next 3 s overflow the
    // 64 KB buffer, so part of them reaches the disk unpatched.
    let speech = bursts(&[(true, 13.0)]);
    for chunk in speech.chunks(4_000) {
        out.push(&clock.stamp(Channel::Mic, chunk.to_vec()));
    }
    assert_eq!(out.channels(), [Channel::Mic]);
    live.push(Segment {
        id: 0,
        start_ms: 0,
        end_ms: 12_000,
        raw: "ciao".into(),
        text: "Ciao.".into(),
        ..Default::default()
    });
    // The process dies: no final patch, no rename, the item stays recording.
    std::mem::forget(out);
    drop(live);
    let wav = folder.join("audio-mic.wav");
    let on_disk = std::fs::read(&wav).unwrap();
    let header_says = u32::from_le_bytes(on_disk[40..44].try_into().unwrap()) as u64;
    assert!(
        header_says < on_disk.len() as u64 - 44,
        "the header lags behind what reached the disk"
    );

    let r = checkpoint::recover(
        &journal,
        Some(&archive_dir),
        None,
        &dir.path().join("data"),
    );
    assert_eq!(r.interrupted.len(), 1);
    let item = archive::read_item(&archive_dir, &r.interrupted[0]).unwrap();
    assert!(item.interrupted);
    assert_eq!(archive::audio::listed(&item.meta), vec!["audio.wav"]);
    assert!(!wav.exists());
    let fixed = folder.join("audio.wav");
    let bytes = std::fs::read(&fixed).unwrap();
    let data = u32::from_le_bytes(bytes[40..44].try_into().unwrap()) as u64;
    assert_eq!(data, bytes.len() as u64 - 44, "sizes match the file");
    // Playable: everything that reached the disk (at least the patched part).
    let back = decode_wav(&fixed);
    assert_eq!(back.len() as u64, data / 2);
    assert!(data >= header_says);
}

/// "Delete audio, keep transcript": the audio goes to the trash, the
/// transcript, segments and metadata stay.
#[test]
fn delete_audio_keeps_the_transcript() {
    let dir = tempfile::tempdir().unwrap();
    let mut j = job(
        dir.path(),
        bursts(&[(true, 3.0), (false, 2.0)]),
        Policy::Block { max_queued: 2 },
    );
    j.save_audio = true;
    j.meta.title = "Keep me".into();
    let r = run(
        j,
        &mut fake_stt(),
        &FakeCleaner::default(),
        Arc::new(VecSink::default()),
    )
    .unwrap();
    let archive_dir = dir.path().join("archive");
    let before = archive::read_item(&archive_dir, &r.item_id).unwrap();
    let wav = archive_dir.join(&r.item_id).join("audio.wav");
    assert!(wav.exists());

    assert_eq!(
        archive::audio::delete_audio(&archive_dir, &r.item_id).unwrap(),
        1
    );
    assert!(!wav.exists());
    assert!(archive::store::test_trash::contains(&wav), "to the trash");
    let after = archive::read_item(&archive_dir, &r.item_id).unwrap();
    assert!(after.audio.is_empty());
    assert!(archive::audio::listed(&after.meta).is_empty());
    assert_eq!(after.segments, before.segments);
    assert_eq!(after.body, before.body);
    assert_eq!(after.meta.title, "Keep me");
    assert!(!after.edited_externally);
    assert!(wavs_under(dir.path()).is_empty());
    // Nothing left: a second delete is a no-op.
    assert_eq!(
        archive::audio::delete_audio(&archive_dir, &r.item_id).unwrap(),
        0
    );
}

#[test]
fn delete_audio_is_refused_while_recording() {
    let dir = tempfile::tempdir().unwrap();
    let archive_dir = dir.path().join("archive");
    let meta = ItemMeta {
        title: "Live".into(),
        date: "2026-09-24T10:00:00+02:00".into(),
        ..Default::default()
    };
    let live = checkpoint::LiveItem::begin(&archive_dir, &meta, None).unwrap();
    let wav = archive_dir.join(live.id()).join("audio-mic.wav");
    archive::audio::WavWriter::create(&wav)
        .unwrap()
        .finish()
        .unwrap();
    let err = archive::audio::delete_audio(&archive_dir, live.id()).unwrap_err();
    assert!(err.to_string().contains("still being recorded"), "{err}");
    assert!(wav.exists());
}

/// The size guard at the engine's side: a channel that reaches the cap
/// stops being saved; its file stays a valid WAV.
#[test]
fn a_full_audio_file_stops_saving_and_stays_valid() {
    let dir = tempfile::tempdir().unwrap();
    let folder = dir.path().join("item");
    std::fs::create_dir_all(&folder).unwrap();
    let mut out = audio_out::AudioOut::with_cap(&folder, 32_000);
    let mut clock = crate::sources::Clock::default();
    for _ in 0..10 {
        out.push(&clock.stamp(Channel::File, vec![0.1; 4_000]));
    }
    assert_eq!(out.finish(), vec!["audio.wav"]);
    assert_eq!(decode_wav(&folder.join("audio.wav")).len(), 16_000);
}

/// A device replaying 16 kHz audio in real (fake) time: by wall time `now`
/// it has delivered `audio[..now]`, until it fails at `fails_at`.
struct ScriptedDevice {
    audio: Vec<f32>,
    pos: usize,
    wall: Arc<std::sync::atomic::AtomicU64>,
    fails_at: Option<u64>,
}

impl crate::sources::system::Capture for ScriptedDevice {
    fn take(&mut self) -> Vec<f32> {
        let mut now = self.wall.load(Ordering::SeqCst);
        if let Some(t) = self.fails_at {
            now = now.min(t);
        }
        let end = (now as usize).min(self.audio.len());
        let out = self.audio[self.pos.min(end)..end].to_vec();
        self.pos = end;
        out
    }
    fn failed(&self) -> bool {
        self.fails_at
            .is_some_and(|t| self.wall.load(Ordering::SeqCst) >= t)
    }
    fn stop(&mut self) -> Vec<f32> {
        Vec::new()
    }
}

/// 250 ms polls of fake time; sets `stop` (the user's Stop) at `end`.
struct ScriptedPacer {
    wall: Arc<std::sync::atomic::AtomicU64>,
    end: u64,
    stop: Arc<AtomicBool>,
}

impl crate::sources::system::Pacer for ScriptedPacer {
    fn wait(&mut self) {
        let now = self.wall.fetch_add(4_000, Ordering::SeqCst) + 4_000;
        if now >= self.end {
            self.stop.store(true, Ordering::SeqCst);
        }
    }
    fn now(&self) -> u64 {
        self.wall.load(Ordering::SeqCst)
    }
}

/// #139 end to end through the real `session` path: *System audio + mic*
/// becomes a `meeting` item with `source: system`; the mic channel is
/// "You", the system channel is clustered into "Voice N"; the system device
/// disappearing mid-session ends its channel with an `engine-warning` and
/// the item keeps everything. Without the 0.9 flag, no voices at all.
#[test]
fn system_audio_session_labels_you_and_voices_and_survives_a_lost_device() {
    use crate::settings::Settings;
    use crate::sources::system::SystemSource;
    use crate::speakers::tracker::tests::fake_loader;
    use crate::state::AppPaths;
    use session::{run_request_with_speakers, system_request, RunOptions};

    for flag in [true, false] {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("appdata");
        let paths = AppPaths {
            settings_file: dir.path().join("settings.json"),
            models_dir: data.join("models"),
            history_file: data.join("history.jsonl"),
            stats_file: data.join("stats.json"),
            archive_index: data.join("index.sqlite"),
            documents_dir: Some(dir.path().join("Documents")),
            home_dir: Some(dir.path().to_path_buf()),
        };
        let shared = Mutex::new(Settings {
            meetings_enabled: flag,
            ..Default::default()
        });
        let wall: Arc<std::sync::atomic::AtomicU64> = Arc::default();
        let stop = Arc::new(AtomicBool::new(false));
        // The user speaks at 1–4 s and 11–14 s; the others (system audio)
        // at 5–9 s, then the loopback device disappears at 10 s.
        let mic = voiced(&[(None, 1.0), (Some(0), 3.0), (None, 7.0), (Some(0), 3.0), (None, 2.0)]);
        let system = voiced(&[(None, 5.0), (Some(1), 4.0), (None, 7.0)]);
        let source = SystemSource::from_parts(
            Box::new(ScriptedDevice {
                audio: mic,
                pos: 0,
                wall: wall.clone(),
                fails_at: None,
            }),
            Box::new(ScriptedDevice {
                audio: system,
                pos: 0,
                wall: wall.clone(),
                fails_at: Some(10 * 16_000),
            }),
            Box::new(ScriptedPacer {
                wall: wall.clone(),
                end: 16 * 16_000,
                stop: stop.clone(),
            }),
            stop,
        );
        let req = system_request(
            21,
            Arc::new(AtomicBool::new(false)),
            source,
            "Standup".into(),
            false,
            RunOptions::default(),
        );
        let (load, _) = fake_loader(5);
        let sink = Arc::new(VecSink::default());
        let r = run_request_with_speakers(
            &shared,
            &paths,
            req,
            |_: &[f32], _: &str| -> Result<TimedTranscript> {
                Ok(TimedTranscript {
                    text: "parole".into(),
                    ..Default::default()
                })
            },
            |_: &Settings, _: Option<&str>, raw: &str| raw.to_string(),
            |_| Box::new(EnergyDetector::default()),
            move |_| load,
            sink.clone(),
        )
        .unwrap();
        let archive_dir = dir.path().join("Documents").join("Sussurro");
        let item = archive::read_item(&archive_dir, &r.item_id).unwrap();
        assert_eq!(item.meta.item_type, ItemType::Meeting);
        assert_eq!(item.meta.source, "system");
        let segs = &item.segments.segments;
        let mic: Vec<_> = segs.iter().filter(|s| s.channel == Channel::Mic).collect();
        let sys: Vec<_> = segs.iter().filter(|s| s.channel == Channel::System).collect();
        assert_eq!((mic.len(), sys.len()), (2, 1), "flag {flag}: {segs:?}");
        // Time order across the channels, on one clock.
        assert!(segs.windows(2).all(|w| w[0].start_ms <= w[1].start_ms));
        assert!((4_500..5_500).contains(&sys[0].start_ms), "{}", sys[0].start_ms);
        assert!(mic[1].start_ms > 10_000, "the mic went on after the loss");
        let warnings: Vec<String> = sink
            .0
            .lock()
            .unwrap()
            .iter()
            .filter_map(|e| match e {
                EngineEvent::Warning(w) => {
                    assert_eq!(w.session_id, 21);
                    assert_eq!(e.name(), "engine-warning");
                    Some(w.message.clone())
                }
                _ => None,
            })
            .collect();
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(warnings[0].contains("system audio device stopped delivering audio at 0:10"));
        if flag {
            assert!(mic.iter().all(|s| s.speaker_id.as_deref() == Some("you")));
            assert!(mic.iter().all(|s| s.embedding.is_none()), "You is never embedded");
            assert!(sys[0].speaker_id.as_deref().is_some_and(|id| id.starts_with("voice:")));
            assert!(item.segments.speakers.iter().any(|s| s.id == "you"));
        } else {
            assert!(segs.iter().all(|s| s.speaker_id.is_none()));
            assert!(item.segments.speakers.is_empty());
        }
    }
}

/// A device whose clock runs 1 % slow: by wall time `now` it has delivered
/// `audio[..now * 99 / 100]`, so the system audio drifts behind the mic.
struct SlowDevice {
    audio: Vec<f32>,
    pos: usize,
    wall: Arc<std::sync::atomic::AtomicU64>,
}

impl crate::sources::system::Capture for SlowDevice {
    fn take(&mut self) -> Vec<f32> {
        let now = self.wall.load(Ordering::SeqCst) * 99 / 100;
        let end = (now as usize).min(self.audio.len());
        let out = self.audio[self.pos.min(end)..end].to_vec();
        self.pos = end;
        out
    }
    fn failed(&self) -> bool {
        false
    }
    fn stop(&mut self) -> Vec<f32> {
        Vec::new()
    }
}

/// Passes a source through, keeping a copy of every frame it hands out.
struct Recorded {
    inner: Box<dyn Source>,
    frames: Arc<Mutex<Vec<Frame>>>,
}

impl Source for Recorded {
    fn channel(&self) -> Channel {
        self.inner.channel()
    }
    fn total_samples(&self) -> Option<u64> {
        self.inner.total_samples()
    }
    fn next_frame(&mut self) -> Result<Option<Frame>> {
        let f = self.inner.next_frame()?;
        if let Some(f) = &f {
            self.frames.lock().unwrap().push(f.clone());
        }
        Ok(f)
    }
    fn take_warnings(&mut self) -> Vec<String> {
        self.inner.take_warnings()
    }
}

/// #141 × #139: *System audio + mic* with Save audio on writes
/// `audio-mic.wav` + `audio-system.wav`. The system device's clock drifts,
/// the source realigns it with silence mid-session, and each file still
/// holds every sample at its position on the run's clock — the clock the
/// segment timestamps use — silence included.
#[test]
fn system_audio_with_save_audio_keeps_both_files_aligned_through_a_realignment() {
    use crate::sources::system::SystemSource;
    let dir = tempfile::tempdir().unwrap();
    let wall: Arc<std::sync::atomic::AtomicU64> = Arc::default();
    let stop = Arc::new(AtomicBool::new(false));
    let mic = voiced(&[(None, 1.0), (Some(0), 4.0), (None, 36.0)]);
    let system = voiced(&[
        (None, 3.0),
        (Some(1), 4.0),
        (None, 22.0),
        (Some(1), 5.0),
        (None, 7.0),
    ]);
    let source = SystemSource::from_parts(
        Box::new(ScriptedDevice {
            audio: mic,
            pos: 0,
            wall: wall.clone(),
            fails_at: None,
        }),
        Box::new(SlowDevice {
            audio: system,
            pos: 0,
            wall: wall.clone(),
        }),
        Box::new(ScriptedPacer {
            wall: wall.clone(),
            end: 40 * 16_000,
            stop: stop.clone(),
        }),
        stop,
    );
    let frames: Arc<Mutex<Vec<Frame>>> = Arc::default();
    let mut j = job(dir.path(), Vec::new(), Policy::Spill { max_in_ram: 4 });
    j.source = Box::new(Recorded {
        inner: Box::new(source),
        frames: frames.clone(),
    });
    j.meta.item_type = ItemType::Meeting;
    j.meta.source = "system".into();
    j.save_audio = true;
    let sink = Arc::new(VecSink::default());
    let r = run(j, &mut fake_stt(), &FakeCleaner::default(), sink.clone()).unwrap();

    let warned = sink.0.lock().unwrap().iter().any(|e| {
        matches!(e, EngineEvent::Warning(w) if w.message.contains("drifted"))
    });
    assert!(warned, "the system channel was realigned");
    let archive_dir = dir.path().join("archive");
    let folder = archive_dir.join(&r.item_id);
    let item = archive::read_item(&archive_dir, &r.item_id).unwrap();
    assert_eq!(
        archive::audio::listed(&item.meta),
        vec!["audio-mic.wav", "audio-system.wav"]
    );
    assert_eq!(wavs_under(dir.path()).len(), 2);

    // Each file = that channel's frames laid out at their `start`.
    let frames = frames.lock().unwrap();
    for (channel, name) in [
        (Channel::Mic, "audio-mic.wav"),
        (Channel::System, "audio-system.wav"),
    ] {
        let mine: Vec<&Frame> = frames.iter().filter(|f| f.channel == channel).collect();
        let end = mine
            .iter()
            .map(|f| f.start + f.samples.len() as u64)
            .max()
            .unwrap() as usize;
        let mut expected = vec![0.0f32; end];
        for f in &mine {
            let at = f.start as usize;
            expected[at..at + f.samples.len()].copy_from_slice(&f.samples);
        }
        let got = decode_wav(&folder.join(name));
        assert_eq!(got.len(), end, "{name}");
        assert!(
            expected.iter().zip(&got).all(|(a, b)| (a - b).abs() < 1e-3),
            "{name} matches the run's clock"
        );
    }
    // The realignment silence is in the system file.
    let silences = frames
        .iter()
        .filter(|f| f.channel == Channel::System && f.samples.iter().all(|&s| s == 0.0))
        .count();
    assert!(silences > 0);

    // And the lines point at their speech in their own file, before and
    // after the realignment.
    let sys = decode_wav(&folder.join("audio-system.wav"));
    let lines: Vec<_> = item
        .segments
        .segments
        .iter()
        .filter(|s| s.channel == Channel::System)
        .collect();
    assert_eq!(lines.len(), 2, "{:?}", item.segments.segments);
    for s in lines {
        let a = (s.start_ms * 16) as usize;
        let b = (s.end_ms * 16) as usize;
        let loud = sys[a..b.min(sys.len())].iter().filter(|x| x.abs() > 0.1).count();
        assert!(loud > (b - a) / 2, "line {}–{} ms is mostly speech", s.start_ms, s.end_ms);
    }
}
