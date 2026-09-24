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
//! text and word timings (≈1 MB per hour) are kept for the whole run.
//!
//! **Crash safety** (#153, [`checkpoint`]): the archive item is created when
//! the run starts (`status: recording`) and every finished segment is saved
//! into it at once, so a crash keeps what was transcribed; the next app
//! start marks such an item `interrupted`.

pub mod checkpoint;
pub mod identify;
pub mod priority;
pub mod queue;
pub mod segmenter;
pub mod session;
pub mod source_files;
pub mod timing;
pub mod vad;

use crate::archive::{self, Channel, ItemMeta, ItemType, Segment};
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
/// STT failures on the first segments of a run, with no success in
/// between, after which the run stops (the engine cannot transcribe at
/// all). Later failures only mark their segment.
pub const MAX_STT_FAILURES_UP_FRONT: usize = 3;
/// Minimum wall time between two progress events from the ingest thread.
const PROGRESS_EVERY: Duration = Duration::from_secs(1);

/// Transcribes one segment (16 kHz mono) with word timings relative to its
/// start. The app implementation shares the dictation's transcriber.
pub trait SegmentStt: Send {
    fn transcribe(&mut self, samples: &[f32]) -> Result<TimedTranscript>;
}

/// Cleans one segment with the previous segment's cleaned text as context.
/// Must never fail: on any problem it returns `raw`.
pub trait Cleaner: Send + Sync {
    fn clean(&self, previous: Option<&str>, raw: &str) -> String;
}

/// Receives the engine's events (Tauri in the app, a Vec in tests).
pub trait EngineSink: Send + Sync {
    fn emit(&self, event: &EngineEvent);
}

/// Sends every event to each sink in turn: a browser meeting (#126) reports
/// to the app's UI and to the extension's WebSocket.
pub struct TeeSink(pub Vec<Arc<dyn EngineSink>>);

impl EngineSink for TeeSink {
    fn emit(&self, event: &EngineEvent) {
        for s in &self.0 {
            s.emit(event);
        }
    }
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

/// `engine-error`: the run failed or was cancelled. A cancelled run (or one
/// that found no speech) leaves nothing in the archive — what it had
/// transcribed goes to the OS trash, an empty item is removed (#158); a run
/// that failed after some segments were transcribed keeps them as an
/// `interrupted` archive item, named by `item_id` (absent when nothing was
/// kept).
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ErrorPayload {
    pub session_id: u64,
    pub error: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item_id: Option<String>,
}

/// `engine-started`: a run began and its archive item exists (marked
/// `status: recording`, #153). Lets a caller of the blocking
/// `transcribe_file` learn the session id to filter the other events by.
/// `item_id` is provisional: a session started without a title gets a
/// `…-untitled` folder, renamed after the final title — `engine-done`
/// carries the final id.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct StartedPayload {
    pub session_id: u64,
    pub item_id: String,
    pub item_type: ItemType,
    /// As given by the caller; may be empty (filled at the end).
    pub title: String,
    /// `mic` | `file:<name>` | `url:<link>`.
    pub source: String,
}

/// `engine-download`: a link run (#123) is fetching its audio, before
/// `engine-started`. `via` is `direct` or `yt-dlp`; `total_bytes` is absent
/// when the server does not say; `title` is the platform's, once known.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct DownloadPayload {
    pub session_id: u64,
    pub via: crate::sources::url::Via,
    pub downloaded_bytes: u64,
    pub total_bytes: Option<u64>,
    pub title: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum EngineEvent {
    Download(DownloadPayload),
    Started(StartedPayload),
    Progress(ProgressPayload),
    Segment(SegmentPayload),
    Done(DonePayload),
    Error(ErrorPayload),
}

