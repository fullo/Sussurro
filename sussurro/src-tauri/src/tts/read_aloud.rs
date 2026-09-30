//! Read aloud (0.12, #256, #327, P17): an item's transcript or one of its
//! companion documents becomes speech — saved next to the item
//! (`speech.opus`, `speech-<document>.opus`, see [`crate::archive::speech`]).
//! There is one button per document in the Audio tab: *Create* when there
//! is no speech for it yet, *Listen* (playing the saved file, no
//! generation) once there is. The temporary *Listen* path of #256 (`save:
//! false`, `listen-N.opus`, `read_aloud_discard`) was removed in #327 —
//! every job now saves.
//!
//! One narrator voice per document (P17): the voice picked for the text's
//! language in Models → Voices ([`super::service::voice_for`]). The language
//! is the item's (`language:` in the frontmatter) unless the user picks one;
//! only languages with a read-aloud model can be read. **P24**: nothing here
//! downloads anything — a missing model or voice is an error that sends the
//! user to Models → Voices, and the commands refuse while the module is off.
//!
//! **The job**: one at a time (the engine is loaded once), in the
//! background, with progress by chunk and cancel between chunks (and inside
//! Pocket's generation loop). The text goes through
//! [`super::text::prepare`] (#254), each chunk is spoken, passed through
//! the marking hook ([`super::marking::Marker::process`], P21: tags plus
//! the AudioSeal watermark of #257 — no watermark model, no speech) and encoded
//! to Ogg Opus at 24 kHz ([`crate::archive::opus::SPEECH`], #309: Pocket's
//! own rate, so its 8–12 kHz band is kept) with the synthetic-speech
//! comments. An engine at another rate is resampled to 24 kHz first
//! ([`super::resample`]). The finished file is then **signed** (#257 part
//! 2, [`super::signing`]): a `.c2pa` sidecar with the same stem, made with
//! this install's key — created on this first need; without a working
//! credential store the file is kept unsigned and the frontmatter says why
//! (`unsigned:`). A saved file and its sidecar are written to the item's
//! `.sussurro/` and moved into place together under the archive lock; a
//! cancelled or failed run leaves nothing.
//!
//! **Out of date**: the frontmatter records the SHA-256 of the speakable
//! text that was read ([`prepared`]); when the document's text changes
//! later, [`statuses`] reports the speech as `stale` and the UI says so.
//! Only what is read counts — a new tag or a frontmatter edit doesn't.

use super::catalog::{self, Language, Voice};
use super::engine::{self, TtsEngine};
use super::marking::{self, Marker, Provenance};
use super::resample::Resampler;
use super::service::{self, ENGINE_NAME};
use super::signing::{self, Identity};
use super::text::{self, Chunk, Lang, PrepOptions};
use super::watermark::{AudioSealGenerator, WatermarkModel};
use crate::archive::audio::MAX_SAMPLES;
use crate::archive::opus::{OpusWriter, SPEECH, SPEECH_RATE};
use crate::archive::speech::{self, SpeechInfo};
use crate::archive::store::{existing_item_dir, sha256_hex, TRANSCRIPT_FILE};
use anyhow::{bail, Context, Result};
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

/// Bumped when [`prepared`] changes what it hashes, so speech made by an
/// older build is not wrongly reported as current.
const TEXT_HASH_VERSION: &str = "read-aloud-text v1";

/// A document to read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Source {
    /// `transcript.md` or a companion document's name.
    pub document: String,
    /// Its markdown (the transcript's body, or the companion's).
    pub markdown: String,
    /// The item's `language:`.
    pub item_language: String,
    /// A session is writing the item.
    pub recording: bool,
}

/// The markdown of `document` (default: the transcript) of item `id`.
pub fn read_source(archive: &Path, id: &str, document: Option<&str>) -> Result<Source> {
    let item = crate::archive::read_item(archive, id)?;
    let document = document.unwrap_or(TRANSCRIPT_FILE).to_string();
    let markdown = if document == TRANSCRIPT_FILE {
        item.body
    } else {
        crate::archive::companion::read_companion(archive, id, &document)?.body
    };
    Ok(Source {
        document,
        markdown,
        item_language: item.meta.language,
        recording: item.recording,
    })
}

/// `markdown` as the chunks read in `lang`, and the hash of what they say
/// (the record that tells when the speech is out of date). Pure.
pub fn prepared(markdown: &str, lang: &str) -> (Vec<Chunk>, String) {
    let chunks = text::prepare(markdown, Lang::from_code(lang), &PrepOptions::default());
    let mut h = format!("{TEXT_HASH_VERSION}\n{lang}\n");
    for c in &chunks {
        h.push_str(&c.text);
        h.push('\n');
    }
    let hash = sha256_hex(h.as_bytes());
    (chunks, hash)
}

/// The read-aloud language for a request: the one asked for, else the
/// item's. Errors when read aloud has no model for it.
pub fn resolve_language(requested: Option<&str>, item_language: &str) -> Result<&'static Language> {
    let code = requested
        .map(str::trim)
        .filter(|c| !c.is_empty())
        .unwrap_or(item_language.trim());
    catalog::language(code).with_context(|| {
        let names: Vec<&str> = catalog::LANGUAGES.iter().map(|l| l.label).collect();
        let what = if code.is_empty() || code == "auto" {
            "This item has no language set".to_string()
        } else {
            format!("Read aloud has no voice for '{code}'")
        };
        format!("{what} — pick one of {} to read it in.", names.join(", "))
    })
}

// ---- the engine, behind a seam tests can fill ------------------------------

