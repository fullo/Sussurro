//! The segment queue between the ingest thread (source → VAD → segmenter)
//! and the STT worker, plus the backlog accounting behind `engine-progress`.
//!
//! Segments queue, never drop (plan §4.2). Two policies keep memory bounded:
//! - [`Policy::Block`] (files): the producer waits while the queue is full,
//!   so decoding never runs more than a few segments ahead of STT.
//! - [`Policy::Spill`] (live mic, which cannot wait): beyond a few segments
//!   in RAM, audio goes to an append-only spool file on disk and is read
//!   back in order. The spool is deleted when the queue is dropped.

use super::segmenter::SegmentAudio;
use anyhow::{Context, Result};
use std::collections::VecDeque;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::PathBuf;
use std::sync::{Condvar, Mutex};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Policy {
    /// Producer blocks while `max_queued` segments are waiting.
    Block { max_queued: usize },
    /// Never block; keep at most `max_in_ram` segments' audio in RAM, spill
    /// the rest to disk.
    Spill { max_in_ram: usize },
}

enum Payload {
    Ram(Vec<f32>),
    Disk { offset: u64, len: usize },
}

struct Queued {
    start: u64,
    len: usize,
    payload: Payload,
}

struct Spool {
    path: PathBuf,
    file: std::fs::File,
    end: u64,
}

impl Drop for Spool {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

#[derive(Default)]
struct Inner {
    items: VecDeque<Queued>,
    in_ram: usize,
    closed: bool,
    aborted: bool,
    spool: Option<Spool>,
}

pub struct SegmentQueue {
    policy: Policy,
    spool_path: PathBuf,
    inner: Mutex<Inner>,
    cv: Condvar,
}

impl SegmentQueue {
    /// `spool_path`: where spilled audio goes (only created if needed).
    pub fn new(policy: Policy, spool_path: PathBuf) -> Self {
        Self {
            policy,
            spool_path,
            inner: Mutex::new(Inner::default()),
            cv: Condvar::new(),
        }
    }

    /// Enqueue a segment. Blocks under [`Policy::Block`] while full.
    /// Returns `Ok(false)` if the queue was aborted (the segment is dropped
    /// because the whole run is being torn down).
    pub fn push(&self, seg: SegmentAudio) -> Result<bool> {
        let mut g = self.inner.lock().unwrap();
        if let Policy::Block { max_queued } = self.policy {
            while !g.aborted && g.items.len() >= max_queued.max(1) {
                g = self.cv.wait(g).unwrap();
            }
        }
        if g.aborted {
            return Ok(false);
        }
        let len = seg.samples.len();
        let payload = match self.policy {
            Policy::Spill { max_in_ram } if g.in_ram >= max_in_ram => {
                let spool = match g.spool.as_mut() {
                    Some(s) => s,
                    None => {
                        if let Some(parent) = self.spool_path.parent() {
                            let _ = std::fs::create_dir_all(parent);
                        }
                        let file = std::fs::OpenOptions::new()
                            .create(true)
                            .truncate(true)
                            .read(true)
                            .write(true)
                            .open(&self.spool_path)
                            .with_context(|| {
                                format!("creating spool {}", self.spool_path.display())
                            })?;
                        g.spool.insert(Spool {
                            path: self.spool_path.clone(),
                            file,
                            end: 0,
                        })
                    }
                };
                let offset = spool.end;
                let mut bytes = Vec::with_capacity(len * 4);
                for s in &seg.samples {
                    bytes.extend_from_slice(&s.to_le_bytes());
                }
                spool.file.seek(SeekFrom::Start(offset))?;
                spool.file.write_all(&bytes).context("writing spool")?;
                spool.end += bytes.len() as u64;
                Payload::Disk { offset, len }
            }
            _ => {
                g.in_ram += 1;
                Payload::Ram(seg.samples)
            }
        };
        g.items.push_back(Queued {
            start: seg.start,
            len,
            payload,
        });
        self.cv.notify_all();
        Ok(true)
    }

