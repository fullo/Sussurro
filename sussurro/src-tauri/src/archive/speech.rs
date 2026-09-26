//! Generated speech of an item (0.12, #256, P17/P21): what *Read aloud →
//! Save* leaves in the item folder, and how the frontmatter records it. The
//! generation itself is [`crate::tts::read_aloud`]; this module owns the
//! names, the frontmatter keys and the file operations under the archive
//! lock.
//!
//! **Names** — never confused with recorded audio (`audio(-<channel>)?.*`,
//! #141/#247):
//!
//! ```text
//! speech.opus                  the transcript read aloud
//! speech-<slug>.opus           a companion document (`action-items.md` →
//!                              `speech-action-items.opus`); a name that is
//!                              not already a clean slug gets 8 hex digits of
//!                              its SHA-256 (`Riunione 3.md` →
//!                              `speech-riunione-3-1a2b3c4d.opus`)
//! ```
//!
//! The pattern the app treats as generated speech is
//! `speech(-[a-z0-9-]+)?.(opus|wav)` ([`is_speech_file_name`], plan §4.5);
//! only `.opus` is written: Ogg Opus, mono, 24 kHz at 32 kb/s since #309
//! ([`super::opus::SPEECH`], Pocket's full band); files saved before are
//! 16 kHz at 24 kb/s, the format of recorded audio. The `sussurro-audio:`
//! scheme plays both at their own rate ([`super::opus::OpusReader::open_native`]).
//!
//! **Frontmatter** — two app-owned keys, kept in [`ItemMeta::extra`] like
//! `audio:` so a UI that round-trips only the fields it knows never drops
//! them, and ignored when the UI sends them back (`store::update_meta`):
//!
//! ```yaml
//! speech: [speech.opus]
//! synthetic:
//!   speech.opus:
//!     document: transcript.md
//!     generator: Sussurro 0.10.1
//!     engine: Pocket TTS
//!     voice: giovanni
//!     language: it
//!     date: 2026-09-26T10:00:00+02:00
//!     text_sha256: 9f…        # of the speakable text: "out of date" when it changes
//!     marked: [metadata]      # "watermark" joins with #257
//! ```
//!
//! `synthetic:` is one entry per file (the plan's single block, keyed by
//! file because an item can have several speech files). The file itself says
//! the same in its Ogg Opus comments (`SYNTHETIC=1`, …, see
//! [`crate::tts::marking`]), so a copy taken out of the folder stays marked.
//!
//! What the UI lists, plays and deletes is the files present that match the
//! pattern — never names from the frontmatter. Deleting moves the file to
//! the OS trash. *Delete audio* (#141) and *Compress audio* (#248) never
//! touch speech files.

use super::audio::{AudioFile, AudioFormat};
use super::companion::validate_companion_name;
use super::store::{
    existing_item_dir, lock_items, move_to_trash, read_segments, sha256_hex, transcript_path,
    META_DIR, TRANSCRIPT_FILE,
};
use super::types::{ItemMeta, SessionState};
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Frontmatter key listing the item's speech files (app-owned).
pub const SPEECH_KEY: &str = "speech";
/// Frontmatter key with what generated each speech file (app-owned).
pub const SYNTHETIC_KEY: &str = "synthetic";
/// The speech file of the transcript.
pub const SPEECH_FILE: &str = "speech.opus";
/// Longest slug kept from a companion document's name.
const MAX_SLUG: usize = 60;

/// `speech.<ext>` or `speech-<[a-z0-9-]+>.<ext>`, with `<ext>` `opus` or
/// `wav`: the only names the app treats as generated speech. Pure.
pub fn is_speech_file_name(name: &str) -> bool {
    let Some(format) = AudioFormat::of_name(name) else {
        return false;
    };
    let stem = &name[..name.len() - format.ext().len() - 1];
    stem == "speech"
        || stem.strip_prefix("speech-").is_some_and(|s| {
            !s.is_empty()
                && s.len() <= MAX_SLUG + 9
                && s.bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        })
}

