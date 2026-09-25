//! *Compress audio* (0.11, #248, P16): an item's saved WAV files become
//! Ogg Opus (24 kb/s, about 10× smaller), one item or the whole archive.
//!
//! **Per file** (`audio.wav` → `audio.opus`, `audio-mic.wav` →
//! `audio-mic.opus`; the channel layout and the t = 0 clock stay):
//!
//! 1. The WAV must be a finished file of ours (canonical header whose size
//!    matches the file — a crashed one is repaired at startup first).
//! 2. It is encoded with the saved-audio writer ([`OpusWriter`]) into a
//!    temporary file in the item's `.sussurro/` folder (never an audio
//!    name, so nothing lists or plays it), streamed in 64 KiB blocks.
//! 3. The copy is decoded through ([`super::opus::verify`]): every sample
//!    must decode and the length must be the WAV's exactly (the writer
//!    trims the end, so it is).
//! 4. Under the archive lock, with the item not being recorded and the WAV
//!    unchanged since step 1, the copy is renamed into place and the WAV
//!    goes to the **OS trash** (like every deletion in the archive; if the
//!    trash fails, the new copy is removed and the WAV stays).
//!
//! Encoding runs outside the lock (an hour takes seconds), so the checks of
//! step 4 are repeated there. A cancel (checked between blocks) or any
//! failure removes the temporary file and leaves the WAV as it was. If the
//! app stopped between the rename and the trash, both files exist: the next
//! run keeps the Opus copy only if it verifies against the WAV, then
//! trashes the WAV. The frontmatter's `audio:` list (a record of the files,
//! the extension naming the format) is rewritten when it exists.

use super::audio::{self, AudioFormat, HEADER_LEN, MAX_SAMPLES};
use super::opus::{self, OpusWriter};
use super::store::{self, lock_items, META_DIR};
use anyhow::{bail, Context, Result};
use serde::Serialize;
use std::io::{BufReader, Read};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// WAV bytes read per block (between two cancel checks).
const BLOCK: usize = 64 * 1024;
/// Prefix and suffix of the temporary files in `.sussurro/`.
const TMP_PREFIX: &str = "compress-";
const TMP_SUFFIX: &str = ".part";

/// What compressing one item did.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Compressed {
    /// WAV files replaced by Opus.
    pub files: usize,
    /// Their size, and the size of the Opus files that replaced them.
    pub bytes_before: u64,
    pub bytes_after: u64,
    /// Stopped by a cancel (files already done stay done).
    pub cancelled: bool,
}

/// The item's WAV files (what Compress audio converts), by name.
pub fn wav_files(dir: &Path) -> Vec<String> {
    audio::files_in(dir)
        .into_iter()
        .filter(|n| AudioFormat::of_name(n) == Some(AudioFormat::Wav))
        .collect()
}

/// Total size of the item's WAV files.
pub fn wav_bytes(dir: &Path) -> u64 {
    wav_files(dir)
        .iter()
        .map(|n| std::fs::metadata(dir.join(n)).map(|m| m.len()).unwrap_or(0))
        .sum()
}

/// Items of the archive that hold WAV audio: `(id, WAV bytes)`.
pub fn items_with_wav(archive: &Path) -> Vec<(String, u64)> {
    store::scan_item_dirs(archive)
        .into_iter()
        .filter_map(|(id, dir)| {
            let bytes = wav_bytes(&dir);
            (bytes > 0).then_some((id, bytes))
        })
        .collect()
}

/// Refuse an item a session is writing (its frontmatter marker; the app
/// also checks the session journal before calling in).
fn ensure_not_recording(dir: &Path, id: &str) -> Result<()> {
    let path = store::transcript_path(dir);
    let text = std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    if let Ok((m, _)) = super::frontmatter::parse(&text) {
        if m.session_state() == Some(super::types::SessionState::Recording) {
            bail!("'{id}' is still being recorded — compress its audio when the session ends");
        }
    }
    Ok(())
}

