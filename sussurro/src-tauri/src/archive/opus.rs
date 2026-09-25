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
}