/// What the `sussurro-audio:` scheme may serve from an item folder:
/// recorded audio (#141) or generated speech. Pure.
pub fn is_playable_file_name(name: &str) -> bool {
    super::audio::is_audio_file_name(name) || is_speech_file_name(name)
}

/// Lowercase ASCII letters and digits, anything else collapsed to one `-`,
/// no `-` at either end. Pure.
fn slugify(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.is_empty() && !out.ends_with('-') {
            out.push('-');
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    out
}

/// The speech file of `document` (`transcript.md` or a companion document's
/// name) — see the module docs. Pure, deterministic.
pub fn speech_file_name(document: &str) -> Result<String> {
    if document == TRANSCRIPT_FILE {
        return Ok(SPEECH_FILE.to_string());
    }
    validate_companion_name(document)?;
    let stem = &document[..document.len() - 3];
    let slug = slugify(stem);
    if !slug.is_empty() && slug.len() <= MAX_SLUG && slug == stem {
        return Ok(format!("speech-{slug}.opus"));
    }
    let hash = &sha256_hex(document.as_bytes())[..8];
    let mut short: String = slug.chars().take(MAX_SLUG).collect();
    while short.ends_with('-') {
        short.pop();
    }
    Ok(if short.is_empty() {
        format!("speech-{hash}.opus")
    } else {
        format!("speech-{short}-{hash}.opus")
    })
}

/// What generated one speech file, as the frontmatter records it under
/// `synthetic.<file>`. Unknown keys are kept.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct SpeechInfo {
    /// What was read: `transcript.md` or a companion document.
    #[serde(default)]
    pub document: String,
    /// `Sussurro <version>`.
    #[serde(default)]
    pub generator: String,
    #[serde(default)]
    pub engine: String,
    /// Voice id (`giovanni`).
    #[serde(default)]
    pub voice: String,
    /// Language code the text was prepared and read in.
    #[serde(default)]
    pub language: String,
    /// RFC 3339.
    #[serde(default)]
    pub date: String,
    /// SHA-256 of the speakable text that was read (the text changed since
    /// = the speech is out of date).
    #[serde(default)]
    pub text_sha256: String,
    /// Marks the file carries (P21): `metadata`, and `watermark` once #257
    /// applies it.
    #[serde(default)]
    pub marked: Vec<String>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

/// The speech files the frontmatter lists (well-formed names only).
pub fn listed(meta: &ItemMeta) -> Vec<String> {
    match meta.extra.get(SPEECH_KEY) {
        Some(serde_json::Value::Array(xs)) => xs
            .iter()
            .filter_map(|x| x.as_str())
            .filter(|n| is_speech_file_name(n))
            .map(str::to_string)
            .collect(),
        Some(serde_json::Value::String(s)) if is_speech_file_name(s) => vec![s.clone()],
        _ => Vec::new(),
    }
}

/// The `synthetic:` records by file; entries that don't parse are skipped.
pub fn infos(meta: &ItemMeta) -> BTreeMap<String, SpeechInfo> {
    let Some(serde_json::Value::Object(map)) = meta.extra.get(SYNTHETIC_KEY) else {
        return BTreeMap::new();
    };
    map.iter()
        .filter(|(k, _)| is_speech_file_name(k))
        .filter_map(|(k, v)| {
            serde_json::from_value::<SpeechInfo>(v.clone())
                .ok()
                .map(|i| (k.clone(), i))
        })
        .collect()
}

/// Record `file` (listed once) with `info`, keeping the other entries.
pub fn record(meta: &mut ItemMeta, file: &str, info: &SpeechInfo) {
    let mut files = listed(meta);
    if !files.iter().any(|f| f == file) {
        files.push(file.to_string());
        files.sort();
    }
    meta.extra.insert(
        SPEECH_KEY.to_string(),
        serde_json::Value::Array(files.into_iter().map(Into::into).collect()),
    );
    let mut map = match meta.extra.remove(SYNTHETIC_KEY) {
        Some(serde_json::Value::Object(m)) => m,
        _ => serde_json::Map::new(),
    };
    map.insert(
        file.to_string(),
        serde_json::to_value(info).unwrap_or(serde_json::Value::Null),
    );
    meta.extra
        .insert(SYNTHETIC_KEY.to_string(), serde_json::Value::Object(map));
}

/// Drop `file` from both keys; a key left empty is removed.
pub fn forget(meta: &mut ItemMeta, file: &str) {
    let files: Vec<String> = listed(meta).into_iter().filter(|f| f != file).collect();
    if files.is_empty() {
        meta.extra.remove(SPEECH_KEY);
    } else {
        meta.extra.insert(
            SPEECH_KEY.to_string(),
            serde_json::Value::Array(files.into_iter().map(Into::into).collect()),
        );
    }
    if let Some(serde_json::Value::Object(map)) = meta.extra.get_mut(SYNTHETIC_KEY) {
        map.remove(file);
        if map.is_empty() {
            meta.extra.remove(SYNTHETIC_KEY);
        }
    }
}

/// Whether `meta` carries either app-owned key.
pub fn has_keys(meta: &ItemMeta) -> bool {
    meta.extra.contains_key(SPEECH_KEY) || meta.extra.contains_key(SYNTHETIC_KEY)
}

/// The speech files in the item folder `dir` (regular files only), by name.
pub fn files_in(dir: &Path) -> Vec<AudioFile> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<AudioFile> = entries
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_str()?.to_string();
            let meta = std::fs::symlink_metadata(e.path()).ok()?;
            (is_speech_file_name(&name) && meta.file_type().is_file()).then_some(AudioFile {
                name,
                bytes: meta.len(),
            })
        })
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// Where a speech file is written while it is generated: the item's
/// `.sussurro/` folder, renamed into place by [`commit`].
pub fn part_path(dir: &Path, file: &str) -> PathBuf {
    dir.join(META_DIR).join(format!("{file}.part"))
}