/// Size and modification time: "unchanged since" for the final check.
fn stamp(path: &Path) -> Result<(u64, Option<std::time::SystemTime>)> {
    let m = std::fs::metadata(path).with_context(|| format!("reading {}", path.display()))?;
    Ok((m.len(), m.modified().ok()))
}

/// Samples of a finished WAV of ours; refuses anything else.
fn wav_samples(path: &Path) -> Result<u64> {
    let mut f = std::fs::File::open(path).with_context(|| format!("opening {}", path.display()))?;
    let len = f.metadata()?.len();
    let mut h = [0u8; HEADER_LEN as usize];
    f.read_exact(&mut h)
        .ok()
        .filter(|_| audio::is_our_header(&h))
        .with_context(|| format!("{} is not a WAV written by Sussurro", path.display()))?;
    let data = u32::from_le_bytes([h[40], h[41], h[42], h[43]]) as u64;
    if HEADER_LEN + data != len || !data.is_multiple_of(2) {
        bail!(
            "{} is not a finished WAV (its header says {data} bytes of audio, the file holds {})",
            path.display(),
            len.saturating_sub(HEADER_LEN)
        );
    }
    Ok(data / 2)
}

/// Encode the WAV at `wav` as Opus into `out`, reporting WAV bytes read.
/// Returns the samples encoded.
fn encode(
    wav: &Path,
    out: &Path,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(u64),
) -> Result<u64> {
    let samples = wav_samples(wav)?;
    let mut r = BufReader::with_capacity(BLOCK, std::fs::File::open(wav)?);
    std::io::copy(&mut (&mut r).take(HEADER_LEN), &mut std::io::sink())?;
    progress(HEADER_LEN);
    let mut w = OpusWriter::create_capped(out, MAX_SAMPLES)?;
    let mut bytes = vec![0u8; BLOCK];
    let mut floats = Vec::with_capacity(BLOCK / 2);
    let mut left = samples * 2;
    while left > 0 {
        if cancel.load(Ordering::Relaxed) {
            return Err(anyhow::Error::new(Cancelled));
        }
        let n = (left as usize).min(BLOCK);
        r.read_exact(&mut bytes[..n])
            .with_context(|| format!("reading {}", wav.display()))?;
        floats.clear();
        floats.extend(
            bytes[..n]
                .chunks_exact(2)
                .map(|b| i16::from_le_bytes([b[0], b[1]]) as f32 / i16::MAX as f32),
        );
        w.write(&floats)?;
        left -= n as u64;
        progress(n as u64);
    }
    w.finish()
}

/// Compress audio was cancelled.
#[derive(Debug)]
pub struct Cancelled;

impl std::fmt::Display for Cancelled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("cancelled")
    }
}

impl std::error::Error for Cancelled {}

/// Compress every WAV file of item `id` (see the module docs).
/// `progress` gets the WAV bytes processed as they are; `cancel` stops
/// between blocks (the file in progress stays WAV).
pub fn compress_item(
    archive: &Path,
    id: &str,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(u64),
) -> Result<Compressed> {
    compress_item_with(archive, id, cancel, progress, &store::move_to_trash)
}

/// [`compress_item`] with the trash step given (tests of its failure).
pub(crate) fn compress_item_with(
    archive: &Path,
    id: &str,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(u64),
    trash: &dyn Fn(&Path) -> Result<()>,
) -> Result<Compressed> {
    let (dir, wavs) = {
        let _lock = lock_items();
        let dir = store::existing_item_dir(archive, id)?;
        ensure_not_recording(&dir, id)?;
        let wavs = wav_files(&dir);
        (dir, wavs)
    };
    let mut done = Compressed::default();
    if wavs.is_empty() {
        return Ok(done);
    }
    let tmp_dir = dir.join(META_DIR);
    std::fs::create_dir_all(&tmp_dir)?;
    remove_leftovers(&tmp_dir);
    let result = (|| -> Result<()> {
        for name in &wavs {
            if cancel.load(Ordering::Relaxed) {
                return Err(anyhow::Error::new(Cancelled));
            }
            let (before, after) = compress_file(&dir, id, name, &tmp_dir, cancel, progress, trash)?;
            done.files += 1;
            done.bytes_before += before;
            done.bytes_after += after;
        }
        Ok(())
    })();
    if done.files > 0 {
        relist(&dir, id);
    }
    match result {
        Ok(()) => Ok(done),
        Err(e) if e.downcast_ref::<Cancelled>().is_some() => {
            done.cancelled = true;
            Ok(done)
        }
        Err(e) => Err(e),
    }
}

