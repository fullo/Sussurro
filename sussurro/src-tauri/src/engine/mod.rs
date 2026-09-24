//! Long-form engine (plan §4.2, #113): any [`Source`] → VAD segments (end of
//! speech or a 30 s cap) → queue → STT with word timings → chunked cleanup
//! (each segment with the previous one as context) → segment events → one
//! archive item. Hotkey dictation does not go through here.
//!
//! Threads per run: an ingest thread (source → [`segmenter`] → [`queue`])
//! and the worker (the caller's thread: STT → cleanup → events → archive).
//!
//! **Memory bound** (60-minute file or session): the source holds one
//! decoded packet (files) or one 250 ms poll (mic); the aligner < 1 s; the
//! segmenter one open segment ≤ 30 s plus 224 ms pre-roll; the queue at most
//! [`FILE_MAX_QUEUED`] segments (files block the decoder) or
//! [`MIC_MAX_IN_RAM`] segments in RAM with the rest spilled to disk (mic);
//! plus the one segment being transcribed. At 16 kHz f32 (64 KB/s), 30 s is
//! 1.92 MB, so audio in RAM stays under ~(1 + 3 + 1) × 1.92 ≈ 10 MB (file)
//! or ~(1 + 4 + 1) × 1.92 ≈ 12 MB (mic), whatever the length. The per-segment
//! text and word timings (≈1 MB per hour) are kept until the item is written.

pub mod queue;
pub mod segmenter;
pub mod session;
pub mod timing;
pub mod vad;

use crate::archive::{self, Channel, ItemMeta, ItemType, Segment, SegmentsFile};
use crate::sources::Source;
use crate::stt::TimedTranscript;
use anyhow::{anyhow, Result};
use queue::{Backlog, Policy, SegmentQueue};
use segmenter::{
    samples_to_ms, FrameAligner, SegmentAudio, Segmenter, SegmenterParams, SpeechDetector, FRAME,
};
use serde::Serialize;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Files: segments waiting for STT before the decoder pauses.
pub const FILE_MAX_QUEUED: usize = 3;
/// Mic: segments kept in RAM before spilling to the disk spool.
pub const MIC_MAX_IN_RAM: usize = 4;
/// VAD batch: frames accumulated per detector call (~1 s).
const VAD_BATCH_FRAMES: usize = 32;
/// Minimum wall time between two progress events from the ingest thread.
const PROGRESS_EVERY: Duration = Duration::from_secs(1);

/// Transcribes one segment (16 kHz mono) with word timings relative to its
/// start. The app implementation shares the dictation's transcriber.
pub trait SegmentStt: Send {
    fn transcribe(&mut self, samples: &[f32]) -> Result<TimedTranscript>;
}

/// Cleans one segment with the previous segment's cleaned text as context.
/// Must never fail: on any problem it returns `raw`.
pub trait Cleaner: Send {
    fn clean(&self, previous: Option<&str>, raw: &str) -> String;
}

/// Receives the engine's events (Tauri in the app, a Vec in tests).
pub trait EngineSink: Send + Sync {
    fn emit(&self, event: &EngineEvent);
}

// ---- events ----------------------------------------------------------------

/// `engine-progress`: how far the engine is and how far behind the source.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ProgressPayload {
    pub session_id: u64,
    /// Audio fully handled (transcribed, or skipped as silence), seconds.
    pub processed_s: f64,
    /// Audio read from the source so far, seconds.
    pub ingested_s: f64,
    /// Known total (files) or what was ingested so far (mic), seconds.
    pub total_s: f64,
    /// `ingested_s - processed_s`: the backlog.
    pub backlog_s: f64,
    /// Segments waiting for STT.
    pub queue_len: usize,
    pub segments_done: usize,
}

/// `engine-segment`: one finished segment (raw + cleaned + words).
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct SegmentPayload {
    pub session_id: u64,
    pub segment: Segment,
}

