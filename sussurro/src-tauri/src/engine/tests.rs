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
    assert_eq!(r.segments, 8);
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
fn stt_failure_reports_an_error_and_writes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    // Long enough that the decoder is blocked on a full queue when STT fails.
    let audio = bursts(&[(true, 3.0), (false, 2.5)].repeat(10));
    let sink = Arc::new(VecSink::default());
    let mut stt = FakeStt {
        calls: 0,
        fail_on: Some(2),
    };
    let err = run(
        job(dir.path(), audio, Policy::Block { max_queued: 1 }),
        &mut stt,
        &FakeCleaner::default(),
        sink.clone(),
    )
    .unwrap_err();
    assert!(format!("{err:#}").contains("model not downloaded"));
    assert!(archive::list_items(&dir.path().join("archive")).is_empty());
    let events = sink.0.lock().unwrap();
    match events.last().unwrap() {
        EngineEvent::Error(e) => assert!(e.error.contains("model not downloaded")),
        other => panic!("last event {other:?}"),
    }
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
    });
    assert_eq!(err.name(), "engine-error");
    assert_eq!(err.payload()["error"], "boom");
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

/// Memory check on a long file (plan §11): run it under `/usr/bin/time -l`
/// (macOS) or `-v` (Linux) and compare the peak RSS with the model alone —
/// the audio in RAM must stay bounded (see the module docs), not grow with
/// the file (a whole-file decode of 60 min is ~230 MB of f32).
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
        "{} segments, {:.0} s of audio in {:.1} s",
        r.segments,
        r.duration_ms as f64 / 1000.0,
        started.elapsed().as_secs_f64()
    );
}