/// The item's frontmatter, refusing a live item and one the app can't
/// rewrite (broken YAML): checked before any file is touched.
fn check_item(dir: &Path, id: &str) -> Result<()> {
    let path = transcript_path(dir);
    let text =
        std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    let (meta, _) = super::frontmatter::parse(&text).map_err(|e| {
        anyhow::anyhow!(
            "the frontmatter of {} can't be read ({e:#}), so Sussurro can't record the speech \
             file there. Fix the YAML between the two `---` lines, then try again.",
            path.display()
        )
    })?;
    if meta.session_state() == Some(SessionState::Recording) {
        bail!("'{id}' is still being recorded — wait for the session to end");
    }
    Ok(())
}

fn rewrite(dir: &Path, update: &dyn Fn(ItemMeta) -> ItemMeta) -> Result<()> {
    let segments = read_segments(dir)?;
    super::live::rewrite_meta(dir, &segments, None, update, &|_| {})?;
    Ok(())
}

/// Move the finished `part` into item `id` as `file` and record `info` in
/// the frontmatter, under the archive lock. An earlier file of that name
/// goes to the OS trash first. On failure `part` is removed.
pub fn commit(archive: &Path, id: &str, part: &Path, file: &str, info: &SpeechInfo) -> Result<()> {
    let done = (|| -> Result<()> {
        if !is_speech_file_name(file) {
            bail!("'{file}' is not a speech file name");
        }
        let _lock = lock_items();
        let dir = existing_item_dir(archive, id)?;
        check_item(&dir, id)?;
        let target = dir.join(file);
        if std::fs::symlink_metadata(&target).is_ok() {
            move_to_trash(&target)
                .with_context(|| format!("moving the old {file} to the trash"))?;
        }
        std::fs::rename(part, &target).with_context(|| format!("saving {file}"))?;
        let info = info.clone();
        let file = file.to_string();
        if let Err(e) = rewrite(&dir, &move |mut m| {
            record(&mut m, &file, &info);
            m
        }) {
            // The file is marked by its own tags, but without its record the
            // app can't say what it was read from: take it back out.
            let _ = std::fs::remove_file(&target);
            return Err(e.context("recording the speech file in the frontmatter"));
        }
        Ok(())
    })();
    if done.is_err() {
        let _ = std::fs::remove_file(part);
    }
    done
}

