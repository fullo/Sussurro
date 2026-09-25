//! Saved audio as Ogg Opus (0.11, #247, P16/E15): the counterpart of the
//! WAV writer in [`super::audio`], with the same per-channel layout
//! (`audio.opus`, `audio-<channel>.opus`), the same session clock (a file
//! starts at the run's t = 0) and the same size/duration cap.
//!
//! **Encoding** (settled by spike V0-4, #238): libopus through the `opus`
//! crate, 16 kHz mono, VOIP, 20 ms packets, VBR at [`BITRATE`] b/s,
//! complexity 10 — about 10.7 MB per hour instead of 115 MB of WAV. The
//! container is written by the pure-Rust `ogg` crate.
//!
//! **Granule positions** (RFC 7845) count 48 kHz samples, pre-skip
//! included. The pre-skip is the encoder's lookahead × 3 (312 at 16 kHz):
//! a decoder drops that many samples first, so decoded sample `i` is input
//! sample `i` — the alignment the WAV files have. A page's granule is the
//! natural one (`packets × 960`); at the end, zeros flush the lookahead and
//! the last page's granule is `pre_skip + samples × 3` (end trimming), so a
//! decoder returns exactly the recorded length.
//!
//! **Crash safety**: a page is closed every [`PAGE_PACKETS`] packets (1 s)
//! and the buffered writer is flushed then, so a crash loses at most about
//! the last second (the spike measured 0.1–0.8 s after SIGKILL); the file
//! is synced to disk every [`SYNC_EVERY_PAGES`] pages against power loss.
//! A file left by a crash has no end-of-stream page and may end in a torn
//! page: [`repair`] (run by the startup recovery of #153) cuts it after the
//! last whole page and marks that page as the end of the stream.

use anyhow::{bail, Context, Result};
use ogg::writing::{PacketWriteEndInfo, PacketWriter};
use std::fs::{File, OpenOptions};
use std::io::{BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use super::audio::RATE;

/// Target bitrate (E15: where the quality curve flattens for Whisper and
/// the speaker embeddings).
pub const BITRATE: i32 = 24_000;
/// libopus complexity (E15; about 1 % of a core while recording).
pub const COMPLEXITY: i32 = 10;
/// Samples per packet: 20 ms at 16 kHz.
pub const FRAME: usize = (RATE / 50) as usize;
/// 48 kHz granule units per 16 kHz sample.
const GRANULE_PER_SAMPLE: u64 = 48_000 / RATE as u64;
/// Packets per Ogg page: one page per second of audio.
pub const PAGE_PACKETS: u64 = 50;
/// `fsync` every this many pages (~10 s), for power loss.
pub const SYNC_EVERY_PAGES: u64 = 10;
/// Largest Opus packet libopus may produce (RFC 6716 §3.4).
const MAX_PACKET: usize = 1275;

/// Ogg page header flags.
const FLAG_BOS: u8 = 0x02;
const FLAG_EOS: u8 = 0x04;
/// Fixed part of an Ogg page header (before the lacing table).
const PAGE_HEADER: usize = 27;

/// The `OpusHead` identification header (RFC 7845 §5.1): version 1, mono,
/// our pre-skip, 16 kHz input, no gain, mapping family 0.
fn opus_head(pre_skip: u16) -> Vec<u8> {
    let mut h = b"OpusHead".to_vec();
    h.push(1);
    h.push(1);
    h.extend_from_slice(&pre_skip.to_le_bytes());
    h.extend_from_slice(&RATE.to_le_bytes());
    h.extend_from_slice(&0i16.to_le_bytes());
    h.push(0);
    h
}

/// The `OpusTags` comment header: the libopus version as vendor, no tags.
fn opus_tags() -> Vec<u8> {
    let vendor = opus::version();
    let mut t = b"OpusTags".to_vec();
    t.extend_from_slice(&(vendor.len() as u32).to_le_bytes());
    t.extend_from_slice(vendor.as_bytes());
    t.extend_from_slice(&0u32.to_le_bytes());
    t
}

/// Our `OpusHead` (as [`opus_head`] writes it): its pre-skip, else `None`.
fn parse_head(packet: &[u8]) -> Option<u16> {
    (packet.len() == 19
        && &packet[0..8] == b"OpusHead"
        && packet[8] == 1
        && packet[9] == 1
        && packet[12..16] == RATE.to_le_bytes()
        && packet[18] == 0)
        .then(|| u16::from_le_bytes([packet[10], packet[11]]))
}

fn encoder() -> Result<opus::Encoder> {
    let mut e = opus::Encoder::new(RATE, opus::Channels::Mono, opus::Application::Voip)
        .context("starting the Opus encoder")?;
    e.set_bitrate(opus::Bitrate::Bits(BITRATE))?;
    e.set_vbr(true)?;
    e.set_complexity(COMPLEXITY)?;
    Ok(e)
}

/// A random stream serial (RFC 3533 asks for a unique one per stream).
fn serial() -> u32 {
    let mut b = [0u8; 4];
    if getrandom::fill(&mut b).is_err() {
        let t = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0);
        return t ^ std::process::id();
    }
    u32::from_le_bytes(b)
}

/// Incremental mono Ogg Opus writer (see the module docs).
pub struct OpusWriter {
    path: PathBuf,
    pw: PacketWriter<'static, BufWriter<File>>,
    enc: opus::Encoder,
    serial: u32,
    /// Input not yet encoded (< one frame).
    pending: Vec<f32>,
    /// Encoder lookahead, in 16 kHz samples.
    lookahead: u64,
    /// Pre-skip, in 48 kHz samples.
    pre_skip: u64,
    /// Samples accepted so far (encoded or pending).
    samples: u64,
    max_samples: u64,
    /// Packets encoded so far (the held one included).
    packets: u64,
    pages: u64,
    /// The latest packet and its granule, written when the next one comes
    /// (or by `finish`, which marks it as the end of the stream).
    held: Option<(Vec<u8>, u64)>,
    scratch: Vec<u8>,
}

impl OpusWriter {
    /// Create `path` — never over an existing file — holding a duration cap
    /// of `max_samples`; both headers are on disk when this returns.
    pub fn create_capped(path: &Path, max_samples: u64) -> Result<Self> {
        let mut enc = encoder()?;
        let lookahead = enc.get_lookahead().context("reading the Opus lookahead")? as u64;
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .with_context(|| format!("creating {}", path.display()))?;
        let mut w = Self {
            path: path.to_path_buf(),
            pw: PacketWriter::new(BufWriter::with_capacity(64 * 1024, file)),
            enc,
            serial: serial(),
            pending: Vec::with_capacity(FRAME),
            lookahead,
            pre_skip: lookahead * GRANULE_PER_SAMPLE,
            samples: 0,
            max_samples,
            packets: 0,
            pages: 0,
            held: None,
            scratch: vec![0u8; MAX_PACKET],
        };
        w.write_headers()?;
        Ok(w)
    }