impl EngineEvent {
    /// The Tauri event name.
    pub fn name(&self) -> &'static str {
        match self {
            EngineEvent::Download(_) => "engine-download",
            EngineEvent::Started(_) => "engine-started",
            EngineEvent::Progress(_) => "engine-progress",
            EngineEvent::Segment(_) => "engine-segment",
            EngineEvent::Done(_) => "engine-done",
            EngineEvent::Error(_) => "engine-error",
        }
    }

    /// The JSON payload sent with [`Self::name`].
    pub fn payload(&self) -> serde_json::Value {
        let v = match self {
            EngineEvent::Download(p) => serde_json::to_value(p),
            EngineEvent::Started(p) => serde_json::to_value(p),
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
    /// Set to abort the run (its item leaves the archive: to the OS trash
    /// if anything was transcribed).
    pub cancel: Arc<AtomicBool>,
    /// Apply the deterministic spoken commands (new line…) before cleanup.
    pub voice_commands: bool,
    pub archive_dir: PathBuf,
    /// Search index to update; `None` skips indexing (tests).
    pub index_db: Option<PathBuf>,
    /// Session journal for crash recovery ([`checkpoint`]); `None` skips it
    /// (tests).
    pub journal: Option<PathBuf>,
    /// Frontmatter prefilled by the caller: type, title (may be empty),
    /// source, language, engine, date. Duration and a missing title or
    /// language are filled in at the end.
    pub meta: ItemMeta,
    /// The run's cleanup goes to an external profile the user opted in for
    /// (#122): this entry is recorded in the item's external-send log right
    /// before the first segment is cleaned (once per run), so a run that
    /// then fails or is cancelled is still marked. `None` = cleanup stays
    /// on this machine (or sends nothing).
    pub external_cleanup: Option<archive::external::ExternalSend>,
    /// Speaker labels (#130): the channels clustered into "Voice N" and
    /// the embedder, loaded when the first segment needs it. `None` (the
    /// default) = no speakers, no embeddings.
    pub speakers: Option<crate::speakers::Tracker>,
    /// The subtitles setting is *Always* (P7, #133): write `transcript.srt`
    /// once the item is final (meetings and transcriptions only; a
    /// `transcript.srt` the user edited is kept).
    pub write_subtitles: bool,
}

/// A [`Cleaner`] whose first call records the run's external send in the
/// item's log before anything is sent (#122). If the entry can't be
/// written, no text is sent at all: every segment keeps its raw text.
struct ExternalLogCleaner<'a> {
    inner: &'a dyn Cleaner,
    archive: PathBuf,
    item_id: String,
    entry: archive::external::ExternalSend,
    /// `None` until the first call, then whether the entry was recorded.
    logged: std::sync::Mutex<Option<bool>>,
}