/// Runs `f` on an engine ready to speak `lang` with `voice`.
pub trait Speaker {
    /// The watermark model every generated file goes through (#257). An
    /// error — the model is missing — stops the job before anything is
    /// spoken (fail closed).
    fn watermark(&self) -> Result<Box<dyn WatermarkModel>>;

    /// This install's signing identity (#257 part 2), asked for only once a
    /// file has been spoken. `Err` = the file stays unsigned, for this
    /// reason (no working credential store).
    fn signer(&self) -> Result<Arc<Identity>, String>;

    fn speak_with(
        &self,
        lang: &'static Language,
        voice: &'static Voice,
        f: &mut dyn FnMut(&mut dyn TtsEngine) -> Result<()>,
    ) -> Result<()>;
}

/// The app's engine: Pocket TTS through [`service::Service`], its models
/// from `models_dir` (never downloaded here).
pub struct PocketSpeaker<'a> {
    pub service: &'a service::Service,
    pub models_dir: &'a Path,
    pub options: super::pocket::PocketOptions,
    /// This install's signing identity (in the app:
    /// [`signing::identity`] over `<app data>/c2pa` and the OS store).
    pub signer: &'a (dyn Fn() -> Result<Arc<Identity>, String> + Sync),
}

impl Speaker for PocketSpeaker<'_> {
    fn watermark(&self) -> Result<Box<dyn WatermarkModel>> {
        load_watermark(self.models_dir)
    }

    fn signer(&self) -> Result<Arc<Identity>, String> {
        (self.signer)()
    }

    fn speak_with(
        &self,
        lang: &'static Language,
        voice: &'static Voice,
        f: &mut dyn FnMut(&mut dyn TtsEngine) -> Result<()>,
    ) -> Result<()> {
        if !super::models::model_present(self.models_dir, lang)
            || !super::models::voice_present(self.models_dir, lang, voice)
        {
            bail!(
                "The {} read-aloud voice {} is not downloaded — download it in Models → Voices.",
                lang.label,
                voice.label
            );
        }
        self.service
            .with_engine(self.models_dir, lang.code, voice.id, self.options, |e| {
                // The same text always sounds the same.
                e.reseed(self.options.seed);
                f(e)
            })
    }
}

/// The AudioSeal generator from `models_dir`, or the error that sends the
/// user to Models → Voices (never downloaded here, P24).
pub fn load_watermark(models_dir: &Path) -> Result<Box<dyn WatermarkModel>> {
    if !super::models::watermark_present(models_dir) {
        bail!(
            "The watermark model that marks generated speech is not downloaded — download it in Models → Voices."
        );
    }
    let path = super::models::watermark_path(models_dir, &catalog::WATERMARK_GENERATOR);
    Ok(Box::new(AudioSealGenerator::load(&path)?))
}

// ---- the job -----------------------------------------------------------------

/// The read-aloud job in progress, as the UI shows it.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct JobStatus {
    pub item_id: String,
    pub document: String,
    pub language: String,
    pub voice: String,
    /// Chunks spoken so far, of `total`.
    pub done: usize,
    pub total: usize,
}

/// One read-aloud job at a time, and its cancel flag.
#[derive(Default)]
pub struct Jobs {
    job: Mutex<Option<JobStatus>>,
    cancel: AtomicBool,
}

/// The app's jobs.
pub fn jobs() -> &'static Jobs {
    static J: OnceLock<Jobs> = OnceLock::new();
    J.get_or_init(Jobs::default)
}

impl Jobs {
    fn begin(&self, status: JobStatus) -> Result<JobGuard<'_>> {
        let mut job = self.job.lock().unwrap_or_else(|e| e.into_inner());
        if job.is_some() {
            bail!("Sussurro is already reading a document aloud — wait for it or cancel it.");
        }
        *job = Some(status);
        self.cancel.store(false, Ordering::Relaxed);
        Ok(JobGuard(self))
    }

    /// The job in progress, if any.
    pub fn current(&self) -> Option<JobStatus> {
        self.job.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    pub fn is_running(&self) -> bool {
        self.current().is_some()
    }

    /// Stop the job in progress (nothing is kept). False when none runs.
    pub fn cancel(&self) -> bool {
        let running = self.is_running();
        if running {
            self.cancel.store(true, Ordering::Relaxed);
        }
        running
    }
}

struct JobGuard<'a>(&'a Jobs);

impl JobGuard<'_> {
    fn update(&self, done: usize, total: usize) -> Option<JobStatus> {
        let mut job = self.0.job.lock().unwrap_or_else(|e| e.into_inner());
        let j = job.as_mut()?;
        j.done = done;
        j.total = total;
        Some(j.clone())
    }
}

impl Drop for JobGuard<'_> {
    fn drop(&mut self) {
        *self.0.job.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }
}

/// Speak `chunks` into a new Ogg Opus file at `out` (24 kHz mono,
/// [`SPEECH`], with `marker`'s comments; every block through its hook, so
/// the file is watermarked — `marker` must be made for [`SPEECH_RATE`]).
/// Returns the samples written, at [`SPEECH_RATE`]. On any error, `out` is
/// removed.
pub fn render_to_opus(
    engine: &mut dyn TtsEngine,
    chunks: &[Chunk],
    out: &Path,
    marker: &mut Marker,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(usize, usize),
) -> Result<u64> {
    let mut w = OpusWriter::create_with(out, MAX_SAMPLES, SPEECH, &marker.tags())?;
    // Pocket speaks at 24 kHz: a pass-through.
    let mut rs = Resampler::new(engine.sample_rate(), SPEECH_RATE);
    let spoken = (|| -> Result<()> {
        engine::render(engine, chunks, cancel, progress, &mut |pcm| {
            let block = rs.push(pcm);
            w.write(&marker.process(&block)?)?;
            Ok(())
        })?;
        let tail = rs.flush();
        w.write(&marker.process(&tail)?)?;
        w.write(&marker.finish()?)?;
        Ok(())
    })();
    let result = spoken.and_then(|()| {
        let n = w.samples();
        w.finish()?;
        Ok(n)
    });
    if result.is_err() {
        let _ = std::fs::remove_file(out);
    }
    result
}