    /// Next segment, waiting for one; `None` once closed and drained (or
    /// aborted). With `wait_for_close` (the "transcribe at the end" mode)
    /// nothing is handed out until the producer has closed the queue.
    pub fn pop(&self, wait_for_close: bool) -> Result<Option<SegmentAudio>> {
        let mut g = self.inner.lock().unwrap();
        loop {
            if g.aborted {
                return Ok(None);
            }
            if (!wait_for_close || g.closed) && !g.items.is_empty() {
                break;
            }
            if g.closed && g.items.is_empty() {
                return Ok(None);
            }
            g = self.cv.wait(g).unwrap();
        }
        let item = g.items.pop_front().expect("checked non-empty");
        let samples = match item.payload {
            Payload::Ram(s) => {
                g.in_ram -= 1;
                s
            }
            Payload::Disk { offset, len } => {
                let spool = g.spool.as_mut().context("spool vanished")?;
                spool.file.seek(SeekFrom::Start(offset))?;
                let mut bytes = vec![0u8; len * 4];
                spool.file.read_exact(&mut bytes).context("reading spool")?;
                bytes
                    .chunks_exact(4)
                    .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
                    .collect()
            }
        };
        self.cv.notify_all();
        Ok(Some(SegmentAudio {
            start: item.start,
            samples,
        }))
    }

    /// The producer is done: the worker drains what is left, then stops.
    pub fn close(&self) {
        self.inner.lock().unwrap().closed = true;
        self.cv.notify_all();
    }

    /// Tear down: wake everyone, drop what is queued.
    pub fn abort(&self) {
        let mut g = self.inner.lock().unwrap();
        g.aborted = true;
        g.items.clear();
        g.in_ram = 0;
        self.cv.notify_all();
    }

    pub fn len(&self) -> usize {
        self.inner.lock().unwrap().items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Clock start of the oldest waiting segment.
    pub fn oldest_start(&self) -> Option<u64> {
        self.inner.lock().unwrap().items.front().map(|q| q.start)
    }

    /// Audio waiting in the queue, in samples.
    pub fn queued_samples(&self) -> u64 {
        self.inner
            .lock()
            .unwrap()
            .items
            .iter()
            .map(|q| q.len as u64)
            .sum()
    }

    /// Segments whose audio is held in RAM right now.
    pub fn in_ram(&self) -> usize {
        self.inner.lock().unwrap().in_ram
    }
}

/// Where the engine stands, on the source's sample clock.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Backlog {
    /// Audio read from the source so far.
    pub ingested: u64,
    /// Start of the segment the segmenter is still collecting.
    pub open_start: Option<u64>,
    /// Start of the oldest segment waiting in the queue.
    pub oldest_queued: Option<u64>,
    /// Start of the segment being transcribed right now.
    pub in_flight: Option<u64>,
}

impl Backlog {
    /// Everything before this point is transcribed (or was silence): the
    /// earliest audio still pending anywhere, else all that was ingested.
    pub fn processed(&self) -> u64 {
        [self.open_start, self.oldest_queued, self.in_flight]
            .into_iter()
            .flatten()
            .min()
            .unwrap_or(self.ingested)
            .min(self.ingested)
    }