impl Cleaner for ExternalLogCleaner<'_> {
    fn clean(&self, previous: Option<&str>, raw: &str) -> String {
        let allowed = {
            let mut logged = self.logged.lock().unwrap_or_else(|e| e.into_inner());
            *logged.get_or_insert_with(|| {
                match archive::external::record(&self.archive, &self.item_id, &self.entry) {
                    Ok(()) => true,
                    Err(e) => {
                        eprintln!(
                            "engine: could not record the external cleanup of {} — keeping raw text, nothing sent ({e:#})",
                            self.item_id
                        );
                        false
                    }
                }
            })
        };
        if allowed {
            self.inner.clean(previous, raw)
        } else {
            raw.to_string()
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct RunResult {
    pub session_id: u64,
    pub item_id: String,
    pub meta: ItemMeta,
    pub text: String,
    pub segments: usize,
    pub duration_ms: u64,
    /// The segment queue's high-water marks (the memory bound, measured).
    pub queue: queue::QueueStats,
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
    let (result, kept) = run_inner(job, stt, cleaner, sink.clone());
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
            item_id: kept,
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

/// How the capture part of a run ended.
enum Captured {
    /// The source ended (or the mic was stopped) and every segment is done.
    Done {
        worked: Worked,
        ingested: u64,
        queue: queue::QueueStats,
    },
    /// The user cancelled.
    Cancelled,
    Failed(anyhow::Error),
}

/// Returns the result and the id of an archive item kept on failure.
fn run_inner(
    job: Job,
    stt: &mut dyn SegmentStt,
    cleaner: &dyn Cleaner,
    sink: Arc<dyn EngineSink>,
) -> (Result<RunResult>, Option<String>) {
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
        journal,
        meta,
        external_cleanup,
        mut speakers,
        write_subtitles,
    } = job;
    let reindex = |id: &str| {
        if let Some(db) = &index_db {
            // The files are written; an index failure only delays search
            // until the next sync.
            if let Err(e) =
                archive::Index::open(&archive_dir, db).and_then(|mut i| i.index_item(id))
            {
                eprintln!("archive index: update failed ({e:#})");
            }
        }
    };

    // The item exists from the start and receives every segment as it is
    // done (#153): a crash from here on keeps what was transcribed.
    let mut item = match checkpoint::LiveItem::begin(&archive_dir, &meta, journal.as_deref()) {
        Ok(item) => item,
        Err(e) => return (Err(e), None),
    };
    sink.emit(&EngineEvent::Started(StartedPayload {
        session_id,
        item_id: item.id().to_string(),
        item_type: meta.item_type,
        title: meta.title.clone(),
        source: meta.source.clone(),
    }));
    // External cleanup (#122): logged in the item before the first segment
    // is sent, whatever happens to the run afterwards.
    let logging = external_cleanup.map(|entry| ExternalLogCleaner {
        inner: cleaner,
        archive: archive_dir.clone(),
        item_id: item.id().to_string(),
        entry,
        logged: std::sync::Mutex::new(None),
    });
    let cleaner: &dyn Cleaner = match &logging {
        Some(l) => l,
        None => cleaner,
    };
    let captured = capture(
        session_id,
        source,
        detector,
        params,
        policy,
        spool_path,
        defer,
        &cancel,
        voice_commands,
        stt,
        cleaner,
        speakers.as_mut(),
        &sink,
        &mut item,
    );
    let (worked, ingested, queue) = match captured {
        Captured::Cancelled => {
            let kept = item.discard();
            return (Err(anyhow!("cancelled")), kept);
        }
        Captured::Failed(e) => {
            let kept = item.abandon();
            if let Some(id) = &kept {
                reindex(id);
            }
            return (Err(e), kept);
        }
        Captured::Done {
            worked,
            ingested,
            queue,
        } => (worked, ingested, queue),
    };
    let Worked {
        detected,
        stt_error,
    } = worked;
    if !item.has_text() {
        let kept = item.discard();
        // Only failed segments: report why, not "no speech".
        let e = stt_error.unwrap_or_else(|| anyhow!("no speech found in the audio"));
        return (Err(e), kept);
    }

    if speakers.is_some() {
        // The online clustering over-splits: tiny voices fold into the
        // nearest one before the item is finalized (#107).
        item.finalize_voices();
    }
    let duration_ms =
        samples_to_ms(ingested).max(item.segments().last().map(|s| s.end_ms).unwrap_or(0));
    let text = transcript_text(item.segments());
    let n = item.segments().len();
    let placeholder_id = item.id().to_string();
    let finished = item.finish(&meta, |m| {
        finalize_meta_with_text(m, &text, duration_ms, detected.as_deref())
    });
    let (item_id, meta) = match finished {
        Ok(done) => done,
        Err(e) => {
            // `finish` kept the item as interrupted if it could.
            let kept = archive::read_item(&archive_dir, &placeholder_id)
                .ok()
                .map(|_| placeholder_id.clone());
            if let Some(id) = &kept {
                reindex(id);
            }
            return (Err(e), kept);
        }
    };
    if write_subtitles {
        // The transcript is saved; subtitles are a derived extra, so a
        // failure here is logged, not a failed run.
        if let Err(e) = archive::export::refresh_subtitles(&archive_dir, &item_id) {
            eprintln!("engine: transcript.srt of {item_id} not written ({e:#})");
        }
    }
    if item_id != placeholder_id {
        // A search during the session may have indexed the old folder.
        if let Some(db) = &index_db {
            let _ = archive::Index::open(&archive_dir, db)
                .and_then(|mut i| i.remove_from_index(&placeholder_id));
        }
    }
    reindex(&item_id);
    (
        Ok(RunResult {
            session_id,
            item_id,
            meta,
            text,
            segments: n,
            duration_ms,
            queue,
        }),
        None,
    )
}

/// Source → segments → STT → `item`: spawns the ingest thread, runs the
/// worker on this thread, joins.
#[allow(clippy::too_many_arguments)]
fn capture(
    session_id: u64,
    source: Box<dyn Source>,
    detector: Box<dyn SpeechDetector>,
    params: SegmenterParams,
    policy: Policy,
    spool_path: PathBuf,
    defer: bool,
    cancel: &Arc<AtomicBool>,
    voice_commands: bool,
    stt: &mut dyn SegmentStt,
    cleaner: &dyn Cleaner,
    speakers: Option<&mut crate::speakers::Tracker>,
    sink: &Arc<dyn EngineSink>,
    item: &mut checkpoint::LiveItem,
) -> Captured {
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
        speakers,
        &**sink,
        session_id,
        defer,
        voice_commands,
        cancel,
        item,
    );
    let user_cancelled = cancel.load(Ordering::Relaxed);
    if worked.is_err() || user_cancelled {
        // Stop the source (a mic would keep recording) and unblock it (a
        // file source may be waiting for room in the queue).
        cancel.store(true, Ordering::Relaxed);
        shared.queue.abort();
    }
    let Ok(ingested) = ingest.join() else {
        return Captured::Failed(anyhow!("the ingest thread panicked"));
    };
    let worked = match worked {
        Ok(w) => w,
        Err(_) if user_cancelled => return Captured::Cancelled,
        Err(e) => return Captured::Failed(e),
    };
    if let Err(e) = ingested {
        return if cancel.load(Ordering::Relaxed) {
            Captured::Cancelled
        } else {
            Captured::Failed(e)
        };
    }
    // Final state: everything ingested is processed, nothing queued.
    sink.emit(&shared.progress(session_id));
    if cancel.load(Ordering::Relaxed) {
        return Captured::Cancelled;
    }
    let ingested = shared.backlog.lock().unwrap().ingested;
    Captured::Done {
        worked,
        ingested,
        queue: shared.queue.stats(),
    }
}

/// One logical channel's segmentation state. A single-channel source has
/// one lane; a meeting (#126) has one per channel (`mic`, `remote`), each
/// with its own aligner, detector state and segmenter, so a pause on one
/// side never cuts the other side's sentence.
struct Lane {
    channel: Channel,
    aligner: FrameAligner,
    detector: Box<dyn SpeechDetector>,
    seg: Segmenter,
}

impl Lane {
    /// Detector → segmenter → queue for one aligned block. Returns false
    /// when the queue was aborted (the worker failed).
    fn feed(&mut self, block: Vec<f32>, shared: &Shared) -> Result<bool> {
        let probs = self.detector.probabilities(&block)?;
        for (frame, p) in block.chunks(FRAME).zip(probs) {
            for s in self.seg.push(frame, p) {
                if !enqueue(shared, s)? {
                    return Ok(false);
                }
            }
        }
        Ok(true)
    }
}

/// The lane for `channel`, created on its first frame: the run's detector
/// goes to the first lane, later lanes get a [`SpeechDetector::fork`] (the
/// energy detector if that fails). `start` is where the channel's first
/// frame sits on the run's clock (a channel may join late).
fn lane_for<'a>(
    lanes: &'a mut Vec<Lane>,
    spare: &mut Option<Box<dyn SpeechDetector>>,
    params: SegmenterParams,
    channel: Channel,
    start: u64,
) -> &'a mut Lane {
    if let Some(i) = lanes.iter().position(|l| l.channel == channel) {
        return &mut lanes[i];
    }
    let detector = match spare.take() {
        Some(d) => d,
        None => match lanes.first().map(|l| l.detector.fork()) {
            Some(Ok(d)) => d,
            Some(Err(e)) => {
                eprintln!("engine: using the energy segmenter for a second channel ({e:#})");
                Box::new(segmenter::EnergyDetector::default())
            }
            None => Box::new(segmenter::EnergyDetector::default()),
        },
    };
    lanes.push(Lane {
        channel,
        aligner: FrameAligner::default(),
        detector,
        seg: Segmenter::for_channel(params, channel).starting_at(start),
    });
    lanes.last_mut().expect("just pushed")
}