/// `engine-done`: the archive item was written.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct DonePayload {
    pub session_id: u64,
    pub item_id: String,
    pub item_type: ItemType,
    pub title: String,
    /// The cleaned transcript as plain text.
    pub text: String,
    pub segments: usize,
    pub duration_s: f64,
}

/// `engine-error`: the run failed or was cancelled; nothing was written.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ErrorPayload {
    pub session_id: u64,
    pub error: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum EngineEvent {
    Progress(ProgressPayload),
    Segment(SegmentPayload),
    Done(DonePayload),
    Error(ErrorPayload),
}

impl EngineEvent {
    /// The Tauri event name.
    pub fn name(&self) -> &'static str {
        match self {
            EngineEvent::Progress(_) => "engine-progress",
            EngineEvent::Segment(_) => "engine-segment",
            EngineEvent::Done(_) => "engine-done",
            EngineEvent::Error(_) => "engine-error",
        }
    }

    /// The JSON payload sent with [`Self::name`].
    pub fn payload(&self) -> serde_json::Value {
        let v = match self {
            EngineEvent::Progress(p) => serde_json::to_value(p),
            EngineEvent::Segment(p) => serde_json::to_value(p),
            EngineEvent::Done(p) => serde_json::to_value(p),
            EngineEvent::Error(p) => serde_json::to_value(p),
        };
        v.unwrap_or(serde_json::Value::Null)
    }
}

fn secs(samples: u64) -> f64 {
    samples as f64 / segmenter::RATE as f64
}

/// Build the progress payload from the engine's counters. Pure.
pub fn progress_payload(
    session_id: u64,
    backlog: &Backlog,
    total: Option<u64>,
    queue_len: usize,
    segments_done: usize,
) -> ProgressPayload {
    let processed = backlog.processed();
    ProgressPayload {
        session_id,
        processed_s: secs(processed),
        ingested_s: secs(backlog.ingested),
        total_s: secs(total.unwrap_or(backlog.ingested).max(backlog.ingested)),
        backlog_s: secs(backlog.behind()),
        queue_len,
        segments_done,
    }
}

// ---- job -------------------------------------------------------------------

/// Everything one run needs besides STT, cleanup and the event sink.
pub struct Job {
    pub session_id: u64,
    pub source: Box<dyn Source>,
    pub detector: Box<dyn SpeechDetector>,
    pub params: SegmenterParams,
    pub policy: Policy,
    /// Spool file for [`Policy::Spill`] (created only if needed).
    pub spool_path: PathBuf,
    /// "Transcribe at the end": hold every segment until the source ends.
    pub defer: bool,
    /// Set to abort the run (nothing is written).
    pub cancel: Arc<AtomicBool>,
    /// Apply the deterministic spoken commands (new line…) before cleanup.
    pub voice_commands: bool,
    pub archive_dir: PathBuf,
    /// Search index to update; `None` skips indexing (tests).
    pub index_db: Option<PathBuf>,
    /// Frontmatter prefilled by the caller: type, title (may be empty),
    /// source, language, engine, date. Duration and a missing title or
    /// language are filled in at the end.
    pub meta: ItemMeta,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RunResult {
    pub item_id: String,
    pub meta: ItemMeta,
    pub text: String,
    pub segments: usize,
    pub duration_ms: u64,
}

/// Run a job to completion on the calling thread (the ingest thread is
/// spawned and joined here). Emits `engine-done` or `engine-error`.
pub fn run(
    job: Job,
    stt: &mut dyn SegmentStt,
    cleaner: &dyn Cleaner,
    sink: Arc<dyn EngineSink>,
) -> Result<RunResult> {
    let session_id = job.session_id;
    let result = run_inner(job, stt, cleaner, sink.clone());
    match &result {
        Ok(r) => sink.emit(&EngineEvent::Done(DonePayload {
            session_id,
            item_id: r.item_id.clone(),
            item_type: r.meta.item_type,
            title: r.meta.title.clone(),
            text: r.text.clone(),
            segments: r.segments,
            duration_s: r.duration_ms as f64 / 1000.0,
        })),
        Err(e) => sink.emit(&EngineEvent::Error(ErrorPayload {
            session_id,
            error: format!("{e:#}"),
        })),
    }
    result
}

struct Shared {
    queue: SegmentQueue,
    backlog: Mutex<Backlog>,
    total: Option<u64>,
    segments_done: std::sync::atomic::AtomicUsize,
}

impl Shared {
    fn progress(&self, session_id: u64) -> EngineEvent {
        let backlog = *self.backlog.lock().unwrap();
        EngineEvent::Progress(progress_payload(
            session_id,
            &backlog,
            self.total,
            self.queue.len(),
            self.segments_done.load(Ordering::Relaxed),
        ))
    }