    fn write_headers(&mut self) -> Result<()> {
        let pre_skip = u16::try_from(self.pre_skip).context("Opus pre-skip out of range")?;
        let path = &self.path;
        (|| -> std::io::Result<()> {
            // Each header on its own page (RFC 7845 §3).
            self.pw.write_packet(
                opus_head(pre_skip),
                self.serial,
                PacketWriteEndInfo::EndPage,
                0,
            )?;
            self.pw
                .write_packet(opus_tags(), self.serial, PacketWriteEndInfo::EndPage, 0)?;
            self.pw.inner_mut().flush()
        })()
        .with_context(|| format!("writing {}", path.display()))
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Samples written so far (the position on the session clock).
    pub fn samples(&self) -> u64 {
        self.samples
    }

    /// The cap was reached: further samples are dropped.
    pub fn is_full(&self) -> bool {
        self.samples >= self.max_samples
    }

    fn room(&self) -> u64 {
        self.max_samples.saturating_sub(self.samples)
    }

    /// Append samples (up to the cap). Returns how many were written.
    pub fn write(&mut self, samples: &[f32]) -> Result<usize> {
        let n = samples
            .len()
            .min(usize::try_from(self.room()).unwrap_or(usize::MAX));
        let mut rest = &samples[..n];
        while !rest.is_empty() {
            let take = (FRAME - self.pending.len()).min(rest.len());
            self.pending.extend_from_slice(&rest[..take]);
            rest = &rest[take..];
            self.samples += take as u64;
            if self.pending.len() == FRAME {
                self.encode_pending()?;
            }
        }
        Ok(n)
    }

    /// Append `n` samples of silence (up to the cap).
    pub fn write_silence(&mut self, n: u64) -> Result<u64> {
        let n = n.min(self.room());
        let mut left = n;
        while left > 0 {
            let take = ((FRAME - self.pending.len()) as u64).min(left);
            self.pending.resize(self.pending.len() + take as usize, 0.0);
            left -= take;
            self.samples += take;
            if self.pending.len() == FRAME {
                self.encode_pending()?;
            }
        }
        Ok(n)
    }

    /// Encode the pending frame (padded with zeros) as the next packet.
    fn encode_pending(&mut self) -> Result<()> {
        self.pending.resize(FRAME, 0.0);
        let n = self
            .enc
            .encode_float(&self.pending, &mut self.scratch)
            .with_context(|| format!("encoding {}", self.path.display()))?;
        self.pending.clear();
        let packet = self.scratch[..n].to_vec();
        self.emit_held(false)?;
        self.packets += 1;
        // Natural granule: every 48 kHz sample decoded so far.
        self.held = Some((packet, self.packets * FRAME as u64 * GRANULE_PER_SAMPLE));
        Ok(())
    }

    /// Write the held packet; `last` ends the stream. A page closes every
    /// [`PAGE_PACKETS`] packets and the file is flushed there.
    fn emit_held(&mut self, last: bool) -> Result<()> {
        let Some((packet, granule)) = self.held.take() else {
            return Ok(());
        };
        let info = if last {
            PacketWriteEndInfo::EndStream
        } else if self.packets.is_multiple_of(PAGE_PACKETS) {
            PacketWriteEndInfo::EndPage
        } else {
            PacketWriteEndInfo::NormalPacket
        };
        let path = &self.path;
        (|| -> std::io::Result<()> {
            self.pw.write_packet(packet, self.serial, info, granule)?;
            if info != PacketWriteEndInfo::NormalPacket {
                self.pw.inner_mut().flush()?;
                self.pages += 1;
                if self.pages.is_multiple_of(SYNC_EVERY_PAGES) {
                    self.pw.inner().get_ref().sync_data()?;
                }
            }
            Ok(())
        })()
        .with_context(|| format!("writing {}", path.display()))
    }

    /// Flush the encoder's lookahead with zeros, end-trim the stream to the
    /// samples written, close it and sync the file to disk.
    pub fn finish(mut self) -> Result<u64> {
        // Encode until every input sample has left the encoder: packets ×
        // FRAME ≥ samples + lookahead (at least one packet, so the stream
        // always has an end-of-stream page carrying audio).
        while self.packets * (FRAME as u64) < self.samples + self.lookahead || self.held.is_none() {
            self.encode_pending()?;
        }
        if let Some((_, granule)) = self.held.as_mut() {
            *granule = self.pre_skip + self.samples * GRANULE_PER_SAMPLE;
        }
        self.emit_held(true)?;
        let path = self.path.clone();
        let out = self.pw.into_inner();
        let file = out
            .into_inner()
            .map_err(|e| e.into_error())
            .with_context(|| format!("writing {}", path.display()))?;
        file.sync_all()
            .with_context(|| format!("saving {}", path.display()))?;
        Ok(self.samples)
    }
}

// ---- crash repair ------------------------------------------------------------

/// CRC-32 of Ogg pages (polynomial 0x04c11db7, no reflection, init 0).
fn ogg_crc(data: &[u8]) -> u32 {
    const fn table() -> [u32; 256] {
        let mut t = [0u32; 256];
        let mut i = 0;
        while i < 256 {
            let mut r = (i as u32) << 24;
            let mut k = 0;
            while k < 8 {
                r = if r & 0x8000_0000 != 0 {
                    (r << 1) ^ 0x04c1_1db7
                } else {
                    r << 1
                };
                k += 1;
            }
            t[i] = r;
            i += 1;
        }
        t
    }
    const TABLE: [u32; 256] = table();
    data.iter().fold(0u32, |crc, &b| {
        (crc << 8) ^ TABLE[(((crc >> 24) as u8) ^ b) as usize]
    })
}

/// One whole, checksum-valid page found by [`scan`].
struct Page {
    offset: u64,
    len: u64,
    flags: u8,
    granule: u64,
    serial: u32,
    sequence: u32,
    /// Its last packet goes on in the next page (lacing value 255).
    continues: bool,
    /// The first packet's first bytes (enough for an `OpusHead`).
    head: Vec<u8>,
}

/// Read the next whole page at `offset`; `None` at the end of the file or
/// on a torn/corrupt page (everything from there on is dropped).
fn read_page<R: Read + Seek>(r: &mut R, offset: u64) -> Option<Page> {
    let mut header = [0u8; PAGE_HEADER];
    r.seek(SeekFrom::Start(offset)).ok()?;
    r.read_exact(&mut header).ok()?;
    if &header[0..4] != b"OggS" || header[4] != 0 {
        return None;
    }
    let mut lacing = vec![0u8; header[26] as usize];
    r.read_exact(&mut lacing).ok()?;
    let body_len: usize = lacing.iter().map(|&l| l as usize).sum();
    let mut body = vec![0u8; body_len];
    r.read_exact(&mut body).ok()?;
    let stored = u32::from_le_bytes(header[22..26].try_into().ok()?);
    header[22..26].fill(0);
    let mut all = Vec::with_capacity(PAGE_HEADER + lacing.len() + body_len);
    all.extend_from_slice(&header);
    all.extend_from_slice(&lacing);
    all.extend_from_slice(&body);
    if ogg_crc(&all) != stored {
        return None;
    }
    Some(Page {
        offset,
        len: all.len() as u64,
        flags: header[5],
        granule: u64::from_le_bytes(header[6..14].try_into().ok()?),
        serial: u32::from_le_bytes(header[14..18].try_into().ok()?),
        sequence: u32::from_le_bytes(header[18..22].try_into().ok()?),
        continues: lacing.last() == Some(&255),
        head: body[..body.len().min(19)].to_vec(),
    })
}

/// Rewrite `file` as an empty stream of ours (headers + one silent packet
/// ending the stream): a crash cut the headers or left no audio page.
fn write_empty_stream(path: &Path, file: File) -> Result<u64> {
    drop(file);
    std::fs::remove_file(path)?;
    OpusWriter::create_capped(path, 0)?.finish()
}

/// Make an Ogg Opus file left by a crash complete again: cut it after the
/// last whole page and mark that page as the end of the stream (its
/// granule already says how much audio precedes it). A file cut inside the
/// headers, or with no audio page, becomes an empty stream. Only a mono
/// 16 kHz Opus stream as [`OpusWriter`] writes it is touched; any other
/// file is refused, untouched. Idempotent. Returns the samples kept.
pub fn repair(path: &Path) -> Result<u64> {
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .with_context(|| format!("opening {}", path.display()))?;
    let len = file.metadata()?.len();
    let mut r = BufReader::new(&file);
    let Some(first) = read_page(&mut r, 0) else {
        // A crash before the first page was whole: rewritten only if what
        // is there is the start of an Ogg page.
        let mut head = Vec::new();
        r.seek(SeekFrom::Start(0))?;
        r.by_ref().take(4).read_to_end(&mut head)?;
        if !b"OggS".starts_with(&head) {
            bail!("{} is not an Opus file written by Sussurro", path.display());
        }
        drop(r);
        return write_empty_stream(path, file);
    };
    let Some(pre_skip) = parse_head(&first.head).filter(|_| first.flags & FLAG_BOS != 0) else {
        bail!("{} is not an Opus file written by Sussurro", path.display());
    };
    let pre_skip = pre_skip as u64;
    // Whole pages of our stream, in order; the first break ends the scan.
    let mut pages = vec![first];
    loop {
        let last = &pages[pages.len() - 1];
        if last.flags & FLAG_EOS != 0 {
            break;
        }
        match read_page(&mut r, last.offset + last.len) {
            Some(p) if p.serial == last.serial && p.sequence == last.sequence.wrapping_add(1) => {
                pages.push(Page {
                    head: Vec::new(),
                    ..p
                });
            }
            _ => break,
        }
    }
    drop(r);
    // The stream must end on a whole packet with a known granule: a page
    // whose last packet goes on in the (lost) next one is dropped too.
    while pages.len() > 2 && pages[pages.len() - 1].flags & FLAG_EOS == 0 {
        let last = &pages[pages.len() - 1];
        if !last.continues && last.granule != u64::MAX {
            break;
        }
        pages.pop();
    }
    // Pages 0 and 1 are the headers: no audio page survived.
    if pages.len() < 3 {
        return write_empty_stream(path, file);
    }
    let last = pages.swap_remove(pages.len() - 1);
    let end = last.offset + last.len;
    if end != len {
        file.set_len(end)?;
    }
    if last.flags & FLAG_EOS == 0 {
        // Set the end-of-stream flag and the page's checksum in place.
        let mut page = vec![0u8; last.len as usize];
        file.seek(SeekFrom::Start(last.offset))?;
        file.read_exact(&mut page)?;
        page[5] |= FLAG_EOS;
        page[22..26].fill(0);
        let crc = ogg_crc(&page);
        page[22..26].copy_from_slice(&crc.to_le_bytes());
        file.seek(SeekFrom::Start(last.offset))?;
        file.write_all(&page[..26])?;
    }
    file.sync_all()?;
    Ok(last.granule.saturating_sub(pre_skip) / GRANULE_PER_SAMPLE)
}

// ---- reading and seeking (#248) --------------------------------------------

/// Decoding restarts this many samples before a seek target: 200 ms, where
/// spike V0-4 measured 49 dB against a linear decode (80 ms gave 29.6 dB).
pub const PRE_ROLL: u64 = RATE as u64 / 5;
/// A seek at most this far ahead decodes forward instead of restarting:
/// cheaper than a restart (which decodes the pre-roll plus up to a page)
/// and bit-exact with a linear decode.
const FORWARD_SLACK: u64 = 2 * RATE as u64;
/// Decoder output room: the longest Opus packet (120 ms) at 48 kHz, more
/// than enough at 16 kHz.
const MAX_DECODED: usize = 5_760;

/// One page of the stream's audio, as the index keeps it.
#[derive(Debug, Clone, Copy)]
struct IndexedPage {
    offset: u64,
    len: u64,
    /// Samples decoded from the start of the stream (16 kHz, pre-skip
    /// included) once this page's packets are; `None` when no packet ends
    /// on the page.
    end: Option<u64>,
    /// Its first packet began on the page before.
    continued: bool,
}

/// A page header read during the index scan (body not read, not checked).
struct RawHeader {
    flags: u8,
    granule: u64,
    serial: u32,
    lacing: Vec<u8>,
}

impl RawHeader {
    fn body_len(&self) -> u64 {
        self.lacing.iter().map(|&l| l as u64).sum()
    }