/// Source → per-channel aligner → detector → segmenter → queue. Frames of
/// one channel must be contiguous on the run's clock (a source fills gaps
/// with silence); the first frame of a channel may start after 0.
fn ingest(
    mut source: Box<dyn Source>,
    detector: Box<dyn SpeechDetector>,
    params: SegmenterParams,
    shared: &Shared,
    cancel: &AtomicBool,
    sink: &dyn EngineSink,
    session_id: u64,
) -> Result<()> {
    let mut spare = Some(detector);
    let mut lanes: Vec<Lane> = Vec::new();
    let mut last_progress = Instant::now();
    let sync_open = |lanes: &[Lane]| {
        shared.backlog.lock().unwrap().open_start =
            lanes.iter().filter_map(|l| l.seg.open_start()).min();
    };

    while let Some(frame) = source.next_frame()? {
        if cancel.load(Ordering::Relaxed) {
            anyhow::bail!("cancelled");
        }
        {
            // The run's clock: the furthest any channel has got.
            let mut b = shared.backlog.lock().unwrap();
            b.ingested = b.ingested.max(frame.start + frame.samples.len() as u64);
        }
        let lane = lane_for(&mut lanes, &mut spare, params, frame.channel, frame.start);
        lane.aligner.push(&frame.samples);
        while let Some(block) = lane.aligner.take(VAD_BATCH_FRAMES) {
            if !lane.feed(block, shared)? {
                return Ok(());
            }
        }
        sync_open(&lanes);
        if last_progress.elapsed() >= PROGRESS_EVERY {
            last_progress = Instant::now();
            sink.emit(&shared.progress(session_id));
        }
    }
    for lane in &mut lanes {
        if let Some(rest) = lane.aligner.take_rest() {
            if !lane.feed(rest, shared)? {
                return Ok(());
            }
        }
    }
    for lane in &mut lanes {
        if let Some(s) = lane.seg.finish() {
            enqueue(shared, s)?;
        }
    }
    shared.backlog.lock().unwrap().open_start = None;
    Ok(())
}

