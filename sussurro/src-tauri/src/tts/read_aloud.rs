//! Read aloud (0.12, #256, P17): an item's transcript or one of its
//! companion documents becomes speech — saved next to the item
//! (`speech.opus`, `speech-<document>.opus`, see [`crate::archive::speech`])
//! or written to a temporary *Listen* file in app data that is deleted when
//! the document closes, at the next *Listen*, at startup and when the module
//! is turned off.
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
//! [`super::text::prepare`] (#254), each chunk is spoken, resampled to
//! 16 kHz ([`super::resample`]), passed through the marking hook
//! ([`super::marking::Marker::process`], P21) and encoded to Ogg Opus with
//! the synthetic-speech comments. A saved file is written to the item's
//! `.sussurro/` and moved into place under the archive lock; a cancelled or
//! failed run leaves nothing.
//!
//! **Out of date**: the frontmatter records the SHA-256 of the speakable
//! text that was read ([`prepared`]); when the document's text changes
//! later, [`statuses`] reports the speech as `stale` and the UI says so.
//! Only what is read counts — a new tag or a frontmatter edit doesn't.

use super::catalog::{self, Language, Voice};
use super::engine::{self, TtsEngine};
use super::marking::{self, Marker, Provenance};
use super::resample::Resampler;
use super::service::{self, ENGINE_NAME, LISTEN_PREFIX, PREVIEW_PREFIX};
use super::text::{self, Chunk, Lang, PrepOptions};
use crate::archive::audio::{MAX_SAMPLES, RATE};
use crate::archive::opus::OpusWriter;
use crate::archive::speech::{self, SpeechInfo};
use crate::archive::store::{existing_item_dir, sha256_hex, TRANSCRIPT_FILE};
use anyhow::{bail, Context, Result};
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

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
}

impl Speaker for PocketSpeaker<'_> {
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

// ---- the job -----------------------------------------------------------------

/// The read-aloud job in progress, as the UI shows it.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct JobStatus {
    pub item_id: String,
    pub document: String,
    /// Saved next to the item (else a temporary *Listen* file).
    pub save: bool,
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

/// Speak `chunks` into a new Ogg Opus file at `out` (16 kHz mono, with
/// `marker`'s comments; every block through its hook). Returns the samples
/// written. On any error, `out` is removed.
pub fn render_to_opus(
    engine: &mut dyn TtsEngine,
    chunks: &[Chunk],
    out: &Path,
    marker: &mut Marker,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(usize, usize),
) -> Result<u64> {
    let mut w = OpusWriter::create_tagged(out, MAX_SAMPLES, &marker.tags())?;
    let mut rs = Resampler::new(engine.sample_rate(), RATE);
    let spoken = (|| -> Result<()> {
        engine::render(engine, chunks, cancel, progress, &mut |pcm| {
            let mut block = rs.push(pcm);
            marker.process(&mut block, RATE);
            w.write(&block)?;
            Ok(())
        })?;
        let mut tail = rs.flush();
        marker.process(&mut tail, RATE);
        w.write(&tail)?;
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

/// Where the result goes.
pub enum Target<'a> {
    /// Next to the item (`speech*.opus`).
    Save,
    /// A temporary file in `dir` (the app data's `tts-preview/`).
    Listen { dir: &'a Path },
}

/// One read-aloud request.
pub struct Request<'a> {
    pub archive: &'a Path,
    pub id: &'a str,
    /// `None` = the transcript.
    pub document: Option<&'a str>,
    /// `None` = the item's language.
    pub language: Option<&'a str>,
    /// The voice picked per language (`Settings.tts_voices`).
    pub voices: &'a BTreeMap<String, String>,
    pub target: Target<'a>,
}

/// What a finished job made.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Outcome {
    /// Saved: the file name in the item folder. Listen: the
    /// `sussurro-audio:` path (`tts-preview/listen-N.opus`).
    pub file: String,
    pub save: bool,
    /// Length of the speech.
    pub seconds: f64,
    pub chunks: usize,
}

static LISTEN_SEQ: AtomicU64 = AtomicU64::new(0);

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
        bail!("'{}' is still being recorded — read it aloud when the session ends", req.id);
    }
    let lang = resolve_language(req.language, &source.item_language)?;
    let voice = service::voice_for(req.voices, lang);
    let (chunks, text_sha256) = prepared(&source.markdown, lang.code);
    if chunks.is_empty() {
        bail!("There is nothing to read in this document.");
    }
    let file = speech::speech_file_name(&source.document)?;
    let save = matches!(req.target, Target::Save);
    let guard = jobs.begin(JobStatus {
        item_id: req.id.to_string(),
        document: source.document.clone(),
        save,
        language: lang.code.to_string(),
        voice: voice.id.to_string(),
        done: 0,
        total: chunks.len(),
    })?;
    let (out, listen) = match req.target {
        Target::Save => {
            let dir = existing_item_dir(req.archive, req.id)?;
            let part = speech::part_path(&dir, &file);
            if let Some(parent) = part.parent() {
                std::fs::create_dir_all(parent)
                    .with_context(|| format!("creating {}", parent.display()))?;
            }
            let _ = std::fs::remove_file(&part);
            (part, None)
        }
        Target::Listen { dir } => {
            std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
            service::clear_prefix(dir, LISTEN_PREFIX);
            let n = LISTEN_SEQ.fetch_add(1, Ordering::Relaxed) + 1;
            let name = format!("{LISTEN_PREFIX}{n}.opus");
            (dir.join(format!("{name}.part")), Some((dir.join(&name), name)))
        }
    };
    let mut marker = Marker::new(Provenance {
        engine: ENGINE_NAME.into(),
        voice: voice.id.into(),
        language: lang.code.into(),
    });
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
    let file = match listen {
        None => {
            let info = SpeechInfo {
                document: source.document.clone(),
                generator: marking::generator(),
                engine: ENGINE_NAME.into(),
                voice: voice.id.into(),
                language: lang.code.into(),
                date: chrono::Local::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, false),
                text_sha256,
                marked: marker.marks(),
                extra: BTreeMap::new(),
            };
            speech::commit(req.archive, req.id, &out, &file, &info)?;
            file
        }
        Some((path, name)) => {
            if let Err(e) = std::fs::rename(&out, &path) {
                let _ = std::fs::remove_file(&out);
                return Err(e).context("saving the temporary speech file");
            }
            format!("{PREVIEW_PREFIX}{name}")
        }
    };
    drop(guard);
    Ok(Outcome {
        file,
        save,
        seconds: samples as f64 / f64::from(RATE),
        chunks: chunks.len(),
    })
}