    fn len(&self) -> u64 {
        (PAGE_HEADER + self.lacing.len()) as u64 + self.body_len()
    }
}

/// The next page header, or `None` at the end of the file or on anything
/// that is not a page.
fn read_raw_header<R: Read>(r: &mut R) -> Option<RawHeader> {
    let mut h = [0u8; PAGE_HEADER];
    r.read_exact(&mut h).ok()?;
    if &h[0..4] != b"OggS" || h[4] != 0 {
        return None;
    }
    let mut lacing = vec![0u8; h[26] as usize];
    r.read_exact(&mut lacing).ok()?;
    Some(RawHeader {
        flags: h[5],
        granule: u64::from_le_bytes(h[6..14].try_into().ok()?),
        serial: u32::from_le_bytes(h[14..18].try_into().ok()?),
        lacing,
    })
}

/// The packets of a page body by its lacing: `(bytes, ends on this page)`.
fn split_packets<'a>(lacing: &[u8], body: &'a [u8]) -> Vec<(&'a [u8], bool)> {
    let mut out = Vec::new();
    let (mut from, mut at) = (0usize, 0usize);
    for &l in lacing {
        at += l as usize;
        if l < 255 {
            out.push((&body[from..at.min(body.len())], true));
            from = at;
        }
    }
    if from < at {
        out.push((&body[from..at.min(body.len())], false));
    }
    out
}