    fn sync_queue(&self) {
        let oldest = self.queue.oldest_start();
        self.backlog.lock().unwrap().oldest_queued = oldest;
    }
}

fn run_inner(
    job: Job,
    stt: &mut dyn SegmentStt,
    cleaner: &dyn Cleaner,
    sink: Arc<dyn EngineSink>,
) -> Result<RunResult> {
    let Job {
        session_id,
        source,
        detector,
        params,
        policy,
        spool_path,
        defer,
        cancel,
        voice_commands,
        archive_dir,
        index_db,
        meta,
    } = job;
    let channel = source.channel();
    let shared = Arc::new(Shared {
        queue: SegmentQueue::new(policy, spool_path),
        backlog: Mutex::new(Backlog::default()),
        total: source.total_samples(),
        segments_done: Default::default(),
    });

    let ingest = {
        let shared = shared.clone();
        let cancel = cancel.clone();
        let sink = sink.clone();
        std::thread::spawn(move || {
            let r = ingest(
                source, detector, params, &shared, &cancel, &*sink, session_id,
            );
            if r.is_err() {
                shared.queue.abort();
            } else {
                shared.queue.close();
            }
            r
        })
    };

    let worked = work(
        &shared,
        stt,
        cleaner,
        &*sink,
        session_id,
        channel,
        defer,
        voice_commands,
        &cancel,
    );
    if worked.is_err() || cancel.load(Ordering::Relaxed) {
        // Stop the source (a mic would keep recording) and unblock it (a
        // file source may be waiting for room in the queue).
        cancel.store(true, Ordering::Relaxed);
        shared.queue.abort();
    }
    let ingested = ingest
        .join()
        .map_err(|_| anyhow!("the ingest thread panicked"))?;
    let (segments, detected) = worked?;
    ingested?;
    // Final state: everything ingested is processed, nothing queued.
    sink.emit(&shared.progress(session_id));
    if cancel.load(Ordering::Relaxed) {
        anyhow::bail!("cancelled");
    }
    if segments.is_empty() {
        anyhow::bail!("no speech found in the audio");
    }

    let ingested_samples = shared.backlog.lock().unwrap().ingested;
    let duration_ms =
        samples_to_ms(ingested_samples).max(segments.last().map(|s| s.end_ms).unwrap_or(0));
    let meta = finalize_meta(meta, &segments, duration_ms, detected.as_deref());
    let text = transcript_text(&segments);
    let file = SegmentsFile {
        segments,
        ..Default::default()
    };
    let n = file.segments.len();
    let item_id = archive::create_item(&archive_dir, &meta, &file)?;
    if let Some(db) = index_db {
        // The files are written; an index failure only delays search until
        // the next sync.
        if let Err(e) =
            archive::Index::open(&archive_dir, &db).and_then(|mut i| i.index_item(&item_id))
        {
            eprintln!("archive index: update failed ({e:#})");
        }
    }
    Ok(RunResult {
        item_id,
        meta,
        text,
        segments: n,
        duration_ms,
    })
}

/// Source → aligner → detector → segmenter → queue.
fn ingest(
    mut source: Box<dyn Source>,
    mut detector: Box<dyn SpeechDetector>,
    params: SegmenterParams,
    shared: &Shared,
    cancel: &AtomicBool,
    sink: &dyn EngineSink,
    session_id: u64,
) -> Result<()> {
    let mut seg = Segmenter::new(params);
    let mut aligner = FrameAligner::default();
    let mut last_progress = Instant::now();

    // Returns false when the queue was aborted (the worker failed).
    let mut feed = |block: Vec<f32>, seg: &mut Segmenter| -> Result<bool> {
        let probs = detector.probabilities(&block)?;
        for (frame, p) in block.chunks(FRAME).zip(probs) {
            for s in seg.push(frame, p) {
                if !enqueue(shared, s)? {
                    return Ok(false);
                }
            }
        }
        shared.backlog.lock().unwrap().open_start = seg.open_start();
        Ok(true)
    };

    while let Some(frame) = source.next_frame()? {
        if cancel.load(Ordering::Relaxed) {
            anyhow::bail!("cancelled");
        }
        shared.backlog.lock().unwrap().ingested += frame.samples.len() as u64;
        aligner.push(&frame.samples);
        while let Some(block) = aligner.take(VAD_BATCH_FRAMES) {
            if !feed(block, &mut seg)? {
                return Ok(());
            }
        }
        if last_progress.elapsed() >= PROGRESS_EVERY {
            last_progress = Instant::now();
            sink.emit(&shared.progress(session_id));
        }
    }
    if let Some(rest) = aligner.take_rest() {
        if !feed(rest, &mut seg)? {
            return Ok(());
        }
    }
    if let Some(s) = seg.finish() {
        enqueue(shared, s)?;
    }
    shared.backlog.lock().unwrap().open_start = None;
    Ok(())
}

fn enqueue(shared: &Shared, s: SegmentAudio) -> Result<bool> {
    let pushed = shared.queue.push(s)?;
    shared.sync_queue();
    Ok(pushed)
}

/// Queue → STT → timings → chunked cleanup → events. Returns the segments
/// and the language the engine detected, if any.
#[allow(clippy::too_many_arguments)]
fn work(
    shared: &Shared,
    stt: &mut dyn SegmentStt,
    cleaner: &dyn Cleaner,
    sink: &dyn EngineSink,
    session_id: u64,
    channel: Channel,
    defer: bool,
    voice_commands: bool,
    cancel: &AtomicBool,
) -> Result<(Vec<Segment>, Option<String>)> {
    let mut segments: Vec<Segment> = Vec::new();
    let mut detected: Option<String> = None;
    while let Some(audio) = shared.queue.pop(defer)? {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        {
            let mut b = shared.backlog.lock().unwrap();
            b.in_flight = Some(audio.start);
            b.oldest_queued = shared.queue.oldest_start();
        }
        let transcript = stt.transcribe(&audio.samples)?;
        if detected.is_none() {
            detected = transcript.language.clone().filter(|l| !l.is_empty());
        }
        let previous = segments.last().map(|s| s.text.clone());
        if let Some(segment) = build_segment(
            segments.len() as u32,
            channel,
            &audio,
            transcript,
            previous.as_deref(),
            voice_commands,
            cleaner,
        ) {
            sink.emit(&EngineEvent::Segment(SegmentPayload {
                session_id,
                segment: segment.clone(),
            }));
            segments.push(segment);
        }
        shared.backlog.lock().unwrap().in_flight = None;
        shared.segments_done.fetch_add(1, Ordering::Relaxed);
        sink.emit(&shared.progress(session_id));
    }
    Ok((segments, detected))
}

/// Turn one transcribed segment into an archive [`Segment`]: skip
/// non-speech, place (or estimate) word timings, clean with context.
pub fn build_segment(
    id: u32,
    channel: Channel,
    audio: &SegmentAudio,
    transcript: TimedTranscript,
    previous: Option<&str>,
    voice_commands: bool,
    cleaner: &dyn Cleaner,
) -> Option<Segment> {
    let raw = transcript.text.trim().to_string();
    if raw.is_empty() || is_non_speech(&raw) {
        return None;
    }
    let start_ms = samples_to_ms(audio.start);
    let duration_ms = samples_to_ms(audio.samples.len() as u64);
    let (words, words_estimated) = if transcript.words.is_empty() {
        (
            timing::proportional_words(&raw, start_ms, start_ms + duration_ms),
            true,
        )
    } else {
        (
            timing::place_words(&transcript.words, start_ms, duration_ms),
            transcript.words_estimated,
        )
    };
    let input = if voice_commands {
        crate::voice_commands::apply_basic_commands(&raw)
    } else {
        raw.clone()
    };
    let text = cleaner.clean(previous, &input).trim().to_string();
    let text = if text.is_empty() { input } else { text };
    Some(Segment {
        id,
        channel,
        start_ms,
        end_ms: start_ms + duration_ms,
        raw,
        text,
        words,
        words_estimated,
        ..Default::default()
    })
}

/// Whisper's non-speech annotations: `[BLANK_AUDIO]`, `[Music]`,
/// `(upbeat music)`, `*applause*`, or a string of them. Such a segment has
/// no words to keep. Pure.
pub fn is_non_speech(raw: &str) -> bool {
    let mut rest = raw.trim();
    if rest.is_empty() {
        return true;
    }
    while !rest.is_empty() {
        let close = match rest.chars().next() {
            Some('[') => ']',
            Some('(') => ')',
            Some('*') => '*',
            _ => return false,
        };
        let Some(end) = rest[1..].find(close) else {
            return false;
        };
        rest = rest[1 + end + 1..].trim_start_matches(|c: char| c.is_whitespace() || c == '.');
    }
    true
}

/// The cleaned transcript as plain text: segments joined with a space, a
/// blank line where the speaker paused long enough for a new paragraph
/// (same rule as the archive's markdown). Pure.
pub fn transcript_text(segments: &[Segment]) -> String {
    let mut out = String::new();
    let mut last_end: Option<u64> = None;
    for s in segments {
        let t = s.text.trim();
        if t.is_empty() {
            continue;
        }
        if let Some(end) = last_end {
            let gap = s.start_ms.saturating_sub(end);
            out.push_str(if gap >= archive::render::PARAGRAPH_GAP_MS {
                "\n\n"
            } else {
                " "
            });
        }
        out.push_str(t);
        last_end = Some(s.end_ms);
    }
    out
}

/// Title from the first words of the transcript (~8 words, ≤ 60 chars).
pub fn title_from_text(text: &str) -> String {
    let mut title = String::new();
    for w in text.split_whitespace().take(8) {
        if title.chars().count() + w.chars().count() + 1 > 60 {
            break;
        }
        if !title.is_empty() {
            title.push(' ');
        }
        title.push_str(w);
    }
    title
        .trim_end_matches(|c: char| c.is_ascii_punctuation())
        .to_string()
}

/// Fill what is only known at the end: duration, and a title or language
/// the caller left empty (`auto` becomes the detected language). Pure.
pub fn finalize_meta(
    mut meta: ItemMeta,
    segments: &[Segment],
    duration_ms: u64,
    detected_language: Option<&str>,
) -> ItemMeta {
    meta.duration = Some(archive::render::format_timestamp(duration_ms));
    let lang = meta.language.trim();
    if lang.is_empty() || lang.eq_ignore_ascii_case("auto") {
        meta.language = detected_language.unwrap_or("auto").to_string();
    }
    if meta.title.trim().is_empty() {
        let t = title_from_text(&transcript_text(segments));
        meta.title = if t.is_empty() { "Note".to_string() } else { t };
    }
    meta
}

#[cfg(test)]
mod tests;