/// Sign the finished Ogg file `audio` and write its manifest to `sidecar`.
/// `Err` = unsigned, with the reason (nothing written).
fn sign_to(
    speaker: &dyn Speaker,
    marker: &Marker,
    audio: &Path,
    sidecar: &Path,
) -> Result<(), String> {
    let identity = speaker.signer()?;
    let manifest =
        signing::sign_sidecar(&identity, marker.provenance(), &signing::now_utc(), audio)
            .map_err(|e| format!("{e:#}"))?;
    std::fs::write(sidecar, manifest).map_err(|e| {
        let _ = std::fs::remove_file(sidecar);
        format!("writing the signature: {e}")
    })
}

/// One read-aloud request: always saved next to the item (`speech*.opus`,
/// #327 — the temporary *Listen* path of #256 was removed).
pub struct Request<'a> {
    pub archive: &'a Path,
    pub id: &'a str,
    /// `None` = the transcript.
    pub document: Option<&'a str>,
    /// `None` = the item's language.
    pub language: Option<&'a str>,
    /// The voice picked per language (`Settings.tts_voices`).
    pub voices: &'a BTreeMap<String, String>,
}

/// What a finished job made.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Outcome {
    /// The file name in the item folder.
    pub file: String,
    /// Length of the speech.
    pub seconds: f64,
    pub chunks: usize,
}

/// Read a document aloud (see the module docs). Blocking; `progress` gets
/// the job's status as chunks are spoken.
pub fn run(
    jobs: &Jobs,
    speaker: &dyn Speaker,
    req: &Request,
    progress: &mut dyn FnMut(&JobStatus),
) -> Result<Outcome> {
    let source = read_source(req.archive, req.id, req.document)?;
    if source.recording {
        bail!(
            "'{}' is still being recorded — read it aloud when the session ends",
            req.id
        );
    }
    let lang = resolve_language(req.language, &source.item_language)?;
    // #259 (P17 stretch goal): the *Podcast script* recipe's companion
    // document is the one case read aloud speaks with two voices — every
    // other document keeps the ordinary single-narrator path below.
    if source.document == super::podcast::SCRIPT_FILE {
        return run_podcast(jobs, speaker, req, lang, &source, progress);
    }
    let voice = service::voice_for(req.voices, lang);
    let (chunks, text_sha256) = prepared(&source.markdown, lang.code);
    if chunks.is_empty() {
        bail!("There is nothing to read in this document.");
    }
    let file = speech::speech_file_name(&source.document)?;
    // #257: no watermark, no speech — checked before any work.
    let watermark = speaker.watermark()?;
    let guard = jobs.begin(JobStatus {
        item_id: req.id.to_string(),
        document: source.document.clone(),
        language: lang.code.to_string(),
        voice: voice.id.to_string(),
        done: 0,
        total: chunks.len(),
    })?;
    let dir = existing_item_dir(req.archive, req.id)?;
    let out = speech::part_path(&dir, &file);
    let sidecar_out = speech::part_path(
        &dir,
        &speech::sidecar_name(&file).context("speech file name")?,
    );
    if let Some(parent) = out.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    let _ = std::fs::remove_file(&out);
    let _ = std::fs::remove_file(&sidecar_out);
    let mut marker = Marker::new(
        Provenance {
            engine: ENGINE_NAME.into(),
            voice: voice.id.into(),
            language: lang.code.into(),
            voice_b: None,
        },
        watermark,
        SPEECH_RATE,
    );
    let mut samples = 0;
    speaker.speak_with(lang, voice, &mut |e| {
        samples = render_to_opus(e, &chunks, &out, &mut marker, &jobs.cancel, &mut |i, n| {
            if let Some(s) = guard.update(i, n) {
                progress(&s);
            }
        })?;
        Ok(())
    })?;
    if let Some(s) = guard.update(chunks.len(), chunks.len()) {
        progress(&s);
    }
    // #257 part 2: the signed manifest, made from the finished file. The
    // key is made on this first need; no store = unsigned, never an error.
    let signed = sign_to(speaker, &marker, &out, &sidecar_out);
    if let Err(why) = &signed {
        eprintln!("read aloud: {file} is not signed: {why}");
    }
    let info = SpeechInfo {
        document: source.document.clone(),
        generator: marking::generator(),
        engine: ENGINE_NAME.into(),
        voice: voice.id.into(),
        voice_b: None,
        language: lang.code.into(),
        date: chrono::Local::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, false),
        text_sha256,
        marked: marker.marks(signed.is_ok()),
        unsigned: signed.as_ref().err().cloned().unwrap_or_default(),
        extra: BTreeMap::new(),
    };
    let sidecar = signed.is_ok().then_some(sidecar_out.as_path());
    speech::commit(req.archive, req.id, &out, sidecar, &file, &info)?;
    drop(guard);
    Ok(Outcome {
        file,
        seconds: samples as f64 / f64::from(SPEECH_RATE),
        chunks: chunks.len(),
    })
}