/// The `OpusHead` of any mono or stereo Ogg Opus stream (channel mapping
/// family 0, RFC 7845 §5.1): its pre-skip (48 kHz samples) and output gain
/// (Q7.8 dB). Stereo streams are decoded to mono by libopus.
fn parse_head_any(packet: &[u8]) -> Option<(u64, i16)> {
    (packet.len() >= 19
        && &packet[0..8] == b"OpusHead"
        && packet[8] & 0xF0 == 0
        && matches!(packet[9], 1 | 2)
        && packet[18] == 0)
        .then(|| {
            (
                u16::from_le_bytes([packet[10], packet[11]]) as u64,
                i16::from_le_bytes([packet[16], packet[17]]),
            )
        })
}

/// Reads an Ogg Opus file as 16 kHz mono samples, with seeking — what the
/// `sussurro-audio:` scheme serves as a WAV (#248, E15) and what re-reads
/// saved audio (Identify voices).
///
/// **Positions** are sample indices of the trimmed stream: pre-skip
/// dropped, end trimmed to the last granule, so sample `i` is sample `i` of
/// what was recorded and [`Self::total_samples`] is its exact length.
///
/// **Index**: opening scans the page headers only (offset, end granule;
/// about 1 ms and 3,600 entries for an hour). A seek decodes from the page
/// that ends at least [`PRE_ROLL`] before the target, with the decoder
/// reset; reading on from where the reader stopped (or seeking up to
/// 2 s ahead) goes on decoding, bit-exact with a linear decode.
///
/// **Damage**: a page with a bad checksum is skipped (the decoder resets)
/// and audio that can't be decoded reads as silence, so a reader always
/// returns exactly `total_samples` samples; [`Self::filled`] says how many
/// were silence put in.
///
/// The file is opened only while pages are being read and can be closed
/// between reads ([`Self::close`]): a reader kept in a cache never holds
/// the file open (Windows can't move a folder with an open file).
pub struct OpusReader {
    path: PathBuf,
    file: Option<File>,
    serial: u32,
    pages: Vec<IndexedPage>,
    /// Pre-skip, in 16 kHz samples.
    pre: u64,
    total: u64,
    dec: opus::Decoder,
    /// The next page to read, as an index into `pages`.
    next_page: usize,
    packets: std::collections::VecDeque<Vec<u8>>,
    /// A packet that goes on in the next page.
    partial: Option<Vec<u8>>,
    /// Stream position (from the first decoded sample, pre-skip included)
    /// of the decoder's next output sample.
    k: u64,
    /// Trimmed position of the next sample [`Self::read`] returns.
    t: u64,
    /// Decoded samples not returned yet: `buf[buf_at..]` start at `t`.
    buf: Vec<f32>,
    buf_at: usize,
    scratch: Vec<f32>,
    filled: u64,
}

impl OpusReader {
    /// Open `path` and index its pages. Fails for anything but a mono or
    /// stereo Ogg Opus stream with whole headers.
    pub fn open(path: &Path) -> Result<Self> {
        let file = File::open(path).with_context(|| format!("opening {}", path.display()))?;
        let file_len = file.metadata()?.len();
        let not_opus = || anyhow::anyhow!("{} is not an Ogg Opus file", path.display());
        let mut r = BufReader::with_capacity(64 * 1024, file);
        // The identification header: a page of its own.
        let first = read_raw_header(&mut r).ok_or_else(not_opus)?;
        if first.flags & FLAG_BOS == 0 || first.lacing.last().is_none_or(|&l| l == 255) {
            return Err(not_opus());
        }
        let mut head = vec![0u8; first.body_len() as usize];
        r.read_exact(&mut head).map_err(|_| not_opus())?;
        let (pre_skip, gain) = parse_head_any(&head).ok_or_else(not_opus)?;
        let serial = first.serial;
        let mut offset = first.len();
        // The comment header, over one or more pages: it ends on the first
        // page with a lacing value under 255, and audio starts on a new page.
        loop {
            let h = read_raw_header(&mut r)
                .filter(|h| offset + h.len() <= file_len)
                .with_context(|| format!("{} ends inside its headers", path.display()))?;
            r.seek_relative(h.body_len() as i64)?;
            offset += h.len();
            if h.serial == serial && h.lacing.iter().any(|&l| l < 255) {
                break;
            }
        }
        // Audio pages: headers only, except the first page's packets (the
        // stream may start after granule 0, RFC 7845 §4.5).
        let mut raw: Vec<(IndexedPage, u64)> = Vec::new();
        let mut start48 = 0u64;
        while let Some(h) = read_raw_header(&mut r) {
            let len = h.len();
            if offset + len > file_len {
                break; // a torn last page
            }
            if h.serial != serial {
                r.seek_relative(h.body_len() as i64)?;
                offset += len;
                continue;
            }
            if raw.is_empty() {
                let mut body = vec![0u8; h.body_len() as usize];
                r.read_exact(&mut body)?;
                if h.granule != u64::MAX {
                    let samples: u64 = split_packets(&h.lacing, &body)
                        .iter()
                        .filter(|(_, whole)| *whole)
                        .filter_map(|(p, _)| opus::packet::get_nb_samples(p, 48_000).ok())
                        .map(|n| n as u64)
                        .sum();
                    start48 = h.granule.saturating_sub(samples);
                }
            } else {
                r.seek_relative(h.body_len() as i64)?;
            }
            raw.push((
                IndexedPage {
                    offset,
                    len,
                    end: None,
                    continued: h.flags & 0x01 != 0,
                },
                h.granule,
            ));
            offset += len;
            if h.flags & FLAG_EOS != 0 {
                break;
            }
        }
        let pages: Vec<IndexedPage> = raw
            .into_iter()
            .map(|(p, granule)| IndexedPage {
                end: (granule != u64::MAX)
                    .then(|| granule.saturating_sub(start48) / GRANULE_PER_SAMPLE),
                ..p
            })
            .collect();
        let pre = pre_skip / GRANULE_PER_SAMPLE;
        let total = pages
            .iter()
            .rev()
            .find_map(|p| p.end)
            .unwrap_or(0)
            .saturating_sub(pre);
        let mut dec = opus::Decoder::new(RATE, opus::Channels::Mono)
            .context("starting the Opus decoder")?;
        if gain != 0 {
            dec.set_gain(gain as i32)?;
        }
        Ok(Self {
            path: path.to_path_buf(),
            file: None,
            serial,
            pages,
            pre,
            total,
            dec,
            next_page: 0,
            packets: Default::default(),
            partial: None,
            k: 0,
            t: 0,
            buf: Vec::new(),
            buf_at: 0,
            scratch: vec![0f32; MAX_DECODED],
            filled: 0,
        })
    }