    /// How far behind the source the engine is.
    pub fn behind(&self) -> u64 {
        self.ingested - self.processed()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn seg(start: u64, n: usize) -> SegmentAudio {
        SegmentAudio {
            start,
            samples: (0..n).map(|i| (start as usize + i) as f32).collect(),
        }
    }

    #[test]
    fn fifo_order_and_close_drains_then_ends() {
        let dir = tempfile::tempdir().unwrap();
        let q = SegmentQueue::new(Policy::Block { max_queued: 4 }, dir.path().join("s"));
        q.push(seg(0, 10)).unwrap();
        q.push(seg(10, 5)).unwrap();
        assert_eq!(q.len(), 2);
        assert_eq!(q.queued_samples(), 15);
        assert_eq!(q.oldest_start(), Some(0));
        q.close();
        assert_eq!(q.pop(false).unwrap().unwrap(), seg(0, 10));
        assert_eq!(q.pop(false).unwrap().unwrap(), seg(10, 5));
        assert!(q.pop(false).unwrap().is_none());
    }

    #[test]
    fn spill_keeps_ram_bounded_and_reads_back_in_order() {
        let dir = tempfile::tempdir().unwrap();
        let spool = dir.path().join("spool.f32");
        let q = SegmentQueue::new(Policy::Spill { max_in_ram: 2 }, spool.clone());
        for i in 0..6 {
            q.push(seg(i * 100, 100)).unwrap();
            assert!(q.in_ram() <= 2);
        }
        assert!(spool.exists());
        assert_eq!(q.len(), 6);
        q.close();
        for i in 0..6 {
            assert_eq!(q.pop(false).unwrap().unwrap(), seg(i * 100, 100));
        }
        assert!(q.pop(false).unwrap().is_none());
        drop(q);
        assert!(!spool.exists(), "spool removed with the queue");
    }

    #[test]
    fn block_policy_applies_backpressure() {
        let dir = tempfile::tempdir().unwrap();
        let q = Arc::new(SegmentQueue::new(
            Policy::Block { max_queued: 1 },
            dir.path().join("s"),
        ));
        q.push(seg(0, 1)).unwrap();
        let producer = {
            let q = q.clone();
            std::thread::spawn(move || q.push(seg(1, 1)).unwrap())
        };
        std::thread::sleep(std::time::Duration::from_millis(50));
        assert_eq!(q.len(), 1, "second push waits for room");
        assert_eq!(q.pop(false).unwrap().unwrap().start, 0);
        assert!(producer.join().unwrap());
        assert_eq!(q.pop(false).unwrap().unwrap().start, 1);
    }

    #[test]
    fn wait_for_close_holds_segments_until_the_end() {
        let dir = tempfile::tempdir().unwrap();
        let q = Arc::new(SegmentQueue::new(
            Policy::Spill { max_in_ram: 1 },
            dir.path().join("s"),
        ));
        q.push(seg(0, 3)).unwrap();
        let consumer = {
            let q = q.clone();
            std::thread::spawn(move || q.pop(true).unwrap())
        };
        std::thread::sleep(std::time::Duration::from_millis(50));
        assert_eq!(q.len(), 1, "deferred: nothing consumed while open");
        q.close();
        assert_eq!(consumer.join().unwrap().unwrap().start, 0);
    }

    #[test]
    fn abort_unblocks_producer_and_consumer() {
        let dir = tempfile::tempdir().unwrap();
        let q = Arc::new(SegmentQueue::new(
            Policy::Block { max_queued: 1 },
            dir.path().join("s"),
        ));
        q.push(seg(0, 1)).unwrap();
        let producer = {
            let q = q.clone();
            std::thread::spawn(move || q.push(seg(1, 1)).unwrap())
        };
        std::thread::sleep(std::time::Duration::from_millis(20));
        q.abort();
        assert!(!producer.join().unwrap());
        assert!(q.pop(false).unwrap().is_none());
    }

    #[test]
    fn backlog_accounting() {
        // Idle and caught up: everything ingested is processed.
        let b = Backlog {
            ingested: 1_000,
            ..Default::default()
        };
        assert_eq!((b.processed(), b.behind()), (1_000, 0));
        // The earliest pending audio wins: in flight < queued < open.
        let b = Backlog {
            ingested: 10_000,
            open_start: Some(9_000),
            oldest_queued: Some(5_000),
            in_flight: Some(2_000),
        };
        assert_eq!((b.processed(), b.behind()), (2_000, 8_000));
        let b = Backlog {
            in_flight: None,
            ..b
        };
        assert_eq!(b.processed(), 5_000);
        // Never past what was ingested.
        let b = Backlog {
            ingested: 100,
            open_start: Some(500),
            ..Default::default()
        };
        assert_eq!(b.processed(), 100);
    }
}