/// Delete the temporary *Listen* files (the document closed). Skipped while
/// a *Listen* job writes one.
pub fn discard_listens(jobs: &Jobs, dir: &Path) {
    if jobs.current().is_some_and(|j| !j.save) {
        return;
    }
    service::clear_prefix(dir, LISTEN_PREFIX);
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
    pub language: String,
    pub engine: String,
    pub date: String,
    pub marked: Vec<String>,
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
            language: String::new(),
            engine: String::new(),
            date: String::new(),
            marked: Vec::new(),
            recorded: false,
            stale: false,
            source_missing: false,
        };
        if let Some(info) = infos.get(&f.name) {
            s.recorded = true;
            s.document = info.document.clone();
            s.voice = info.voice.clone();
            s.language = info.language.clone();
            s.engine = info.engine.clone();
            s.date = info.date.clone();
            s.marked = info.marked.clone();
            let text = if info.document == TRANSCRIPT_FILE {
                Ok(item.body.clone())
            } else {
                crate::archive::companion::read_companion(archive, id, &info.document).map(|d| d.body)
            };
            match text {
                Ok(md) => s.stale = prepared(&md, &info.language).1 != info.text_sha256,
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
        if j.save && j.item_id == id && speech::speech_file_name(&j.document).ok().as_deref() == Some(file) {
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
        spoken: Mutex<Vec<String>>,
        /// Cancel this job before speaking.
        cancel_first: Option<&'static Jobs>,
    }

    impl FakeSpeaker {
        fn new() -> Self {
            Self {
                fail: false,
                spoken: Mutex::new(Vec::new()),
                cancel_first: None,
            }
        }
    }

    impl Speaker for FakeSpeaker {
        fn speak_with(
            &self,
            lang: &'static Language,
            _voice: &'static Voice,
            f: &mut dyn FnMut(&mut dyn TtsEngine) -> Result<()>,
        ) -> Result<()> {
            if self.fail {
                bail!("the {} read-aloud model is not downloaded", lang.label);
            }
            if let Some(j) = self.cancel_first {
                j.cancel();
            }
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

    fn req<'a>(archive: &'a Path, id: &'a str, voices: &'a BTreeMap<String, String>, target: Target<'a>) -> Request<'a> {
        Request {
            archive,
            id,
            document: None,
            language: None,
            voices,
            target,
        }
    }

    #[test]
    fn saving_writes_a_marked_opus_next_to_the_item_and_records_it() {
        let tmp = tempfile::tempdir().unwrap();
        let archive = tmp.path();
        let id = item(archive, "it", "# Riunione\n\nAbbiamo 3 punti. Il primo è il budget.\n");
        let jobs = Jobs::default();
        let voices = BTreeMap::from([("it".to_string(), "marius".to_string())]);
        let speaker = FakeSpeaker::new();
        let mut seen = Vec::new();
        let out = run(&jobs, &speaker, &req(archive, &id, &voices, Target::Save), &mut |s| {
            seen.push((s.done, s.total))
        })
        .unwrap();
        assert_eq!(out.file, "speech.opus");
        assert!(out.save && out.seconds > 0.0);
        assert!(!jobs.is_running(), "the job ends");
        assert_eq!(seen.last(), Some(&(out.chunks, out.chunks)));
        assert!(seen.first().unwrap().0 == 0);
        // The text went through #254's preparation (numbers in words).
        let spoken = speaker.spoken.lock().unwrap().join(" ");
        assert!(spoken.contains("tre punti"), "{spoken}");

        let dir = existing_item_dir(archive, &id).unwrap();
        let path = dir.join("speech.opus");
        let samples = crate::archive::opus::verify(&path).unwrap();
        assert_eq!(samples as f64 / 16_000.0, out.seconds);
        let tags = crate::archive::opus::read_tags(&path).unwrap();
        assert!(tags.contains(&("SYNTHETIC".into(), "1".into())), "{tags:?}");
        assert!(tags.contains(&("TTS_VOICE".into(), "marius".into())), "{tags:?}");
        assert!(!speech::part_path(&dir, "speech.opus").exists());

        let item = crate::archive::read_item(archive, &id).unwrap();
        assert_eq!(item.speech.len(), 1);
        assert!(item.audio.is_empty());
        let info = &speech::infos(&item.meta)["speech.opus"];
        assert_eq!(info.document, "transcript.md");
        assert_eq!((info.voice.as_str(), info.language.as_str()), ("marius", "it"));
        assert_eq!(info.marked, [marking::MARK_METADATA]);

        let st = statuses(archive, &id).unwrap();
        assert_eq!(st.len(), 1);
        assert!(st[0].recorded && !st[0].stale && !st[0].source_missing);
    }

    #[test]
    fn speech_goes_out_of_date_when_the_text_changes_not_the_frontmatter() {
        let tmp = tempfile::tempdir().unwrap();
        let archive = tmp.path();
        let id = item(archive, "en", "Hello there. This is a note.\n");
        let jobs = Jobs::default();
        let voices = BTreeMap::new();
        run(&jobs, &FakeSpeaker::new(), &req(archive, &id, &voices, Target::Save), &mut |_| {}).unwrap();
        let mut meta = crate::archive::read_item(archive, &id).unwrap().meta;
        meta.tags = vec!["new tag".into()];
        crate::archive::update_meta(archive, &id, &meta).unwrap();
        assert!(!statuses(archive, &id).unwrap()[0].stale, "frontmatter edits don't count");

        let dir = existing_item_dir(archive, &id).unwrap();
        let doc = std::fs::read_to_string(dir.join(TRANSCRIPT_FILE)).unwrap();
        std::fs::write(dir.join(TRANSCRIPT_FILE), doc.replace("a note", "a longer note")).unwrap();
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
        let doc = write_companion(archive, &id, "action-items.md", &meta, "- Call Anna at 3 pm.\n").unwrap();
        let jobs = Jobs::default();
        let voices = BTreeMap::new();
        let mut r = req(archive, &id, &voices, Target::Save);
        r.document = Some(&doc);
        let out = run(&jobs, &FakeSpeaker::new(), &r, &mut |_| {}).unwrap();
        assert_eq!(out.file, "speech-action-items.opus");
        let st = statuses(archive, &id).unwrap();
        assert_eq!(st[0].document, "action-items.md");
        let dir = existing_item_dir(archive, &id).unwrap();
        std::fs::remove_file(dir.join("action-items.md")).unwrap();
        assert!(statuses(archive, &id).unwrap()[0].source_missing);

        delete(&jobs, archive, &id, "speech-action-items.opus").unwrap();
        assert!(test_trash::contains(&dir.join("speech-action-items.opus")));
        assert!(statuses(archive, &id).unwrap().is_empty());
    }

    #[test]
    fn listening_writes_a_temporary_file_and_never_touches_the_item() {
        let tmp = tempfile::tempdir().unwrap();
        let archive = tmp.path().join("archive");
        let listen = tmp.path().join("tts-preview");
        let id = item(&archive, "it", "Ciao a tutti.\n");
        let before = std::fs::read(existing_item_dir(&archive, &id).unwrap().join(TRANSCRIPT_FILE)).unwrap();
        let jobs = Jobs::default();
        let voices = BTreeMap::new();
        let out = run(&jobs, &FakeSpeaker::new(), &req(&archive, &id, &voices, Target::Listen { dir: &listen }), &mut |_| {}).unwrap();
        assert!(!out.save);
        let name = service::preview_file_name(&out.file).expect("served by the scheme");
        assert!(listen.join(name).is_file());
        let item = crate::archive::read_item(&archive, &id).unwrap();
        assert!(item.speech.is_empty() && !speech::has_keys(&item.meta));
        assert_eq!(
            std::fs::read(existing_item_dir(&archive, &id).unwrap().join(TRANSCRIPT_FILE)).unwrap(),
            before
        );
        // The next Listen replaces it; closing the document discards it.
        let again = run(&jobs, &FakeSpeaker::new(), &req(&archive, &id, &voices, Target::Listen { dir: &listen }), &mut |_| {}).unwrap();
        assert!(!listen.join(name).exists());
        let name2 = service::preview_file_name(&again.file).unwrap();
        discard_listens(&jobs, &listen);
        assert!(!listen.join(name2).exists());
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
        let err = run(jobs, &speaker, &req(archive, &id, &voices, Target::Save), &mut |_| {}).unwrap_err();
        assert!(format!("{err:#}").contains("cancelled"), "{err:#}");
        assert!(!jobs.is_running());
        assert!(!dir.join("speech.opus").exists());
        assert!(!speech::part_path(&dir, "speech.opus").exists());

        let broken = FakeSpeaker {
            fail: true,
            ..FakeSpeaker::new()
        };
        let err = run(jobs, &broken, &req(archive, &id, &voices, Target::Save), &mut |_| {}).unwrap_err();
        assert!(err.to_string().contains("not downloaded"), "{err}");
        assert!(!jobs.is_running());
        assert!(crate::archive::speech::files_in(&dir).is_empty());
        assert!(!speech::has_keys(&crate::archive::read_item(archive, &id).unwrap().meta));
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
        let err = run(&jobs, &speaker, &req(archive, &id, &voices, Target::Save), &mut |_| {}).unwrap_err();
        assert!(err.to_string().contains("'fr'"), "{err}");
        // …unless one is picked.
        let mut r = req(archive, &id, &voices, Target::Save);
        r.language = Some("en");
        assert!(run(&jobs, &speaker, &r, &mut |_| {}).is_ok());
        // Nothing to read.
        let empty = item(archive, "it", "```\ncode only\n```\n");
        let err = run(&jobs, &speaker, &req(archive, &empty, &voices, Target::Save), &mut |_| {}).unwrap_err();
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
        let err = run(&jobs, &speaker, &req(archive, &live, &voices, Target::Save), &mut |_| {}).unwrap_err();
        assert!(err.to_string().contains("recorded"), "{err}");
        // A document that isn't one.
        let mut r = req(archive, &id, &voices, Target::Save);
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
            save: true,
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
        assert!(err.contains("no language") && err.contains("Italian"), "{err}");
        assert!(resolve_language(None, "").is_err());
    }

    #[test]
    fn the_text_hash_follows_what_is_read() {
        let (c1, h1) = prepared("# T\n\nUno due.\n", "it");
        let (_, h2) = prepared("---\ntags: [x]\n---\n# T\n\nUno   due.\n", "it");
        assert!(!c1.is_empty());
        assert_eq!(h1, h2, "frontmatter and spacing are not read");
        assert_ne!(h1, prepared("# T\n\nUno tre.\n", "it").1);
        assert_ne!(h1, prepared("# T\n\nUno due.\n", "en").1, "language changes the reading");
    }
}