/// *Delete speech*: `file` goes to the OS trash and the frontmatter stops
/// listing it. A file already gone is just forgotten.
pub fn delete(archive: &Path, id: &str, file: &str) -> Result<()> {
    if !is_speech_file_name(file) {
        bail!("'{file}' is not a speech file");
    }
    let _lock = lock_items();
    let dir = existing_item_dir(archive, id)?;
    let path = dir.join(file);
    match std::fs::symlink_metadata(&path) {
        Ok(m) if m.file_type().is_file() => move_to_trash(&path)?,
        Ok(_) => bail!("'{file}' in '{id}' is not a regular file"),
        Err(_) => {}
    }
    let text = std::fs::read_to_string(transcript_path(&dir)).unwrap_or_default();
    let recorded = super::frontmatter::parse(&text)
        .map(|(m, _)| has_keys(&m))
        .unwrap_or(false);
    if recorded {
        let file = file.to_string();
        if let Err(e) = rewrite(&dir, &move |mut m| {
            forget(&mut m, &file);
            m
        }) {
            eprintln!("archive: {id}: speech deleted, frontmatter not updated ({e:#})");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::archive::store::test_trash;
    use crate::archive::{create_item, read_item, update_meta, SegmentsFile};

    #[test]
    fn speech_names_never_look_like_recordings() {
        for ok in [
            "speech.opus",
            "speech.wav",
            "speech-action-items.opus",
            "speech-riunione-3-1a2b3c4d.opus",
            "speech-2.opus",
        ] {
            assert!(is_speech_file_name(ok), "{ok}");
            assert!(!super::super::audio::is_audio_file_name(ok), "{ok}");
            assert!(is_playable_file_name(ok), "{ok}");
        }
        for bad in [
            "speech",
            "speech.mp3",
            "speech-.opus",
            "speech-A.opus",
            "speech_x.opus",
            "speech-x/y.opus",
            "speech-é.opus",
            "audio-speech.opus",
            "Speech.opus",
            "speech.opus.part",
        ] {
            assert!(!is_speech_file_name(bad), "{bad}");
        }
        assert!(is_playable_file_name("audio-mic.opus"));
        assert!(!is_playable_file_name("transcript.md"));
    }

    #[test]
    fn a_document_gets_a_stable_speech_file_name() {
        assert_eq!(speech_file_name("transcript.md").unwrap(), "speech.opus");
        assert_eq!(
            speech_file_name("document.md").unwrap(),
            "speech-document.opus"
        );
        assert_eq!(
            speech_file_name("action-items-2.md").unwrap(),
            "speech-action-items-2.opus"
        );
        let a = speech_file_name("Riunione 3.md").unwrap();
        assert!(
            a.starts_with("speech-riunione-3-") && a.ends_with(".opus"),
            "{a}"
        );
        assert_eq!(
            a,
            speech_file_name("Riunione 3.md").unwrap(),
            "deterministic"
        );
        // Names that slug alike still get different files.
        assert_ne!(a, speech_file_name("riunione_3.md").unwrap());
        assert_ne!(a, speech_file_name("riunione-3.md").unwrap());
        let only_symbols = speech_file_name("日本語.md").unwrap();
        assert_eq!(
            only_symbols.len(),
            "speech-12345678.opus".len(),
            "{only_symbols}"
        );
        let long = speech_file_name(&format!("{}.md", "a".repeat(200))).unwrap();
        assert!(is_speech_file_name(&long), "{long}");
        for name in ["Riunione 3.md", "日本語.md", "x.md", "Ünïcödé — notes.md"] {
            assert!(
                is_speech_file_name(&speech_file_name(name).unwrap()),
                "{name}"
            );
        }
        assert!(speech_file_name("../x.md").is_err());
        assert!(speech_file_name("notes.txt").is_err());
        assert!(speech_file_name(".hidden.md").is_err());
    }

    fn info(doc: &str, hash: &str) -> SpeechInfo {
        SpeechInfo {
            document: doc.into(),
            generator: "Sussurro test".into(),
            engine: "Pocket TTS".into(),
            voice: "giovanni".into(),
            language: "it".into(),
            date: "2026-09-26T10:00:00+02:00".into(),
            text_sha256: hash.into(),
            marked: vec!["metadata".into()],
            extra: BTreeMap::new(),
        }
    }

    #[test]
    fn frontmatter_keys_round_trip_through_yaml() {
        let mut meta = ItemMeta {
            title: "T".into(),
            ..ItemMeta::default()
        };
        record(&mut meta, "speech.opus", &info("transcript.md", "aa"));
        record(
            &mut meta,
            "speech-document.opus",
            &info("document.md", "bb"),
        );
        record(&mut meta, "speech.opus", &info("transcript.md", "cc"));
        let doc = super::super::frontmatter::render(&meta).unwrap();
        assert!(doc.contains("synthetic:"), "{doc}");
        let (back, _) = super::super::frontmatter::parse(&format!("{doc}\nbody\n")).unwrap();
        assert_eq!(listed(&back), ["speech-document.opus", "speech.opus"]);
        let infos = infos(&back);
        assert_eq!(infos["speech.opus"].text_sha256, "cc");
        assert_eq!(infos["speech-document.opus"].document, "document.md");
        assert_eq!(infos["speech.opus"].marked, ["metadata"]);
        let mut m = back.clone();
        forget(&mut m, "speech.opus");
        assert_eq!(listed(&m), ["speech-document.opus"]);
        assert!(!self::infos(&m).contains_key("speech.opus"));
        forget(&mut m, "speech-document.opus");
        assert!(!has_keys(&m), "empty keys are removed");
    }

    #[test]
    fn hand_edited_keys_are_read_leniently() {
        let doc = "---\ntype: note\ntitle: T\nspeech: speech.opus\nsynthetic:\n  speech.opus:\n    voice: alba\n    future_key: 3\n  ../x.opus: {}\n  speech-b.opus: nonsense\n---\nbody\n";
        let (meta, _) = super::super::frontmatter::parse(doc).unwrap();
        assert_eq!(listed(&meta), ["speech.opus"]);
        let infos = infos(&meta);
        assert_eq!(infos.len(), 1);
        assert_eq!(infos["speech.opus"].voice, "alba");
        assert_eq!(infos["speech.opus"].extra["future_key"], 3);
        let old = "---\ntype: note\ntitle: T\n---\nbody\n";
        let (meta, _) = super::super::frontmatter::parse(old).unwrap();
        assert!(
            listed(&meta).is_empty() && self::infos(&meta).is_empty(),
            "items from before #256"
        );
    }

    fn new_item(archive: &Path) -> String {
        let meta = ItemMeta {
            title: "Read me".into(),
            date: "2026-09-26T10:00:00+02:00".into(),
            language: "it".into(),
            ..ItemMeta::default()
        };
        create_item(archive, &meta, &SegmentsFile::default()).unwrap()
    }

    fn part(archive: &Path, id: &str, file: &str, bytes: &[u8]) -> PathBuf {
        let dir = existing_item_dir(archive, id).unwrap();
        let p = part_path(&dir, file);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, bytes).unwrap();
        p
    }

    #[test]
    fn commit_moves_the_file_in_records_it_and_trashes_the_old_one() {
        let tmp = tempfile::tempdir().unwrap();
        let archive = tmp.path();
        let id = new_item(archive);
        let dir = existing_item_dir(archive, &id).unwrap();
        let p = part(archive, &id, SPEECH_FILE, b"one");
        commit(archive, &id, &p, SPEECH_FILE, &info("transcript.md", "h1")).unwrap();
        assert!(!p.exists());
        assert_eq!(std::fs::read(dir.join(SPEECH_FILE)).unwrap(), b"one");
        let item = read_item(archive, &id).unwrap();
        assert_eq!(listed(&item.meta), [SPEECH_FILE]);
        assert!(item.audio.is_empty(), "speech is not recorded audio");
        assert_eq!(files_in(&dir).len(), 1);

        let p = part(archive, &id, SPEECH_FILE, b"two");
        commit(archive, &id, &p, SPEECH_FILE, &info("transcript.md", "h2")).unwrap();
        assert_eq!(std::fs::read(dir.join(SPEECH_FILE)).unwrap(), b"two");
        assert!(
            test_trash::contains(&dir.join(SPEECH_FILE)),
            "old one trashed"
        );
        let item = read_item(archive, &id).unwrap();
        assert_eq!(infos(&item.meta)[SPEECH_FILE].text_sha256, "h2");

        // The UI sending the frontmatter back can't drop or forge the keys.
        let mut sent = item.meta.clone();
        sent.title = "Renamed".into();
        sent.extra.remove(SPEECH_KEY);
        sent.extra.insert(
            SYNTHETIC_KEY.into(),
            serde_json::json!({"speech.opus": {"voice": "x"}}),
        );
        let after = update_meta(archive, &id, &sent).unwrap();
        assert_eq!(after.meta.title, "Renamed");
        assert_eq!(listed(&after.meta), [SPEECH_FILE]);
        assert_eq!(infos(&after.meta)[SPEECH_FILE].voice, "giovanni");
    }

    #[test]
    fn commit_refuses_a_live_or_unreadable_item_and_removes_the_part() {
        let tmp = tempfile::tempdir().unwrap();
        let archive = tmp.path();
        let id = crate::archive::live::begin_session(
            archive,
            &ItemMeta {
                title: "Live".into(),
                ..ItemMeta::default()
            },
        )
        .unwrap();
        let p = part(archive, &id, SPEECH_FILE, b"x");
        let err = commit(archive, &id, &p, SPEECH_FILE, &info("transcript.md", "h")).unwrap_err();
        assert!(err.to_string().contains("recorded"), "{err}");
        assert!(!p.exists());

        let id = new_item(archive);
        let dir = existing_item_dir(archive, &id).unwrap();
        std::fs::write(transcript_path(&dir), "---\ntitle: [broken\n---\nbody\n").unwrap();
        let p = part(archive, &id, SPEECH_FILE, b"x");
        let err = commit(archive, &id, &p, SPEECH_FILE, &info("transcript.md", "h")).unwrap_err();
        assert!(format!("{err:#}").contains("frontmatter"), "{err:#}");
        assert!(!p.exists() && !dir.join(SPEECH_FILE).exists());

        let p = part(archive, &id, "audio.opus", b"x");
        assert!(commit(archive, &id, &p, "audio.opus", &info("transcript.md", "h")).is_err());
    }

    #[test]
    fn delete_trashes_the_file_and_forgets_it() {
        let tmp = tempfile::tempdir().unwrap();
        let archive = tmp.path();
        let id = new_item(archive);
        let dir = existing_item_dir(archive, &id).unwrap();
        for f in [SPEECH_FILE, "speech-document.opus"] {
            let p = part(archive, &id, f, b"x");
            commit(archive, &id, &p, f, &info("transcript.md", "h")).unwrap();
        }
        delete(archive, &id, SPEECH_FILE).unwrap();
        assert!(test_trash::contains(&dir.join(SPEECH_FILE)));
        let item = read_item(archive, &id).unwrap();
        assert_eq!(listed(&item.meta), ["speech-document.opus"]);
        // Gone already: forgotten without error.
        std::fs::remove_file(dir.join("speech-document.opus")).unwrap();
        delete(archive, &id, "speech-document.opus").unwrap();
        assert!(!has_keys(&read_item(archive, &id).unwrap().meta));
        // Never anything but a speech file.
        std::fs::write(dir.join("audio.opus"), b"rec").unwrap();
        assert!(delete(archive, &id, "audio.opus").is_err());
        assert!(delete(archive, &id, "../transcript.md").is_err());
        assert!(dir.join("audio.opus").exists());
    }
}