    /// Length of the stream in 16 kHz samples (trimmed: what was recorded).
    pub fn total_samples(&self) -> u64 {
        self.total
    }

    /// Position of the next sample [`Self::read`] returns.
    pub fn position(&self) -> u64 {
        self.t
    }

    /// Samples read as silence because they could not be decoded.
    pub fn filled(&self) -> u64 {
        self.filled
    }

    /// Close the file until the next read needs it.
    pub fn close(&mut self) {
        self.file = None;
    }

    /// Move to sample `t` (clamped to the end). See the type docs for when
    /// this restarts the decoder.
    pub fn seek(&mut self, t: u64) -> Result<()> {
        let t = t.min(self.total);
        if t >= self.t && t - self.t <= FORWARD_SLACK {
            // Forward: through what is decoded, then by decoding on (the
            // samples before `t` are dropped as they come).
            let ahead = (t - self.t) as usize;
            let buffered = self.buf.len() - self.buf_at;
            if ahead <= buffered {
                self.buf_at += ahead;
            } else {
                self.buf.clear();
                self.buf_at = 0;
            }
            self.t = t;
            return Ok(());
        }
        let from = (t + self.pre).saturating_sub(PRE_ROLL);
        // The last page that ends before the pre-roll starts and is
        // followed by a page starting on a whole packet.
        let restart = (0..self.pages.len()).rev().find(|&i| {
            self.pages[i].end.is_some_and(|e| e <= from)
                && self.pages.get(i + 1).is_some_and(|n| !n.continued)
        });
        (self.next_page, self.k) = match restart {
            Some(i) => (i + 1, self.pages[i].end.unwrap_or(0)),
            None => (0, 0),
        };
        self.dec.reset_state()?;
        self.packets.clear();
        self.partial = None;
        self.buf.clear();
        self.buf_at = 0;
        self.t = t;
        Ok(())
    }

    /// Append up to `max` samples from the current position to `out`;
    /// returns how many (0 at the end).
    pub fn read(&mut self, out: &mut Vec<f32>, max: usize) -> Result<usize> {
        let want = (max as u64).min(self.total - self.t) as usize;
        let mut n = 0;
        while n < want {
            if self.buf_at < self.buf.len() {
                let take = (want - n).min(self.buf.len() - self.buf_at);
                out.extend_from_slice(&self.buf[self.buf_at..self.buf_at + take]);
                self.buf_at += take;
                self.t += take as u64;
                n += take;
            } else if !self.decode_next()? {
                // The stream ended early: silence up to its announced end.
                let rest = want - n;
                out.resize(out.len() + rest, 0.0);
                self.t += rest as u64;
                self.filled += rest as u64;
                n = want;
            }
        }
        Ok(n)
    }

    /// Decode packets until some output falls at or after `t`; false when
    /// the stream has no more packets.
    fn decode_next(&mut self) -> Result<bool> {
        let tk = self.t + self.pre;
        loop {
            let Some(packet) = self.next_packet()? else {
                return Ok(false);
            };
            let got = match self.dec.decode_float(&packet, &mut self.scratch, false) {
                Ok(n) => n,
                Err(_) => {
                    // A damaged packet: its length in silence, decoder reset.
                    self.dec.reset_state()?;
                    let n = opus::packet::get_nb_samples(&packet, RATE).unwrap_or(0);
                    self.scratch[..n.min(MAX_DECODED)].fill(0.0);
                    n.min(MAX_DECODED)
                }
            };
            let start = self.k;
            self.k += got as u64;
            if self.k <= tk {
                continue;
            }
            self.buf.clear();
            self.buf_at = 0;
            if start > tk {
                // A gap (a skipped page) before this packet: silence.
                let gap = (start - tk) as usize;
                self.buf.resize(gap, 0.0);
                self.filled += gap as u64;
                self.buf.extend_from_slice(&self.scratch[..got]);
            } else {
                self.buf
                    .extend_from_slice(&self.scratch[(tk - start) as usize..got]);
            }
            return Ok(true);
        }
    }

    /// The next whole packet of the stream, reading pages as needed.
    fn next_packet(&mut self) -> Result<Option<Vec<u8>>> {
        loop {
            if let Some(p) = self.packets.pop_front() {
                return Ok(Some(p));
            }
            let Some(&page) = self.pages.get(self.next_page) else {
                return Ok(None);
            };
            self.next_page += 1;
            let Some((lacing, body)) = self.read_page(page)? else {
                // A damaged page: its packets are lost; the next page
                // starts where it ended.
                self.partial = None;
                if let Some(end) = page.end.filter(|&e| e > self.k) {
                    self.k = end;
                }
                self.dec.reset_state()?;
                continue;
            };
            let mut partial = self.partial.take();
            for (i, (bytes, whole)) in split_packets(&lacing, &body).into_iter().enumerate() {
                let mut packet = if i == 0 && page.continued {
                    match partial.take() {
                        Some(mut p) => {
                            p.extend_from_slice(bytes);
                            p
                        }
                        // The packet's start was not read (a seek, a lost
                        // page): drop the rest of it.
                        None => continue,
                    }
                } else {
                    bytes.to_vec()
                };
                if whole {
                    self.packets.push_back(std::mem::take(&mut packet));
                } else {
                    self.partial = Some(packet);
                }
            }
        }
    }

    /// A whole page of ours, checksum verified: `(lacing, body)`; `None`
    /// when it is damaged.
    fn read_page(&mut self, page: IndexedPage) -> Result<Option<(Vec<u8>, Vec<u8>)>> {
        if self.file.is_none() {
            self.file = Some(
                File::open(&self.path)
                    .with_context(|| format!("opening {}", self.path.display()))?,
            );
        }
        let file = self.file.as_mut().expect("opened above");
        let mut bytes = vec![0u8; page.len as usize];
        file.seek(SeekFrom::Start(page.offset))?;
        if file.read_exact(&mut bytes).is_err() {
            return Ok(None); // the file shrank
        }
        let segments = bytes[26] as usize;
        if &bytes[0..4] != b"OggS"
            || bytes[14..18] != self.serial.to_le_bytes()
            || PAGE_HEADER + segments > bytes.len()
        {
            return Ok(None);
        }
        let stored = u32::from_le_bytes(bytes[22..26].try_into()?);
        bytes[22..26].fill(0);
        if ogg_crc(&bytes) != stored {
            return Ok(None);
        }
        let body = bytes.split_off(PAGE_HEADER + segments);
        let lacing = bytes.split_off(PAGE_HEADER);
        Ok(Some((lacing, body)))
    }
}