/// The two-voice *Podcast script* path (#259, P17): `source.markdown` is
/// parsed into dialogue turns ([`super::podcast::parse_script`]), spoken
/// with two distinct built-in voices for `lang` — Host A with the
/// document's ordinary read-aloud voice ([`service::voice_for`]), Host B
/// with [`super::podcast::second_voice`] — into one continuous marked Ogg
/// Opus file. Never cloning: both are catalogue voices, never a voice built
/// from audio. Everything else (marking, signing, staleness, saving) is the
/// same as [`run`].
fn run_podcast(
    jobs: &Jobs,
    speaker: &dyn Speaker,
    req: &Request,
    lang: &'static Language,
    source: &Source,
    progress: &mut dyn FnMut(&JobStatus),
) -> Result<Outcome> {
    use super::podcast::{self, Host};

    let voice_a = service::voice_for(req.voices, lang);
    let voice_b = podcast::second_voice(lang, voice_a);
    let turns = podcast::parse_script(&source.markdown);
    if turns.is_empty() {
        bail!(
            "This podcast script has no “Host A:” / “Host B:” lines to read — run the Podcast \
             script recipe first."
        );
    }
    let groups = podcast::chunk_script(&turns, Lang::from_code(lang.code));
    let total: usize = groups.iter().map(|(_, c)| c.len()).sum();
    if total == 0 {
        bail!("There is nothing to read in this podcast script.");
    }
    let text_sha256 = podcast::script_hash(&turns);
    let file = speech::speech_file_name(&source.document)?;
    // #257: no watermark, no speech — checked before any work.
    let watermark = speaker.watermark()?;
    let guard = jobs.begin(JobStatus {
        item_id: req.id.to_string(),
        document: source.document.clone(),
        language: lang.code.to_string(),
        voice: format!("{}+{}", voice_a.id, voice_b.id),
        done: 0,
        total,
    })?;
    let dir = existing_item_dir(req.archive, req.id)?;
    let out = speech::part_path(&dir, &file);
    let sidecar_out = speech::part_path(
        &dir,
        &speech::sidecar_name(&file).context("speech file name")?,
    );
    if let Some(parent) = out.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    let _ = std::fs::remove_file(&out);
    let _ = std::fs::remove_file(&sidecar_out);
    let mut marker = Marker::new(
        Provenance {
            engine: ENGINE_NAME.into(),
            voice: voice_a.id.into(),
            voice_b: Some(voice_b.id.into()),
            language: lang.code.into(),
        },
        watermark,
        SPEECH_RATE,
    );
    let render: Result<u64> = (|| {
        let mut w = OpusWriter::create_with(&out, MAX_SAMPLES, SPEECH, &marker.tags())?;
        let mut done = 0usize;
        for (host, group) in &groups {
            let voice = match host {
                Host::A => voice_a,
                Host::B => voice_b,
            };
            speaker.speak_with(lang, voice, &mut |e| {
                let mut rs = Resampler::new(e.sample_rate(), SPEECH_RATE);
                engine::render(
                    e,
                    group,
                    &jobs.cancel,
                    &mut |i, _n| {
                        if let Some(s) = guard.update(done + i, total) {
                            progress(&s);
                        }
                    },
                    &mut |pcm| {
                        let block = rs.push(pcm);
                        w.write(&marker.process(&block)?)?;
                        Ok(())
                    },
                )?;
                let tail = rs.flush();
                w.write(&marker.process(&tail)?)?;
                Ok(())
            })?;
            done += group.len();
            if let Some(s) = guard.update(done, total) {
                progress(&s);
            }
        }
        w.write(&marker.finish()?)?;
        let n = w.samples();
        w.finish()?;
        Ok(n)
    })();
    let samples = match render {
        Ok(n) => n,
        Err(e) => {
            let _ = std::fs::remove_file(&out);
            return Err(e);
        }
    };
    let signed = sign_to(speaker, &marker, &out, &sidecar_out);
    if let Err(why) = &signed {
        eprintln!("read aloud: {file} is not signed: {why}");
    }
    let info = SpeechInfo {
        document: source.document.clone(),
        generator: marking::generator(),
        engine: ENGINE_NAME.into(),
        voice: voice_a.id.into(),
        voice_b: Some(voice_b.id.into()),
        language: lang.code.into(),
        date: chrono::Local::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, false),
        text_sha256,
        marked: marker.marks(signed.is_ok()),
        unsigned: signed.as_ref().err().cloned().unwrap_or_default(),
        extra: BTreeMap::new(),
    };
    let sidecar = signed.is_ok().then_some(sidecar_out.as_path());
    speech::commit(req.archive, req.id, &out, sidecar, &file, &info)?;
    drop(guard);
    Ok(Outcome {
        file,
        seconds: samples as f64 / f64::from(SPEECH_RATE),
        chunks: total,
    })
}

// ---- what the Audio tab lists ------------------------------------------------

/// A speech file of an item, as the UI lists it.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct SpeechStatus {
    pub file: String,
    pub bytes: u64,
    /// What was read (`transcript.md`, a companion document); empty when
    /// the frontmatter has no record of the file.
    pub document: String,
    pub voice: String,
    /// A second voice (#259): this file is a two-voice podcast, Host A read
    /// by `voice`, Host B by `voice_b`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub voice_b: Option<String>,
    pub language: String,
    pub engine: String,
    pub date: String,
    pub marked: Vec<String>,
    /// Its signed-manifest sidecar (`.c2pa`) is next to it.
    pub signed: bool,
    /// Why it was left unsigned (from the frontmatter), else empty.
    pub unsigned: String,
    /// The frontmatter records the file.
    pub recorded: bool,
    /// The document's text changed since the speech was made.
    pub stale: bool,
    /// The document it was read from is gone.
    pub source_missing: bool,
}