fn enqueue(shared: &Shared, s: SegmentAudio) -> Result<bool> {
    let pushed = shared.queue.push(s)?;
    shared.sync_queue();
    Ok(pushed)
}

/// Queue → STT → timings → chunked cleanup → checkpoint into `item` →
/// events. Each segment keeps the channel it was cut from. Returns the language the engine detected, if any.
#[allow(clippy::too_many_arguments)]
fn work(
    shared: &Shared,
    stt: &mut dyn SegmentStt,
    cleaner: &dyn Cleaner,
    mut speakers: Option<&mut crate::speakers::Tracker>,
    sink: &dyn EngineSink,
    session_id: u64,
    defer: bool,
    voice_commands: bool,
    cancel: &AtomicBool,
    item: &mut checkpoint::LiveItem,
) -> Result<Worked> {
    let mut worked = Worked::default();
    let mut transcribed_any = false;
    let mut failures = 0usize;
    while let Some(audio) = shared.queue.pop(defer)? {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        {
            let mut b = shared.backlog.lock().unwrap();
            b.in_flight = Some(audio.start);
            b.oldest_queued = shared.queue.oldest_start();
        }
        let id = item.segments().len() as u32;
        let channel = audio.channel;
        let built = match stt.transcribe(&audio.samples) {
            Ok(transcript) => {
                transcribed_any = true;
                if worked.detected.is_none() {
                    worked.detected = transcript.language.clone().filter(|l| !l.is_empty());
                }
                let previous = item
                    .segments()
                    .iter()
                    .rev()
                    .find(|s| !s.text.trim().is_empty())
                    .map(|s| s.text.clone());
                build_segment(
                    id,
                    channel,
                    &audio,
                    transcript,
                    previous.as_deref(),
                    voice_commands,
                    cleaner,
                )
            }
            // Cancelled while waiting for the model (a dictation had it,
            // #158): not a failed segment — the run ends below.
            Err(_) if cancel.load(Ordering::Relaxed) => break,
            Err(e) => {
                failures += 1;
                // Nothing has ever worked (model missing, broken install):
                // stop now rather than grind through an hour of audio.
                if !transcribed_any && failures >= MAX_STT_FAILURES_UP_FRONT {
                    return Err(e);
                }
                // One bad stretch must not cost the session: keep its time
                // range, marked, and go on (a model switch mid-session can
                // be fixed while the rest keeps coming).
                eprintln!("engine: segment {id} not transcribed ({e:#})");
                let segment = failed_segment(id, channel, &audio, &e);
                worked.stt_error = Some(e);
                Some(segment)
            }
        };
        if let Some(mut segment) = built {
            if let Some(tracker) = speakers.as_deref_mut() {
                if segment.stt_error.is_none() {
                    let labelled = tracker.label(channel, &audio.samples);
                    if let Some(sp) = labelled.new_speaker {
                        item.add_speaker(sp);
                    }
                    segment.speaker_id = labelled.speaker_id;
                    segment.embedding = labelled.embedding;
                }
            }
            // The UI gets the line without its embedding (256 floats only
            // the backend uses).
            let event = EngineEvent::Segment(SegmentPayload {
                session_id,
                segment: Segment {
                    embedding: None,
                    ..segment.clone()
                },
            });
            // On disk before the UI hears about it.
            item.push(segment);
            sink.emit(&event);
        }
        shared.backlog.lock().unwrap().in_flight = None;
        shared.segments_done.fetch_add(1, Ordering::Relaxed);
        sink.emit(&shared.progress(session_id));
    }
    Ok(worked)
}