/// Decode `path` through to the end and check that every sample decoded:
/// the length in samples of a sound Ogg Opus file (the check before a WAV
/// is replaced by its Opus copy, #248).
pub fn verify(path: &Path) -> Result<u64> {
    let mut r = OpusReader::open(path)?;
    let mut buf = Vec::with_capacity(RATE as usize);
    loop {
        buf.clear();
        if r.read(&mut buf, RATE as usize)? == 0 {
            break;
        }
    }
    if r.filled() > 0 {
        bail!(
            "{}: {} samples could not be decoded",
            path.display(),
            r.filled()
        );
    }
    Ok(r.total_samples())
}

/// Decode a whole file with libopus (tests; playback is #248): 16 kHz mono
/// samples with the pre-skip dropped and the end trimmed to the last
/// granule — what any conforming player returns — and whether the stream
/// had its end-of-stream page.
#[cfg(test)]
pub(crate) fn decode_file(path: &Path) -> Result<(Vec<f32>, bool)> {
    let mut reader = ogg::reading::PacketReader::new(BufReader::new(File::open(path)?));
    let head = reader
        .read_packet_expected()
        .context("no OpusHead packet")?;
    let pre_skip = parse_head(&head.data).context("not our OpusHead")? as u64;
    reader
        .read_packet_expected()
        .context("no OpusTags packet")?;
    let mut dec = opus::Decoder::new(RATE, opus::Channels::Mono)?;
    let mut pcm = Vec::new();
    let mut out = vec![0f32; 5_760];
    let mut granule = None;
    let mut complete = false;
    while let Ok(Some(p)) = reader.read_packet() {
        let n = dec.decode_float(&p.data, &mut out, false)?;
        pcm.extend_from_slice(&out[..n]);
        if p.last_in_page() {
            granule = Some(p.absgp_page());
        }
        if p.last_in_stream() {
            complete = true;
            break;
        }
    }
    let skip = (pre_skip / GRANULE_PER_SAMPLE) as usize;
    let mut pcm = pcm.split_off(skip.min(pcm.len()));
    if let Some(g) = granule {
        pcm.truncate((g.saturating_sub(pre_skip) / GRANULE_PER_SAMPLE) as usize);
    }
    Ok((pcm, complete))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Test signal with no period (so a lag is unambiguous): a tone whose
    /// pitch sweeps 150–1 150 Hz and back every second, with a slow
    /// envelope — speech-like enough for the VOIP encoder.
    fn voice(n: usize, from: usize) -> Vec<f32> {
        (from..from + n)
            .map(|i| {
                let t = i as f64 / RATE as f64;
                let env = 0.6 + 0.4 * (t * 3.0).sin();
                // Phase of a triangle-swept frequency: ∫ f(t) dt.
                let phase = 2.0
                    * std::f64::consts::PI
                    * (650.0 * t
                        - 500.0 * (2.0 * std::f64::consts::PI * t).sin()
                            / (2.0 * std::f64::consts::PI));
                (env * 0.3 * (phase.sin() + 0.4 * (2.0 * phase).sin())) as f32
            })
            .collect()
    }

    fn corr(a: &[f32], b: &[f32]) -> f32 {
        let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
        let na: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
        let nb: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
        dot / (na * nb).max(1e-9)
    }

    /// Best-matching lag of `b` against `a` in -150..=150 samples (wider
    /// than the 104-sample encoder delay, so a missing pre-skip would show).
    fn lag(a: &[f32], b: &[f32]) -> i32 {
        (-150i32..=150)
            .max_by(|&x, &y| {
                let c = |l: i32| {
                    let (a, b) = if l >= 0 {
                        (&a[..a.len() - l as usize], &b[l as usize..])
                    } else {
                        (&a[(-l) as usize..], &b[..b.len() - (-l) as usize])
                    };
                    corr(a, b)
                };
                c(x).total_cmp(&c(y))
            })
            .unwrap()
    }

    #[test]
    fn crc_matches_the_ogg_reference() {
        // Pages written by the `ogg` crate verify with ours (every repair
        // test relies on it); here the catalogue check value of
        // CRC-32/CKSUM (same polynomial and init) without its final xor.
        assert_eq!(ogg_crc(b""), 0);
        assert_eq!(ogg_crc(b"123456789"), 0x89a1_897f);
    }

    #[test]
    fn round_trip_keeps_length_and_alignment() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("audio-mic.opus");
        let mut w = OpusWriter::create_capped(&path, u64::MAX).unwrap();
        // Uneven writes (a mic's polls), a silence gap, more audio.
        let a = voice(37_123, 0);
        for chunk in a.chunks(4_001) {
            assert_eq!(w.write(chunk).unwrap(), chunk.len());
        }
        assert_eq!(w.write_silence(8_000).unwrap(), 8_000);
        let b = voice(20_000, 45_123);
        w.write(&b).unwrap();
        assert_eq!(w.samples(), 65_123);
        assert_eq!(w.finish().unwrap(), 65_123);

        let (pcm, complete) = decode_file(&path).unwrap();
        assert!(complete);
        assert_eq!(pcm.len(), 65_123, "exact length (pre-skip + end trimming)");
        // Aligned: decoded sample i is input sample i.
        // (A lossy codec's phase may move a few samples: well under 1 ms.)
        let mid = 16_000..32_000;
        assert!(lag(&a[mid.clone()], &pcm[mid.clone()]).abs() <= 4);
        assert!(corr(&a[mid.clone()], &pcm[mid]) > 0.8);
        let tail = 45_123 + 4_000..45_123 + 16_000;
        assert!(lag(&b[4_000..16_000], &pcm[tail.clone()]).abs() <= 4);
        assert!(corr(&b[4_000..16_000], &pcm[tail]) > 0.8);
        // The gap stays (almost) silent.
        let gap = &pcm[37_123 + 800..45_123 - 800];
        assert!(gap.iter().all(|s| s.abs() < 0.02));

        // ~24 kb/s, one page per second plus the two headers.
        let bytes = std::fs::metadata(&path).unwrap().len();
        let seconds = 65_123.0 / RATE as f64;
        assert!(
            (bytes as f64 * 8.0 / seconds) < 40_000.0,
            "{bytes} bytes for {seconds} s"
        );
    }

    #[test]
    fn pages_are_flushed_every_second_while_recording() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("audio.opus");
        let mut w = OpusWriter::create_capped(&path, u64::MAX).unwrap();
        let headers = std::fs::metadata(&path).unwrap().len();
        assert!(headers > 0, "headers are on disk at once");
        w.write(&voice(3 * RATE as usize + 700, 0)).unwrap();
        // Not finished: whole pages of the first seconds are already on disk
        // and decode (the tail, < 1 s + one packet, is still in memory).
        let (pcm, complete) = decode_file(&path).unwrap();
        assert!(!complete);
        assert!(pcm.len() >= 2 * RATE as usize, "{}", pcm.len());
        assert!(pcm.len() <= 3 * RATE as usize, "{}", pcm.len());
        drop(w);
    }

    #[test]
    fn empty_and_tiny_streams_are_valid() {
        let dir = tempfile::tempdir().unwrap();
        let empty = dir.path().join("audio-mic.opus");
        OpusWriter::create_capped(&empty, u64::MAX)
            .unwrap()
            .finish()
            .unwrap();
        assert_eq!(decode_file(&empty).unwrap(), (vec![], true));
        let tiny = dir.path().join("audio-remote.opus");
        let mut w = OpusWriter::create_capped(&tiny, u64::MAX).unwrap();
        w.write(&voice(5, 0)).unwrap();
        w.finish().unwrap();
        let (pcm, complete) = decode_file(&tiny).unwrap();
        assert!(complete);
        assert_eq!(pcm.len(), 5);
    }

    #[test]
    fn never_overwrites_an_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("audio.opus");
        std::fs::write(&path, b"user's own file").unwrap();
        assert!(OpusWriter::create_capped(&path, u64::MAX).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"user's own file");
    }

    #[test]
    fn duration_cap_stops_writing_and_keeps_a_valid_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("audio.opus");
        let mut w = OpusWriter::create_capped(&path, 500).unwrap();
        assert_eq!(w.write(&voice(300, 0)).unwrap(), 300);
        assert_eq!(w.write(&voice(300, 300)).unwrap(), 200);
        assert!(w.is_full());
        assert_eq!(w.write(&voice(10, 0)).unwrap(), 0);
        assert_eq!(w.write_silence(10).unwrap(), 0);
        assert_eq!(w.finish().unwrap(), 500);
        assert_eq!(decode_file(&path).unwrap().0.len(), 500);
    }

    /// A crash: the writer never finishes (its held packet and the open
    /// page never reach the disk) and the last page on disk is torn. The
    /// repaired file ends with an end-of-stream page and decodes up to the
    /// last whole page, still aligned.
    #[test]
    fn truncated_file_is_closed_by_repair() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("audio-mic.opus");
        let audio = voice(5 * RATE as usize + 3_000, 0);
        let mut w = OpusWriter::create_capped(&path, u64::MAX).unwrap();
        w.write(&audio).unwrap();
        std::mem::forget(w);
        let whole = std::fs::read(&path).unwrap();
        // Cut in the middle of the last page written.
        let last_page = whole.windows(4).rposition(|w| w == b"OggS").unwrap();
        let cut = last_page + (whole.len() - last_page) / 2;
        std::fs::write(&path, &whole[..cut]).unwrap();

        let kept = repair(&path).unwrap();
        // Pages end on whole seconds of packets: 4 s of audio minus the
        // lookahead survive (the 5th page was torn).
        assert_eq!(kept, (4 * 50 * FRAME as u64 * 3 - 312) / 3);
        let fixed = std::fs::read(&path).unwrap();
        assert_eq!(fixed.len(), last_page, "the torn page is dropped");
        let (pcm, complete) = decode_file(&path).unwrap();
        assert!(complete, "the stream is closed");
        assert_eq!(pcm.len() as u64, kept);
        let span = 8_000..40_000;
        assert!(lag(&audio[span.clone()], &pcm[span.clone()]).abs() <= 4);
        assert!(corr(&audio[span.clone()], &pcm[span]) > 0.8);

        // Idempotent, and a finished file is left as it is.
        assert_eq!(repair(&path).unwrap(), kept);
        assert_eq!(std::fs::read(&path).unwrap(), fixed);
        let done = dir.path().join("audio-remote.opus");
        let mut w = OpusWriter::create_capped(&done, u64::MAX).unwrap();
        w.write(&audio[..10_000]).unwrap();
        w.finish().unwrap();
        let before = std::fs::read(&done).unwrap();
        assert_eq!(repair(&done).unwrap(), 10_000);
        assert_eq!(std::fs::read(&done).unwrap(), before);
    }

    #[test]
    fn repair_of_files_cut_inside_the_headers_and_of_foreign_files() {
        let dir = tempfile::tempdir().unwrap();
        // Cut inside the first page, and before any audio page.
        for keep in [0usize, 3, 20, 40, 60] {
            let path = dir.path().join(format!("audio-x{keep}.opus"));
            let mut w = OpusWriter::create_capped(&path, u64::MAX).unwrap();
            w.write(&voice(1_000, 0)).unwrap();
            std::mem::forget(w);
            let bytes = std::fs::read(&path).unwrap();
            let keep = keep.min(bytes.len());
            std::fs::write(&path, &bytes[..keep]).unwrap();
            assert_eq!(repair(&path).unwrap(), 0, "cut at {keep}");
            assert_eq!(decode_file(&path).unwrap(), (vec![], true));
        }
        let headers_only = dir.path().join("audio-h.opus");
        std::mem::forget(OpusWriter::create_capped(&headers_only, u64::MAX).unwrap());
        assert_eq!(repair(&headers_only).unwrap(), 0);
        assert_eq!(decode_file(&headers_only).unwrap(), (vec![], true));

        // Not ours: refused, untouched.
        for junk in [
            b"not an ogg file, just text".to_vec(),
            // An Ogg page that isn't our OpusHead (a Vorbis-like stream).
            {
                let mut p = b"OggS\0\x02".to_vec();
                p.extend_from_slice(&[0u8; 20]);
                p.push(1);
                p.push(7);
                p.extend_from_slice(b"\x01vorbis");
                let crc = ogg_crc(&p);
                p[22..26].copy_from_slice(&crc.to_le_bytes());
                p
            },
        ] {
            let path = dir.path().join("audio-y.opus");
            std::fs::write(&path, &junk).unwrap();
            assert!(repair(&path).is_err());
            assert_eq!(std::fs::read(&path).unwrap(), junk);
        }
    }

    // ---- reader (#248) ----

    /// A finished file of `seconds` of the test voice with a silent gap.
    pub(crate) fn written(dir: &Path, name: &str, samples: usize) -> PathBuf {
        let path = dir.join(name);
        let mut w = OpusWriter::create_capped(&path, u64::MAX).unwrap();
        w.write(&voice(samples, 0)).unwrap();
        w.finish().unwrap();
        path
    }

    fn read_all(r: &mut OpusReader, chunk: usize) -> Vec<f32> {
        let mut out = Vec::new();
        while r.read(&mut out, chunk).unwrap() > 0 {}
        out
    }

    /// Signal-to-noise ratio of `got` against `want`, in dB.
    fn snr(want: &[f32], got: &[f32]) -> f64 {
        let s: f64 = want.iter().map(|&x| (x as f64).powi(2)).sum();
        let n: f64 = want
            .iter()
            .zip(got)
            .map(|(&a, &b)| (a as f64 - b as f64).powi(2))
            .sum();
        10.0 * (s / n.max(1e-20)).log10()
    }

    #[test]
    fn reader_matches_a_full_decode_exactly() {
        let dir = tempfile::tempdir().unwrap();
        let path = written(dir.path(), "audio.opus", 7 * RATE as usize + 1_234);
        let (full, _) = decode_file(&path).unwrap();
        let mut r = OpusReader::open(&path).unwrap();
        assert_eq!(r.total_samples(), full.len() as u64);
        assert_eq!(r.total_samples(), 7 * RATE as u64 + 1_234);
        // Uneven reads, as the scheme's byte ranges make them.
        for chunk in [1usize, 7, 1_000, 16_001] {
            let mut r = OpusReader::open(&path).unwrap();
            assert_eq!(read_all(&mut r, chunk), full, "chunk {chunk}");
            assert_eq!(r.filled(), 0);
            assert_eq!(r.position(), full.len() as u64);
        }
        assert_eq!(verify(&path).unwrap(), full.len() as u64);
        // Reading continues across a close (the scheme's cache closes the
        // file between requests).
        let mut out = Vec::new();
        r.read(&mut out, 40_000).unwrap();
        r.close();
        while r.read(&mut out, 9_999).unwrap() > 0 {
            r.close();
        }
        assert_eq!(out, full);
    }

    #[test]
    fn seeks_land_on_the_exact_sample() {
        let dir = tempfile::tempdir().unwrap();
        let path = written(dir.path(), "audio-mic.opus", 12 * RATE as usize + 77);
        let (full, _) = decode_file(&path).unwrap();
        let total = full.len() as u64;
        let mut r = OpusReader::open(&path).unwrap();
        let mut worst = f64::INFINITY;
        // Backwards, far ahead, near the start, on and near page edges.
        for t in [
            9 * RATE as u64 + 5,
            1_000,
            RATE as u64 - 1,
            4 * RATE as u64,
            11 * RATE as u64 + 3_000,
            2 * RATE as u64 + 17,
            0,
            total - 100,
        ] {
            r.seek(t).unwrap();
            assert_eq!(r.position(), t);
            let mut got = Vec::new();
            let n = r.read(&mut got, 8_000).unwrap();
            assert_eq!(n as u64, 8_000.min(total - t), "at {t}");
            let want = &full[t as usize..t as usize + n];
            if t < PRE_ROLL {
                // Restarted from the first page: exactly a linear decode.
                assert_eq!(got, want, "at {t}");
            } else {
                worst = worst.min(snr(want, &got));
            }
        }
        assert!(worst > 35.0, "worst seek {worst:.1} dB");
        // A short seek ahead decodes on: still bit-exact.
        let mut r = OpusReader::open(&path).unwrap();
        let mut got = Vec::new();
        r.read(&mut got, 5_000).unwrap();
        r.seek(5_000 + 20_000).unwrap();
        got.clear();
        r.read(&mut got, 3_000).unwrap();
        assert_eq!(got, &full[25_000..28_000]);
        // At and past the end: nothing more.
        r.seek(total + 50).unwrap();
        assert_eq!(r.position(), total);
        assert_eq!(r.read(&mut got, 10).unwrap(), 0);
    }

    #[test]
    fn reader_handles_crashed_damaged_and_foreign_files() {
        let dir = tempfile::tempdir().unwrap();
        // A crashed file (no end-of-stream page) reads up to its last page.
        let crashed = dir.path().join("audio-remote.opus");
        let mut w = OpusWriter::create_capped(&crashed, u64::MAX).unwrap();
        w.write(&voice(3 * RATE as usize + 500, 0)).unwrap();
        std::mem::forget(w);
        let (partial, complete) = decode_file(&crashed).unwrap();
        assert!(!complete);
        let mut r = OpusReader::open(&crashed).unwrap();
        assert_eq!(read_all(&mut r, 4_096), partial);

        // A damaged page in the middle: the length holds, the page reads
        // as silence, and verify refuses the file.
        let path = written(dir.path(), "audio.opus", 6 * RATE as usize);
        let (full, _) = decode_file(&path).unwrap();
        let mut bytes = std::fs::read(&path).unwrap();
        let pages: Vec<usize> = bytes
            .windows(4)
            .enumerate()
            .filter(|(_, w)| *w == b"OggS")
            .map(|(i, _)| i)
            .collect();
        let hit = pages[4] + 300; // inside the third audio page's body
        bytes[hit] ^= 0x5a;
        std::fs::write(&path, &bytes).unwrap();
        let mut r = OpusReader::open(&path).unwrap();
        let got = read_all(&mut r, 3_333);
        assert_eq!(got.len(), full.len());
        assert!(r.filled() >= RATE as u64 / 2, "{}", r.filled());
        assert!(verify(&path).is_err());
        // Pages after the damaged one decode again (decoder reset).
        let tail = 5 * RATE as usize..6 * RATE as usize - 400;
        assert!(snr(&full[tail.clone()], &got[tail]) > 20.0);

        // Not Ogg Opus.
        let wav = dir.path().join("audio.wav");
        std::fs::write(&wav, crate::archive::audio::wav_bytes(&[0.1; 100])).unwrap();
        assert!(OpusReader::open(&wav).is_err());
        let cut = dir.path().join("audio-x.opus");
        std::fs::write(&cut, &bytes[..30]).unwrap();
        assert!(OpusReader::open(&cut).is_err());
        // An empty stream.
        let empty = dir.path().join("audio-e.opus");
        OpusWriter::create_capped(&empty, u64::MAX)
            .unwrap()
            .finish()
            .unwrap();
        let mut r = OpusReader::open(&empty).unwrap();
        assert_eq!(r.total_samples(), 0);
        assert_eq!(read_all(&mut r, 100), Vec::<f32>::new());
    }
}