/// The speech files of item `id`, with whether each is out of date.
pub fn statuses(archive: &Path, id: &str) -> Result<Vec<SpeechStatus>> {
    let dir = existing_item_dir(archive, id)?;
    let item = crate::archive::read_item(archive, id)?;
    let infos = speech::infos(&item.meta);
    let mut out = Vec::new();
    for f in speech::files_in(&dir) {
        let mut s = SpeechStatus {
            file: f.name.clone(),
            bytes: f.bytes,
            document: String::new(),
            voice: String::new(),
            voice_b: None,
            language: String::new(),
            engine: String::new(),
            date: String::new(),
            marked: Vec::new(),
            signed: speech::has_sidecar(&dir, &f.name),
            unsigned: String::new(),
            recorded: false,
            stale: false,
            source_missing: false,
        };
        if let Some(info) = infos.get(&f.name) {
            s.recorded = true;
            s.document = info.document.clone();
            s.voice = info.voice.clone();
            s.voice_b = info.voice_b.clone();
            s.language = info.language.clone();
            s.engine = info.engine.clone();
            s.date = info.date.clone();
            s.marked = info.marked.clone();
            s.unsigned = info.unsigned.clone();
            let text = if info.document == TRANSCRIPT_FILE {
                Ok(item.body.clone())
            } else {
                crate::archive::companion::read_companion(archive, id, &info.document)
                    .map(|d| d.body)
            };
            match text {
                Ok(md) => {
                    s.stale = if info.document == super::podcast::SCRIPT_FILE {
                        super::podcast::script_hash(&super::podcast::parse_script(&md))
                            != info.text_sha256
                    } else {
                        prepared(&md, &info.language).1 != info.text_sha256
                    }
                }
                Err(_) => s.source_missing = true,
            }
        }
        out.push(s);
    }
    Ok(out)
}