/// What the worker learned besides the segments (which went to the item).
#[derive(Default)]
struct Worked {
    /// Language the engine detected, if any.
    detected: Option<String>,
    /// The last per-segment STT error, if some segment failed.
    stt_error: Option<anyhow::Error>,
}

/// A segment whose STT failed: its time range, no text, the error.
pub fn failed_segment(
    id: u32,
    channel: Channel,
    audio: &SegmentAudio,
    error: &anyhow::Error,
) -> Segment {
    let start_ms = samples_to_ms(audio.start);
    Segment {
        id,
        channel,
        start_ms,
        end_ms: start_ms + samples_to_ms(audio.samples.len() as u64),
        stt_error: Some(format!("{error:#}")),
        ..Default::default()
    }
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
    meta: ItemMeta,
    segments: &[Segment],
    duration_ms: u64,
    detected_language: Option<&str>,
) -> ItemMeta {
    finalize_meta_with_text(
        meta,
        &transcript_text(segments),
        duration_ms,
        detected_language,
    )
}

/// [`finalize_meta`] with the transcript text already joined. Pure.
pub fn finalize_meta_with_text(
    mut meta: ItemMeta,
    text: &str,
    duration_ms: u64,
    detected_language: Option<&str>,
) -> ItemMeta {
    meta.duration = Some(archive::render::format_timestamp(duration_ms));
    let lang = meta.language.trim();
    if lang.is_empty() || lang.eq_ignore_ascii_case("auto") {
        meta.language = detected_language.unwrap_or("auto").to_string();
    }
    if meta.title.trim().is_empty() {
        let t = title_from_text(text);
        meta.title = if t.is_empty() { "Note".to_string() } else { t };
    }
    meta
}

#[cfg(test)]
mod tests;