/// Temporary files a previous run left (the app stopped mid-encode).
fn remove_leftovers(tmp_dir: &Path) {
    let Ok(entries) = std::fs::read_dir(tmp_dir) else {
        return;
    };
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if name.starts_with(TMP_PREFIX) && name.ends_with(TMP_SUFFIX) {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

/// One WAV → Opus (steps 1–4 of the module docs). Returns the sizes before
/// and after.
fn compress_file(
    dir: &Path,
    id: &str,
    name: &str,
    tmp_dir: &Path,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(u64),
    trash: &dyn Fn(&Path) -> Result<()>,
) -> Result<(u64, u64)> {
    let wav = dir.join(name);
    let stem = name.strip_suffix(".wav").unwrap_or(name);
    let target_name = format!("{stem}.opus");
    let target = dir.join(&target_name);
    let before = stamp(&wav)?;

    // Both there: a run stopped between the rename and the trash.
    if target.exists() {
        let samples = wav_samples(&wav)?;
        match opus::verify(&target) {
            Ok(n) if n == samples => {
                let _lock = lock_items();
                ensure_not_recording(dir, id)?;
                if stamp(&wav)? != before {
                    bail!("{name} changed while it was being compressed");
                }
                trash(&wav).with_context(|| format!("moving {name} to the trash"))?;
                progress(before.0);
                return Ok((before.0, stamp(&target)?.0));
            }
            _ => bail!(
                "'{id}' already has {target_name}, which doesn't match {name}: left as they are"
            ),
        }
    }

    let tmp = tmp_dir.join(format!("{TMP_PREFIX}{target_name}{TMP_SUFFIX}"));
    let _ = std::fs::remove_file(&tmp);
    let made = (|| -> Result<()> {
        let samples = encode(&wav, &tmp, cancel, progress)?;
        let decoded = opus::verify(&tmp)?;
        if decoded != samples {
            bail!("the Opus copy of {name} decodes to {decoded} samples, not {samples}");
        }
        let _lock = lock_items();
        ensure_not_recording(dir, id)?;
        if stamp(&wav)? != before {
            bail!("{name} changed while it was being compressed");
        }
        if target.exists() {
            bail!("'{id}' already has {target_name}");
        }
        std::fs::rename(&tmp, &target).with_context(|| format!("saving {target_name}"))?;
        if let Err(e) = trash(&wav) {
            // Keep the WAV: the new copy goes (ours, made a moment ago).
            let _ = std::fs::remove_file(&target);
            return Err(e.context(format!("moving {name} to the trash")));
        }
        Ok(())
    })();
    if let Err(e) = made {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    Ok((before.0, stamp(&target)?.0))
}

/// Rewrite the frontmatter's `audio:` list after files changed format (only
/// when the item has the list). The files are converted already, so a
/// frontmatter that can't be rewritten is left to the user, as for Delete
/// audio.
fn relist(dir: &Path, id: &str) {
    let _lock = lock_items();
    let path = store::transcript_path(dir);
    let Ok(text) = std::fs::read_to_string(&path) else {
        return;
    };
    let listed = super::frontmatter::parse(&text)
        .map(|(m, _)| m.extra.contains_key(audio::AUDIO_KEY))
        .unwrap_or(false);
    if !listed {
        return;
    }
    let files = audio::files_in(dir);
    let segments = store::read_segments(dir).unwrap_or_default();
    if let Err(e) = super::live::rewrite_meta(
        dir,
        &segments,
        None,
        &|mut m| {
            audio::set_listed(&mut m, &files);
            m
        },
        &|_| {},
    ) {
        eprintln!("archive: {id}: audio compressed, frontmatter not updated ({e:#})");
    }
}

// ---- one job at a time -------------------------------------------------------

static JOB: Mutex<Option<Arc<AtomicBool>>> = Mutex::new(None);

/// A running Compress audio job; ends when dropped.
pub struct Job(Arc<AtomicBool>);

impl Job {
    /// Start a job; refused while another runs (one encode at a time keeps
    /// the app responsive and the progress readable).
    pub fn begin() -> Result<Self> {
        let mut job = JOB.lock().unwrap_or_else(|e| e.into_inner());
        if job.is_some() {
            bail!("audio is already being compressed — wait for it to finish or cancel it");
        }
        let flag = Arc::new(AtomicBool::new(false));
        *job = Some(flag.clone());
        Ok(Self(flag))
    }

    pub fn cancel_flag(&self) -> &AtomicBool {
        &self.0
    }
}

impl Drop for Job {
    fn drop(&mut self) {
        let mut job = JOB.lock().unwrap_or_else(|e| e.into_inner());
        if job.as_ref().is_some_and(|f| Arc::ptr_eq(f, &self.0)) {
            *job = None;
        }
    }
}

/// Ask the running job to stop; false when none runs.
pub fn cancel() -> bool {
    match JOB.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
        Some(flag) => {
            flag.store(true, Ordering::Relaxed);
            true
        }
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::archive::audio::{listed, set_listed, WavWriter, RATE};
    use crate::archive::store::test_trash;
    use crate::archive::{create_item, read_item, ItemMeta, SegmentsFile, SessionState};
    use std::path::PathBuf;

    /// The temporary file of `target_name`.
    fn tmp_path(dir: &Path, target_name: &str) -> PathBuf {
        dir.join(META_DIR)
            .join(format!("{TMP_PREFIX}{target_name}{TMP_SUFFIX}"))
    }

    fn speech(n: usize, from: usize) -> Vec<f32> {
        (from..from + n)
            .map(|i| {
                let t = i as f32 / RATE as f32;
                let f = 300.0 + 120.0 * (t * 0.9).sin();
                (0.25 + 0.1 * (t * 3.0).sin()) * (t * f * std::f32::consts::TAU).sin()
            })
            .collect()
    }

    /// An item with WAV files `names` of `samples` each, listed in the
    /// frontmatter.
    fn item(archive: &Path, names: &[&str], samples: usize) -> (String, PathBuf) {
        let mut meta = ItemMeta {
            title: "Sync".into(),
            date: "2026-09-25T10:00:00+02:00".into(),
            ..Default::default()
        };
        let files: Vec<String> = names.iter().map(|n| n.to_string()).collect();
        set_listed(&mut meta, &files);
        let id = create_item(archive, &meta, &SegmentsFile::default()).unwrap();
        let dir = archive.join(&id);
        for (k, name) in names.iter().enumerate() {
            let mut w = WavWriter::create(&dir.join(name)).unwrap();
            w.write(&speech(samples, k * 7_000)).unwrap();
            w.finish().unwrap();
        }
        (id, dir)
    }

    fn decode_wav(path: &Path) -> Vec<f32> {
        let bytes = std::fs::read(path).unwrap();
        bytes[44..]
            .chunks_exact(2)
            .map(|b| i16::from_le_bytes([b[0], b[1]]) as f32 / i16::MAX as f32)
            .collect()
    }

    fn corr(a: &[f32], b: &[f32]) -> f32 {
        let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
        let na: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
        let nb: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
        dot / (na * nb).max(1e-9)
    }

    #[test]
    fn compress_round_trip_keeps_length_and_trashes_the_wav() {
        let tmp = tempfile::tempdir().unwrap();
        let archive = tmp.path().join("Sussurro");
        let n = 5 * RATE as usize + 321;
        let (id, dir) = item(&archive, &["audio-mic.wav", "audio-remote.wav"], n);
        let originals: Vec<Vec<f32>> = ["audio-mic.wav", "audio-remote.wav"]
            .iter()
            .map(|f| decode_wav(&dir.join(f)))
            .collect();
        let wav_total = wav_bytes(&dir);
        assert_eq!(items_with_wav(&archive), vec![(id.clone(), wav_total)]);

        let mut seen = 0u64;
        let done = compress_item(&archive, &id, &AtomicBool::new(false), &mut |b| seen += b).unwrap();
        assert_eq!(done.files, 2);
        assert!(!done.cancelled);
        assert_eq!(done.bytes_before, wav_total);
        assert_eq!(seen, wav_total, "progress covers every WAV byte");
        assert!(done.bytes_after * 5 < done.bytes_before, "{done:?}");

        for (name, original) in ["audio-mic", "audio-remote"].iter().zip(&originals) {
            let wav = dir.join(format!("{name}.wav"));
            assert!(!wav.exists());
            assert!(test_trash::contains(&wav), "{name}.wav to the trash");
            let (pcm, complete) = opus::decode_file(&dir.join(format!("{name}.opus"))).unwrap();
            assert!(complete);
            // Same duration to the sample (the criterion is 20 ms).
            assert_eq!(pcm.len(), n);
            let mid = RATE as usize..4 * RATE as usize;
            // The same audio (a lossy copy: the writer's own tests check
            // alignment to the sample), at the same level.
            let c = corr(&original[mid.clone()], &pcm[mid.clone()]);
            assert!(c > 0.7, "{name}: {c}");
            let rms = |x: &[f32]| (x.iter().map(|s| s * s).sum::<f32>() / x.len() as f32).sqrt();
            let ratio = rms(&pcm[mid.clone()]) / rms(&original[mid]);
            assert!((0.8..1.25).contains(&ratio), "{name}: level ×{ratio}");
        }
        // Frontmatter and the item's files follow; no temporary file left.
        let item = read_item(&archive, &id).unwrap();
        assert_eq!(listed(&item.meta), vec!["audio-mic.opus", "audio-remote.opus"]);
        let names: Vec<&str> = item.audio.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names, ["audio-mic.opus", "audio-remote.opus"]);
        assert!(!tmp_path(&dir, "audio-mic.opus").exists());
        assert!(items_with_wav(&archive).is_empty());
        // Nothing left to do: a second run is a no-op.
        let again = compress_item(&archive, &id, &AtomicBool::new(false), &mut |_| {}).unwrap();
        assert_eq!(again, Compressed::default());
    }

    #[test]
    fn a_cancel_leaves_the_wav_and_no_leftovers() {
        let tmp = tempfile::tempdir().unwrap();
        let archive = tmp.path().join("Sussurro");
        let (id, dir) = item(&archive, &["audio.wav"], 4 * RATE as usize);
        let before = std::fs::read(dir.join("audio.wav")).unwrap();
        let cancel = AtomicBool::new(false);
        // Cancel after the first block.
        let done = compress_item(&archive, &id, &cancel, &mut |b| {
            if b > HEADER_LEN {
                cancel.store(true, Ordering::Relaxed);
            }
        })
        .unwrap();
        assert!(done.cancelled);
        assert_eq!(done.files, 0);
        assert_eq!(std::fs::read(dir.join("audio.wav")).unwrap(), before);
        assert!(!dir.join("audio.opus").exists());
        assert!(!tmp_path(&dir, "audio.opus").exists());
        assert_eq!(listed(&read_item(&archive, &id).unwrap().meta), vec!["audio.wav"]);
    }

    #[test]
    fn failures_leave_the_wav_in_place() {
        let tmp = tempfile::tempdir().unwrap();
        let archive = tmp.path().join("Sussurro");
        let no = AtomicBool::new(false);

        // The trash refuses: the WAV stays, the new Opus copy goes.
        let (id, dir) = item(&archive, &["audio.wav"], RATE as usize);
        let failing = |_: &Path| -> Result<()> { bail!("no trash here") };
        let err = compress_item_with(&archive, &id, &no, &mut |_| {}, &failing).unwrap_err();
        assert!(format!("{err:#}").contains("no trash here"), "{err:#}");
        assert!(dir.join("audio.wav").exists());
        assert!(!dir.join("audio.opus").exists());
        assert!(!tmp_path(&dir, "audio.opus").exists());

        // A WAV that isn't finished (header sizes don't match): refused.
        let (id, dir) = item(&archive, &["audio.wav"], RATE as usize);
        let wav = dir.join("audio.wav");
        let mut bytes = std::fs::read(&wav).unwrap();
        bytes.extend_from_slice(&[0u8; 100]);
        std::fs::write(&wav, &bytes).unwrap();
        let err = compress_item(&archive, &id, &no, &mut |_| {}).unwrap_err();
        assert!(err.to_string().contains("not a finished WAV"), "{err:#}");
        assert_eq!(std::fs::read(&wav).unwrap(), bytes);
        assert!(!dir.join("audio.opus").exists());

        // Not ours at all.
        std::fs::write(&wav, b"RIFF....something else entirely, not ours").unwrap();
        assert!(compress_item(&archive, &id, &no, &mut |_| {}).is_err());
        assert!(wav.exists());

        // Being recorded: refused before anything is read.
        let (id, dir) = item(&archive, &["audio.wav"], RATE as usize);
        let segments = store::read_segments(&dir).unwrap();
        crate::archive::live::rewrite_meta(
            &dir,
            &segments,
            None,
            &|mut m| {
                m.set_session_state(Some(SessionState::Recording));
                m
            },
            &|_| {},
        )
        .unwrap();
        let err = compress_item(&archive, &id, &no, &mut |_| {}).unwrap_err();
        assert!(err.to_string().contains("still being recorded"), "{err:#}");
        assert!(dir.join("audio.wav").exists());
        assert!(!dir.join("audio.opus").exists());

        // Unknown item.
        assert!(compress_item(&archive, "2026/09/nope", &no, &mut |_| {}).is_err());
    }

    #[test]
    fn an_interrupted_run_is_finished_or_left_alone() {
        let tmp = tempfile::tempdir().unwrap();
        let archive = tmp.path().join("Sussurro");
        let no = AtomicBool::new(false);
        let n = 2 * RATE as usize;

        // Stopped after the rename: the Opus copy matches, the WAV goes.
        let (id, dir) = item(&archive, &["audio.wav"], n);
        let keep = tmp.path().join("copy.opus");
        encode(&dir.join("audio.wav"), &keep, &no, &mut |_| {}).unwrap();
        std::fs::rename(&keep, dir.join("audio.opus")).unwrap();
        // And a temporary file of a run stopped mid-encode.
        std::fs::write(tmp_path(&dir, "audio.opus"), b"half").unwrap();
        let done = compress_item(&archive, &id, &no, &mut |_| {}).unwrap();
        assert_eq!(done.files, 1);
        assert!(test_trash::contains(&dir.join("audio.wav")));
        assert_eq!(opus::verify(&dir.join("audio.opus")).unwrap(), n as u64);
        assert!(!tmp_path(&dir, "audio.opus").exists());

        // An Opus file that doesn't match the WAV: both left as they are.
        let (id, dir) = item(&archive, &["audio.wav"], n);
        let mut w = OpusWriter::create_capped(&dir.join("audio.opus"), u64::MAX).unwrap();
        w.write(&speech(100, 0)).unwrap();
        w.finish().unwrap();
        let err = compress_item(&archive, &id, &no, &mut |_| {}).unwrap_err();
        assert!(err.to_string().contains("doesn't match"), "{err:#}");
        assert!(dir.join("audio.wav").exists());
        assert_eq!(opus::verify(&dir.join("audio.opus")).unwrap(), 100);
    }

    #[test]
    fn one_job_at_a_time_and_cancel_reaches_it() {
        // Other tests don't use the job registry.
        assert!(!cancel());
        let job = Job::begin().unwrap();
        assert!(Job::begin().is_err());
        assert!(!job.cancel_flag().load(Ordering::Relaxed));
        assert!(cancel());
        assert!(job.cancel_flag().load(Ordering::Relaxed));
        drop(job);
        assert!(!cancel());
        let again = Job::begin().unwrap();
        assert!(!again.cancel_flag().load(Ordering::Relaxed));
    }
}