/// Delete a speech file (to the OS trash); refused while a job saves it.
pub fn delete(jobs: &Jobs, archive: &Path, id: &str, file: &str) -> Result<()> {
    if let Some(j) = jobs.current() {
        if j.item_id == id && speech::speech_file_name(&j.document).ok().as_deref() == Some(file) {
            bail!("this speech is being made right now — cancel it first");
        }
    }
    speech::delete(archive, id, file)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::archive::companion::{write_companion, CompanionMeta};
    use crate::archive::store::test_trash;
    use crate::archive::{create_item, ItemMeta, SegmentsFile};
    use crate::tts::engine::tests::FakeEngine;

    /// Speaks through [`FakeEngine`] (1 kHz, 10 samples per character);
    /// `fail` makes the engine unavailable.
    struct FakeSpeaker {
        fail: bool,
        /// The watermark model is missing.
        no_watermark: bool,
        spoken: Mutex<Vec<String>>,
        /// The voice id of every `speak_with` call, in order (#259: proves
        /// the podcast path alternates two distinct voices).
        voices_used: Mutex<Vec<String>>,
        /// Cancel this job before speaking.
        cancel_first: Option<&'static Jobs>,
        /// No working credential store: nothing can be signed.
        no_store: bool,
        /// Where the signing chain goes.
        sign_dir: tempfile::TempDir,
    }

    impl FakeSpeaker {
        fn new() -> Self {
            Self {
                fail: false,
                no_watermark: false,
                spoken: Mutex::new(Vec::new()),
                voices_used: Mutex::new(Vec::new()),
                cancel_first: None,
                no_store: false,
                sign_dir: tempfile::tempdir().unwrap(),
            }
        }
    }

    impl Speaker for FakeSpeaker {
        fn watermark(&self) -> Result<Box<dyn WatermarkModel>> {
            if self.no_watermark {
                bail!("The watermark model that marks generated speech is not downloaded");
            }
            Ok(crate::tts::watermark::tests::FakeWatermark::boxed())
        }

        fn signer(&self) -> Result<Arc<Identity>, String> {
            let store = crate::secrets::tests::FakeStore::default();
            store.broken.set(self.no_store);
            signing::load_or_create(self.sign_dir.path(), &store).map(Arc::new)
        }

        fn speak_with(
            &self,
            lang: &'static Language,
            voice: &'static Voice,
            f: &mut dyn FnMut(&mut dyn TtsEngine) -> Result<()>,
        ) -> Result<()> {
            if self.fail {
                bail!("the {} read-aloud model is not downloaded", lang.label);
            }
            if let Some(j) = self.cancel_first {
                j.cancel();
            }
            self.voices_used.lock().unwrap().push(voice.id.to_string());
            let mut e = FakeEngine { spoken: Vec::new() };
            let r = f(&mut e);
            self.spoken.lock().unwrap().extend(e.spoken);
            r
        }
    }

    fn item(archive: &Path, language: &str, body: &str) -> String {
        let meta = ItemMeta {
            title: "Riunione".into(),
            date: "2026-09-26T10:00:00+02:00".into(),
            language: language.into(),
            ..ItemMeta::default()
        };
        let id = create_item(archive, &meta, &SegmentsFile::default()).unwrap();
        // An item edited outside: its body is what the user wrote.
        let dir = existing_item_dir(archive, &id).unwrap();
        std::fs::write(
            dir.join(TRANSCRIPT_FILE),
            format!("---\ntype: note\ntitle: Riunione\nlanguage: {language}\n---\n{body}"),
        )
        .unwrap();
        id
    }

    fn req<'a>(
        archive: &'a Path,
        id: &'a str,
        voices: &'a BTreeMap<String, String>,
    ) -> Request<'a> {
        Request {
            archive,
            id,
            document: None,
            language: None,
            voices,
        }
    }

    #[test]
    fn saving_writes_a_marked_opus_next_to_the_item_and_records_it() {
        let tmp = tempfile::tempdir().unwrap();
        let archive = tmp.path();
        let id = item(
            archive,
            "it",
            "# Riunione\n\nAbbiamo 3 punti. Il primo è il budget.\n",
        );
        let jobs = Jobs::default();
        let voices = BTreeMap::from([("it".to_string(), "marius".to_string())]);
        let speaker = FakeSpeaker::new();
        let mut seen = Vec::new();
        let out = run(&jobs, &speaker, &req(archive, &id, &voices), &mut |s| {
            seen.push((s.done, s.total))
        })
        .unwrap();
        assert_eq!(out.file, "speech.opus");
        assert!(out.seconds > 0.0);
        assert!(!jobs.is_running(), "the job ends");
        assert_eq!(seen.last(), Some(&(out.chunks, out.chunks)));
        assert!(seen.first().unwrap().0 == 0);
        // The text went through #254's preparation (numbers in words).
        let spoken = speaker.spoken.lock().unwrap().join(" ");
        assert!(spoken.contains("tre punti"), "{spoken}");

        let dir = existing_item_dir(archive, &id).unwrap();
        let path = dir.join("speech.opus");
        let samples = crate::archive::opus::verify(&path).unwrap();
        assert_eq!(samples as f64 / 24_000.0, out.seconds, "24 kHz (#309)");
        let r = crate::archive::opus::OpusReader::open_native(&path).unwrap();
        assert_eq!((r.rate(), r.total_samples()), (24_000, samples));
        let tags = crate::archive::opus::read_tags(&path).unwrap();
        assert!(tags.contains(&("SYNTHETIC".into(), "1".into())), "{tags:?}");
        assert!(
            tags.contains(&("TTS_VOICE".into(), "marius".into())),
            "{tags:?}"
        );
        assert!(!speech::part_path(&dir, "speech.opus").exists());

        let item = crate::archive::read_item(archive, &id).unwrap();
        assert_eq!(item.speech.len(), 1);
        assert!(item.audio.is_empty());
        let info = &speech::infos(&item.meta)["speech.opus"];
        assert_eq!(info.document, "transcript.md");
        assert_eq!(
            (info.voice.as_str(), info.language.as_str()),
            ("marius", "it")
        );
        assert_eq!(
            info.marked,
            [
                marking::MARK_METADATA,
                marking::MARK_WATERMARK,
                marking::MARK_SIGNATURE
            ],
            "#257: every layer"
        );
        assert!(info.unsigned.is_empty());

        // The sidecar moved in with the audio and verifies against it.
        let sidecar = std::fs::read(dir.join("speech.c2pa")).unwrap();
        assert!(!speech::part_path(&dir, "speech.c2pa").exists());
        let layer = signing::verify_sidecar(&sidecar, &mut std::fs::File::open(&path).unwrap());
        assert_eq!(
            layer.status,
            signing::SignatureStatus::Valid,
            "{}",
            layer.problem
        );
        assert_eq!(layer.voice, "marius");
        assert!(layer.claims_sussurro && layer.ai_generated);

        let st = statuses(archive, &id).unwrap();
        assert_eq!(st.len(), 1);
        assert!(st[0].recorded && !st[0].stale && !st[0].source_missing);
        assert!(st[0].signed && st[0].unsigned.is_empty());
    }

    #[test]
    fn without_a_credential_store_the_speech_is_kept_unsigned_and_says_why() {
        let tmp = tempfile::tempdir().unwrap();
        let archive = tmp.path();
        let id = item(archive, "en", "Hello there.\n");
        let jobs = Jobs::default();
        let voices = BTreeMap::new();
        let speaker = FakeSpeaker {
            no_store: true,
            ..FakeSpeaker::new()
        };
        run(&jobs, &speaker, &req(archive, &id, &voices), &mut |_| {}).unwrap();
        let dir = existing_item_dir(archive, &id).unwrap();
        assert!(dir.join("speech.opus").is_file(), "the file is still made");
        assert!(!dir.join("speech.c2pa").exists());
        assert!(!speech::part_path(&dir, "speech.c2pa").exists());
        let item = crate::archive::read_item(archive, &id).unwrap();
        let info = &speech::infos(&item.meta)["speech.opus"];
        assert_eq!(
            info.marked,
            [marking::MARK_METADATA, marking::MARK_WATERMARK]
        );
        assert!(info.unsigned.contains("not available"), "{}", info.unsigned);
        let st = statuses(archive, &id).unwrap();
        assert!(!st[0].signed && !st[0].unsigned.is_empty());
        // Signed again later: the sidecar arrives.
        run(
            &jobs,
            &FakeSpeaker::new(),
            &req(archive, &id, &voices),
            &mut |_| {},
        )
        .unwrap();
        assert!(dir.join("speech.c2pa").is_file());
        assert!(statuses(archive, &id).unwrap()[0].unsigned.is_empty());
    }

    #[test]
    fn speech_goes_out_of_date_when_the_text_changes_not_the_frontmatter() {
        let tmp = tempfile::tempdir().unwrap();
        let archive = tmp.path();
        let id = item(archive, "en", "Hello there. This is a note.\n");
        let jobs = Jobs::default();
        let voices = BTreeMap::new();
        run(
            &jobs,
            &FakeSpeaker::new(),
            &req(archive, &id, &voices),
            &mut |_| {},
        )
        .unwrap();
        let mut meta = crate::archive::read_item(archive, &id).unwrap().meta;
        meta.tags = vec!["new tag".into()];
        crate::archive::update_meta(archive, &id, &meta).unwrap();
        assert!(
            !statuses(archive, &id).unwrap()[0].stale,
            "frontmatter edits don't count"
        );

        let dir = existing_item_dir(archive, &id).unwrap();
        let doc = std::fs::read_to_string(dir.join(TRANSCRIPT_FILE)).unwrap();
        std::fs::write(
            dir.join(TRANSCRIPT_FILE),
            doc.replace("a note", "a longer note"),
        )
        .unwrap();
        assert!(statuses(archive, &id).unwrap()[0].stale);
    }

    #[test]
    fn a_companion_document_gets_its_own_file_and_its_deletion_is_noticed() {
        let tmp = tempfile::tempdir().unwrap();
        let archive = tmp.path();
        let id = item(archive, "en", "The transcript.\n");
        let meta = CompanionMeta {
            title: "Action items".into(),
            ..CompanionMeta::default()
        };
        let doc = write_companion(
            archive,
            &id,
            "action-items.md",
            &meta,
            "- Call Anna at 3 pm.\n",
        )
        .unwrap();
        let jobs = Jobs::default();
        let voices = BTreeMap::new();
        let mut r = req(archive, &id, &voices);
        r.document = Some(&doc);
        let out = run(&jobs, &FakeSpeaker::new(), &r, &mut |_| {}).unwrap();
        assert_eq!(out.file, "speech-action-items.opus");
        let st = statuses(archive, &id).unwrap();
        assert_eq!(st[0].document, "action-items.md");
        let dir = existing_item_dir(archive, &id).unwrap();
        std::fs::remove_file(dir.join("action-items.md")).unwrap();
        assert!(statuses(archive, &id).unwrap()[0].source_missing);

        assert!(dir.join("speech-action-items.c2pa").is_file());
        delete(&jobs, archive, &id, "speech-action-items.opus").unwrap();
        assert!(test_trash::contains(&dir.join("speech-action-items.opus")));
        assert!(test_trash::contains(&dir.join("speech-action-items.c2pa")));
        assert!(statuses(archive, &id).unwrap().is_empty());
    }

    // ---- #259 (P17 stretch goal): the two-voice podcast script ----

    #[test]
    fn a_podcast_script_is_read_with_two_distinct_voices_into_one_file() {
        let tmp = tempfile::tempdir().unwrap();
        let archive = tmp.path();
        let id = item(archive, "en", "The transcript.\n");
        let meta = CompanionMeta {
            title: "Podcast script".into(),
            ..CompanionMeta::default()
        };
        let script =
            "Host A: Welcome to the show.\nHost B: Great to be here.\nHost A: Let's wrap up.\n";
        write_companion(
            archive,
            &id,
            super::super::podcast::SCRIPT_FILE,
            &meta,
            script,
        )
        .unwrap();
        let jobs = Jobs::default();
        let voices = BTreeMap::from([("en".to_string(), "alba".to_string())]);
        let speaker = FakeSpeaker::new();
        let mut r = req(archive, &id, &voices);
        r.document = Some(super::super::podcast::SCRIPT_FILE);
        let out = run(&jobs, &speaker, &r, &mut |_| {}).unwrap();
        assert_eq!(out.file, "speech-podcast-script.opus");
        assert!(!jobs.is_running());

        // Two distinct voices were used, alternating A, B, A.
        let used = speaker.voices_used.lock().unwrap().clone();
        assert_eq!(used, ["alba", "marius", "alba"], "{used:?}");

        let dir = existing_item_dir(archive, &id).unwrap();
        let path = dir.join("speech-podcast-script.opus");
        let tags = crate::archive::opus::read_tags(&path).unwrap();
        assert!(
            tags.contains(&("TTS_VOICE".into(), "alba".into())),
            "{tags:?}"
        );
        assert!(
            tags.contains(&("TTS_VOICE_B".into(), "marius".into())),
            "{tags:?}"
        );

        let item = crate::archive::read_item(archive, &id).unwrap();
        let info = &speech::infos(&item.meta)["speech-podcast-script.opus"];
        assert_eq!(info.voice, "alba");
        assert_eq!(info.voice_b.as_deref(), Some("marius"));

        let st = statuses(archive, &id).unwrap();
        assert_eq!(st.len(), 1);
        assert_eq!(st[0].voice_b.as_deref(), Some("marius"));
        assert!(!st[0].stale);

        // Editing the script (not the frontmatter) makes it stale.
        let dir = existing_item_dir(archive, &id).unwrap();
        let path_md = dir.join(super::super::podcast::SCRIPT_FILE);
        let doc = std::fs::read_to_string(&path_md).unwrap();
        std::fs::write(&path_md, doc.replace("wrap up", "wrap up now")).unwrap();
        assert!(statuses(archive, &id).unwrap()[0].stale);
    }

    #[test]
    fn a_script_with_no_host_lines_is_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let archive = tmp.path();
        let id = item(archive, "en", "The transcript.\n");
        write_companion(
            archive,
            &id,
            super::super::podcast::SCRIPT_FILE,
            &CompanionMeta::default(),
            "Just a plain paragraph, no host tags.\n",
        )
        .unwrap();
        let jobs = Jobs::default();
        let voices = BTreeMap::new();
        let mut r = req(archive, &id, &voices);
        r.document = Some(super::super::podcast::SCRIPT_FILE);
        let err = run(&jobs, &FakeSpeaker::new(), &r, &mut |_| {}).unwrap_err();
        assert!(err.to_string().contains("Host A"), "{err}");
        assert!(!jobs.is_running());
    }

    #[test]
    fn cancel_and_failures_leave_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let archive = tmp.path();
        let id = item(archive, "it", "Una frase. Poi un'altra.\n");
        let dir = existing_item_dir(archive, &id).unwrap();
        static JOBS: OnceLock<Jobs> = OnceLock::new();
        let jobs = JOBS.get_or_init(Jobs::default);
        let voices = BTreeMap::new();
        let speaker = FakeSpeaker {
            cancel_first: Some(jobs),
            ..FakeSpeaker::new()
        };
        let err = run(jobs, &speaker, &req(archive, &id, &voices), &mut |_| {}).unwrap_err();
        assert!(format!("{err:#}").contains("cancelled"), "{err:#}");
        assert!(!jobs.is_running());
        assert!(!dir.join("speech.opus").exists());
        assert!(!speech::part_path(&dir, "speech.opus").exists());

        let broken = FakeSpeaker {
            fail: true,
            ..FakeSpeaker::new()
        };
        let err = run(jobs, &broken, &req(archive, &id, &voices), &mut |_| {}).unwrap_err();
        assert!(err.to_string().contains("not downloaded"), "{err}");
        assert!(!jobs.is_running());
        assert!(crate::archive::speech::files_in(&dir).is_empty());

        // #257: without the watermark model nothing is spoken.
        let unmarked = FakeSpeaker {
            no_watermark: true,
            ..FakeSpeaker::new()
        };
        let err = run(jobs, &unmarked, &req(archive, &id, &voices), &mut |_| {}).unwrap_err();
        assert!(err.to_string().contains("watermark"), "{err}");
        assert!(unmarked.spoken.lock().unwrap().is_empty());
        assert!(!jobs.is_running());
        assert!(crate::archive::speech::files_in(&dir).is_empty());
        assert!(!speech::has_keys(
            &crate::archive::read_item(archive, &id).unwrap().meta
        ));
    }

    #[test]
    fn requests_that_cannot_be_read_are_refused_before_any_work() {
        let tmp = tempfile::tempdir().unwrap();
        let archive = tmp.path();
        let jobs = Jobs::default();
        let voices = BTreeMap::new();
        let speaker = FakeSpeaker::new();
        // No language the module speaks.
        let id = item(archive, "fr", "Bonjour.\n");
        let err = run(&jobs, &speaker, &req(archive, &id, &voices), &mut |_| {}).unwrap_err();
        assert!(err.to_string().contains("'fr'"), "{err}");
        // …unless one is picked.
        let mut r = req(archive, &id, &voices);
        r.language = Some("en");
        assert!(run(&jobs, &speaker, &r, &mut |_| {}).is_ok());
        // Nothing to read.
        let empty = item(archive, "it", "```\ncode only\n```\n");
        let err = run(&jobs, &speaker, &req(archive, &empty, &voices), &mut |_| {}).unwrap_err();
        assert!(err.to_string().contains("nothing to read"), "{err}");
        // A live item.
        let live = crate::archive::live::begin_session(
            archive,
            &ItemMeta {
                title: "Live".into(),
                language: "it".into(),
                ..ItemMeta::default()
            },
        )
        .unwrap();
        let err = run(&jobs, &speaker, &req(archive, &live, &voices), &mut |_| {}).unwrap_err();
        assert!(err.to_string().contains("recorded"), "{err}");
        // A document that isn't one.
        let mut r = req(archive, &id, &voices);
        r.document = Some("../x.md");
        assert!(run(&jobs, &speaker, &r, &mut |_| {}).is_err());
        assert!(speaker.spoken.lock().unwrap().iter().all(|s| !s.is_empty()));
    }

    #[test]
    fn one_job_at_a_time() {
        let jobs = Jobs::default();
        let status = JobStatus {
            item_id: "x".into(),
            document: TRANSCRIPT_FILE.into(),
            language: "it".into(),
            voice: "giovanni".into(),
            done: 0,
            total: 3,
        };
        let g = jobs.begin(status.clone()).unwrap();
        assert!(jobs.begin(status.clone()).is_err());
        assert_eq!(g.update(1, 3).unwrap().done, 1);
        assert!(jobs.cancel());
        assert!(delete(&jobs, Path::new("/nonexistent"), "x", "speech.opus")
            .unwrap_err()
            .to_string()
            .contains("being made"));
        drop(g);
        assert!(!jobs.cancel(), "nothing to cancel");
        assert!(jobs.begin(status).is_ok());
    }

    #[test]
    fn language_resolution() {
        assert_eq!(resolve_language(None, "it").unwrap().code, "it");
        assert_eq!(resolve_language(Some("en-GB"), "it").unwrap().code, "en");
        assert_eq!(resolve_language(Some(" "), "en").unwrap().code, "en");
        let err = resolve_language(None, "auto").unwrap_err().to_string();
        assert!(
            err.contains("no language") && err.contains("Italian"),
            "{err}"
        );
        assert!(resolve_language(None, "").is_err());
    }

    #[test]
    fn the_text_hash_follows_what_is_read() {
        let (c1, h1) = prepared("# T\n\nUno due.\n", "it");
        let (_, h2) = prepared("---\ntags: [x]\n---\n# T\n\nUno   due.\n", "it");
        assert!(!c1.is_empty());
        assert_eq!(h1, h2, "frontmatter and spacing are not read");
        assert_ne!(h1, prepared("# T\n\nUno tre.\n", "it").1);
        assert_ne!(
            h1,
            prepared("# T\n\nUno due.\n", "en").1,
            "language changes the reading"
        );
    }
}
