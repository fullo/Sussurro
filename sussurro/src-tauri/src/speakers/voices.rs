//! Voice profile storage (0.11, #241; plan P13): one file per person with
//! *Recognise this voice* on, in the app data dir — never in the archive.
//!
//! ```text
//! <app data>/voices/            (0700 on Unix)
//!   <person id>.json            (0600 on Unix, written atomically)
//! ```
//!
//! The archive is often synced (iCloud Drive, OneDrive) and is meant to be
//! moved and shared, while a voiceprint linked to a person is biometric
//! data (GDPR art. 9). So profiles stay on this machine: the opt-in itself
//! lives here too (a profile file exists = recognition is on), and a second
//! machine turns recognition on again and **rebuilds** the profile from the
//! archive's confirmed links — the per-line embeddings in `segments.json`
//! make that a re-read, not a re-recording.
//!
//! - Turning recognition on builds the profile from the whole archive.
//! - Every change to a document's speakers or lines (link, unlink, move,
//!   re-detect, a deleted line or item) updates the profiles it touches
//!   ([`VoiceStore::document_changed`]): the profile's own documents plus
//!   the changed one are read again, so the result is the one a full
//!   rebuild would give.
//! - *Forget this voice* / turning recognition off, deleting the person,
//!   and *Forget all voices* delete the files (a real delete, never the
//!   trash: nothing of a voiceprint should linger).
//!
//! Nothing here logs a vector or a name; errors name the person id at
//! most. Vectors never leave this module and [`super::profiles`] — the UI
//! gets [`VoiceStatus`] only.

use super::profiles::{
    build_profile, links_person, person_lines, VoiceLines, VoiceProfile, MIN_PROFILE_DOCUMENTS,
    MIN_PROFILE_SPEECH_MS,
};
use crate::archive::store::{existing_item_dir, read_linked_segments, scan_item_dirs};
use crate::archive::SegmentsFile;
use anyhow::{bail, Context, Result};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// Folder of the profiles inside the app data dir.
pub const VOICES_DIR: &str = "voices";

/// Longest person id usable as a file name.
const MAX_ID_LEN: usize = 64;

/// Serializes every read-modify-write of the profiles in this process.
static VOICES_LOCK: Mutex<()> = Mutex::new(());

fn lock() -> std::sync::MutexGuard<'static, ()> {
    VOICES_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

/// What the UI may know of a person's voice profile (no vectors, ever).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct VoiceStatus {
    pub person_id: String,
    /// *Recognise this voice* is on (a profile file exists).
    pub enabled: bool,
    /// Seconds of confirmed speech, in ms.
    pub speech_ms: u64,
    /// Documents the confirmed lines come from.
    pub documents: usize,
    /// Enough of both to make suggestions (P12).
    pub ready: bool,
    pub min_speech_ms: u64,
    pub min_documents: usize,
    /// RFC 3339 time of the last build; empty when off.
    pub updated: String,
}

impl VoiceStatus {
    fn off(person_id: &str) -> Self {
        VoiceStatus {
            person_id: person_id.to_string(),
            enabled: false,
            speech_ms: 0,
            documents: 0,
            ready: false,
            min_speech_ms: MIN_PROFILE_SPEECH_MS,
            min_documents: MIN_PROFILE_DOCUMENTS,
            updated: String::new(),
        }
    }

    fn of(p: &VoiceProfile) -> Self {
        VoiceStatus {
            enabled: true,
            speech_ms: p.speech_ms,
            documents: p.documents.len(),
            ready: p.ready(),
            updated: p.updated.clone(),
            ..VoiceStatus::off(&p.person_id)
        }
    }
}

/// A person id that can name a file: `[A-Za-z0-9_-]`, at most 64 chars
/// (the registry's own ids are `p-<12 hex>`; a hand-edited id outside
/// this set can't get a profile).
fn check_id(person_id: &str) -> Result<()> {
    let ok = !person_id.is_empty()
        && person_id.len() <= MAX_ID_LEN
        && person_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
    if !ok {
        bail!("this person's id can't be used for a voice profile");
    }
    Ok(())
}

fn now() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

/// Profiles on disk. Cheap to create; all state is in the folder.
#[derive(Debug, Clone)]
pub struct VoiceStore {
    dir: PathBuf,
}

impl VoiceStore {
    /// The store in `<app data>/voices` (`app_data` is the app data dir).
    pub fn in_app_data(app_data: &Path) -> Self {
        Self::at(app_data.join(VOICES_DIR))
    }

    /// The store in `dir` itself.
    pub fn at(dir: PathBuf) -> Self {
        VoiceStore { dir }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn path(&self, person_id: &str) -> Result<PathBuf> {
        check_id(person_id)?;
        Ok(self.dir.join(format!("{person_id}.json")))
    }

    fn ensure_dir(&self) -> Result<()> {
        std::fs::create_dir_all(&self.dir)
            .with_context(|| format!("creating {}", self.dir.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&self.dir, std::fs::Permissions::from_mode(0o700))?;
        }
        Ok(())
    }

    fn read(&self, person_id: &str) -> Result<Option<VoiceProfile>> {
        let path = self.path(person_id)?;
        let bytes = match std::fs::read(&path) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e).context("reading a voice profile"),
        };
        let p: VoiceProfile =
            serde_json::from_slice(&bytes).context("a voice profile can't be read")?;
        if p.person_id != person_id {
            bail!("a voice profile file doesn't match its person");
        }
        Ok(Some(p))
    }

    fn write(&self, p: &VoiceProfile) -> Result<()> {
        let path = self.path(&p.person_id)?;
        self.ensure_dir()?;
        let json = serde_json::to_vec(p).context("serializing a voice profile")?;
        crate::settings::write_private_atomic(&path, &json).context("writing a voice profile")
    }

    /// Every readable profile, by person id. A file that can't be read is
    /// skipped (and left alone: *Forget* still deletes it).
    fn read_all(&self) -> Vec<VoiceProfile> {
        let Ok(entries) = std::fs::read_dir(&self.dir) else {
            return Vec::new();
        };
        let mut out: Vec<VoiceProfile> = entries
            .flatten()
            .filter_map(|e| {
                let name = e.file_name().to_string_lossy().into_owned();
                let id = name.strip_suffix(".json")?;
                self.read(id).ok().flatten()
            })
            .collect();
        out.sort_by(|a, b| a.person_id.cmp(&b.person_id));
        out
    }

    /// The profiles that can make suggestions, for the people in `people`
    /// (the current registry: a profile of someone no longer in People, or
    /// from another archive's registry, takes no part). For the backend's
    /// matching only (#242) — never hand these to the UI.
    pub fn ready_profiles(&self, people: &[String]) -> Vec<VoiceProfile> {
        let people: BTreeSet<&str> = people.iter().map(String::as_str).collect();
        self.read_all()
            .into_iter()
            .filter(|p| p.ready() && people.contains(p.person_id.as_str()))
            .collect()
    }

    /// Status of one person's profile (off when there is none).
    pub fn status(&self, person_id: &str) -> Result<VoiceStatus> {
        let _l = lock();
        Ok(match self.read(person_id)? {
            Some(p) => VoiceStatus::of(&p),
            None => VoiceStatus::off(person_id),
        })
    }

    /// Status of every profile, by person id.
    pub fn statuses(&self) -> Vec<VoiceStatus> {
        let _l = lock();
        self.read_all().iter().map(VoiceStatus::of).collect()
    }

    /// Turn *Recognise this voice* on for `person_id` and build the
    /// profile from the whole archive (also a rebuild when already on).
    pub fn enable(&self, archive: &Path, person_id: &str) -> Result<VoiceStatus> {
        check_id(person_id)?;
        let _l = lock();
        let built = self.scan(archive, &[person_id.to_string()])?;
        let p = built
            .into_iter()
            .next()
            .expect("one profile per person asked");
        self.write(&p)?;
        Ok(VoiceStatus::of(&p))
    }

    /// Rebuild every profile from the whole archive (a new machine, a
    /// moved archive, links made elsewhere). Returns the new statuses.
    pub fn rebuild_all(&self, archive: &Path) -> Result<Vec<VoiceStatus>> {
        let _l = lock();
        let ids: Vec<String> = self.enabled_ids();
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let built = self.scan(archive, &ids)?;
        for p in &built {
            self.write(p)?;
        }
        Ok(built.iter().map(VoiceStatus::of).collect())
    }

    /// Person ids with a profile file (readable or not).
    fn enabled_ids(&self) -> Vec<String> {
        let Ok(entries) = std::fs::read_dir(&self.dir) else {
            return Vec::new();
        };
        let mut ids: Vec<String> = entries
            .flatten()
            .filter_map(|e| {
                let name = e.file_name().to_string_lossy().into_owned();
                let id = name.strip_suffix(".json")?.to_string();
                check_id(&id).is_ok().then_some(id)
            })
            .collect();
        ids.sort();
        ids
    }

    /// One pass over the whole archive, building a profile for each of
    /// `ids` (in that order).
    fn scan(&self, archive: &Path, ids: &[String]) -> Result<Vec<VoiceProfile>> {
        let mut per_person: BTreeMap<&str, Vec<(String, VoiceLines)>> =
            ids.iter().map(|id| (id.as_str(), Vec::new())).collect();
        for (item, dir) in scan_item_dirs(archive) {
            // An unreadable document contributes nothing (as in the Library).
            let Ok(Some((source, file))) = read_linked_segments(&dir) else {
                continue;
            };
            for (person, docs) in per_person.iter_mut() {
                let lines = person_lines(&file, &source, person);
                if !lines.is_empty() {
                    docs.push((item.clone(), lines));
                }
            }
        }
        let updated = now();
        Ok(ids
            .iter()
            .map(|id| build_profile(id, &per_person[id.as_str()], &updated))
            .collect())
    }

    /// Document `item` changed (its speakers, its lines) or was deleted:
    /// update every profile it contributes to, or now links. Each such
    /// profile is rebuilt from its documents plus this one, read again —
    /// the same result as a full rebuild. A no-op without profiles.
    pub fn document_changed(&self, archive: &Path, item: &str) -> Result<()> {
        let _l = lock();
        let profiles = self.read_all();
        if profiles.is_empty() {
            return Ok(());
        }
        let changed = load_document(archive, item);
        let mut cache: BTreeMap<String, Option<(String, SegmentsFile)>> = BTreeMap::new();
        cache.insert(item.to_string(), changed);
        for p in profiles {
            let had = p.documents.iter().any(|d| d == item);
            let links = cache[item]
                .as_ref()
                .is_some_and(|(_, f)| links_person(f, &p.person_id));
            if !had && !links {
                continue;
            }
            let mut docs: BTreeSet<String> = p.documents.iter().cloned().collect();
            docs.insert(item.to_string());
            let mut lines: Vec<(String, VoiceLines)> = Vec::new();
            for d in docs {
                let data = cache
                    .entry(d.clone())
                    .or_insert_with(|| load_document(archive, &d));
                if let Some((source, file)) = data {
                    lines.push((d, person_lines(file, source, &p.person_id)));
                }
            }
            let next = build_profile(&p.person_id, &lines, &now());
            if !next.same_as(&p) {
                self.write(&next)?;
            }
        }
        Ok(())
    }

    /// Turn recognition off for `person_id`: its profile file is deleted.
    /// Returns whether there was one.
    pub fn forget(&self, person_id: &str) -> Result<bool> {
        let path = self.path(person_id)?;
        let _l = lock();
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(true),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(e).context("deleting a voice profile"),
        }
    }

    /// *Forget all voices*: delete every file in the voices folder (left
    /// over temp files included), then the folder. Returns how many
    /// profiles there were.
    pub fn forget_all(&self) -> Result<usize> {
        let _l = lock();
        let entries = match std::fs::read_dir(&self.dir) {
            Ok(e) => e,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(0),
            Err(e) => return Err(e).context("reading the voices folder"),
        };
        let mut n = 0;
        for entry in entries.flatten() {
            let path = entry.path();
            // Never follows a link out of the folder: remove_file on a
            // symlink removes the link.
            let is_profile = path.extension().is_some_and(|x| x == "json");
            if entry.file_type().is_ok_and(|t| !t.is_dir()) {
                std::fs::remove_file(&path).context("deleting a voice profile")?;
                n += usize::from(is_profile);
            }
        }
        // Fails only when something other than files is left (a folder
        // someone put there): it stays, the profiles are gone.
        let _ = std::fs::remove_dir(&self.dir);
        Ok(n)
    }
}

/// `(source, segments)` of item `id` when it links anyone; `None` when it
/// is gone, links nobody, or can't be read.
fn load_document(archive: &Path, id: &str) -> Option<(String, SegmentsFile)> {
    let dir = existing_item_dir(archive, id).ok()?;
    read_linked_segments(&dir).ok().flatten()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::archive::{create_item, Channel, ItemMeta, ItemType, SpeakerEdit};
    use crate::speakers::profiles::tests::{axis, line, speaker, towards};
    use crate::speakers::profiles::{suggest, Condition};

    struct Fixture {
        _tmp: tempfile::TempDir,
        archive: PathBuf,
        app_data: PathBuf,
        store: VoiceStore,
    }

    fn fixture() -> Fixture {
        let tmp = tempfile::tempdir().unwrap();
        let archive = tmp.path().join("archive");
        let app_data = tmp.path().join("app-data");
        std::fs::create_dir_all(&archive).unwrap();
        std::fs::create_dir_all(&app_data).unwrap();
        Fixture {
            store: VoiceStore::in_app_data(&app_data),
            _tmp: tmp,
            archive,
            app_data,
        }
    }

    fn add_person(archive: &Path, name: &str) -> String {
        crate::archive::people::modify(archive, |ps| {
            crate::archive::people::add_person(
                ps,
                &crate::archive::people::Person {
                    name: name.into(),
                    ..Default::default()
                },
            )
        })
        .unwrap()
        .id
    }

    /// A meeting with two voices: voice:1 (Anna's voice, `anna_ms` in two
    /// lines) and voice:2 (someone else), on the remote channel.
    fn meeting(archive: &Path, day: u32, anna_ms: u64) -> String {
        let meta = ItemMeta {
            item_type: ItemType::Meeting,
            title: format!("Weekly {day}"),
            date: format!("2026-09-{day:02}T10:00:00+02:00"),
            source: "browser:meet.google.com".into(),
            ..Default::default()
        };
        let half = anna_ms / 2;
        let file = SegmentsFile {
            speakers: vec![speaker("voice:1", None), speaker("voice:2", None)],
            segments: vec![
                line(
                    1,
                    "voice:1",
                    half,
                    towards(&axis(0), &axis(10 + day as usize), 0.9),
                    Channel::Remote,
                ),
                line(2, "voice:2", 20_000, axis(1), Channel::Remote),
                line(
                    3,
                    "voice:1",
                    anna_ms - half,
                    towards(&axis(0), &axis(40 + day as usize), 0.9),
                    Channel::Remote,
                ),
            ],
            ..Default::default()
        };
        let mut file = file;
        for s in &mut file.segments {
            s.text = format!("Riga {}.", s.id);
            s.raw = s.text.clone();
        }
        create_item(archive, &meta, &file).unwrap()
    }

    fn link(archive: &Path, item: &str, speaker: &str, person: &str) {
        crate::archive::edit_speakers(
            archive,
            item,
            SpeakerEdit::Link {
                speaker_id: speaker.into(),
                person_id: person.into(),
            },
        )
        .unwrap();
    }

    #[test]
    fn enable_builds_from_confirmed_lines_across_the_archive() {
        let f = fixture();
        let anna = add_person(&f.archive, "Anna");
        let d1 = meeting(&f.archive, 1, 40_000);
        let d2 = meeting(&f.archive, 2, 30_000);
        let _d3 = meeting(&f.archive, 3, 50_000); // not linked: not confirmed
        link(&f.archive, &d1, "voice:1", &anna);
        link(&f.archive, &d2, "voice:1", &anna);
        let st = f.store.enable(&f.archive, &anna).unwrap();
        assert!(st.enabled && st.ready);
        assert_eq!((st.speech_ms, st.documents), (70_000, 2));
        let p = f.store.read(&anna).unwrap().unwrap();
        let mut docs = vec![d1, d2];
        docs.sort();
        assert_eq!(p.documents, docs);
        // The profile recognises Anna's voice in a third meeting.
        assert_eq!(
            suggest(&[p], &towards(&axis(0), &axis(99), 0.85), Condition::Close)
                .unwrap()
                .person_id,
            anna
        );
    }

    #[test]
    fn below_the_minimums_the_profile_exists_but_is_not_ready() {
        let f = fixture();
        let anna = add_person(&f.archive, "Anna");
        let d1 = meeting(&f.archive, 1, 90_000);
        link(&f.archive, &d1, "voice:1", &anna);
        let st = f.store.enable(&f.archive, &anna).unwrap();
        assert!(st.enabled && !st.ready, "one document only");
        assert_eq!((st.min_speech_ms, st.min_documents), (60_000, 2));
        assert!(f.store.ready_profiles(&[anna.clone()]).is_empty());
        let d2 = meeting(&f.archive, 2, 10_000);
        link(&f.archive, &d2, "voice:1", &anna);
        f.store.document_changed(&f.archive, &d2).unwrap();
        assert!(f.store.status(&anna).unwrap().ready);
        assert_eq!(f.store.ready_profiles(&[anna.clone()]).len(), 1);
        // Someone no longer in People takes no part.
        assert!(f.store.ready_profiles(&["p-other".into()]).is_empty());
    }

    #[test]
    fn incremental_updates_match_a_full_rebuild() {
        let f = fixture();
        let anna = add_person(&f.archive, "Anna");
        let bob = add_person(&f.archive, "Bob");
        let d1 = meeting(&f.archive, 1, 40_000);
        let d2 = meeting(&f.archive, 2, 30_000);
        let d3 = meeting(&f.archive, 3, 20_000);
        link(&f.archive, &d1, "voice:1", &anna);
        f.store.enable(&f.archive, &anna).unwrap();
        f.store.enable(&f.archive, &bob).unwrap();

        // Link in d2 and d3, move a line away in d1, link Bob in d3.
        link(&f.archive, &d2, "voice:1", &anna);
        f.store.document_changed(&f.archive, &d2).unwrap();
        link(&f.archive, &d3, "voice:1", &anna);
        link(&f.archive, &d3, "voice:2", &bob);
        f.store.document_changed(&f.archive, &d3).unwrap();
        crate::archive::edit_speakers(
            &f.archive,
            &d1,
            SpeakerEdit::Move {
                segment_id: 3,
                speaker_id: "voice:2".into(),
            },
        )
        .unwrap();
        f.store.document_changed(&f.archive, &d1).unwrap();

        let a = f.store.read(&anna).unwrap().unwrap();
        let b = f.store.read(&bob).unwrap().unwrap();
        assert_eq!(a.speech_ms, 20_000 + 30_000 + 20_000, "moved line left out");
        assert_eq!(b.speech_ms, 20_000);

        // A full rebuild gives the same profiles.
        let rebuilt = f.store.rebuild_all(&f.archive).unwrap();
        assert_eq!(rebuilt.len(), 2);
        assert!(f.store.read(&anna).unwrap().unwrap().same_as(&a));
        assert!(f.store.read(&bob).unwrap().unwrap().same_as(&b));

        // Unlink in d2: it leaves Anna's profile.
        crate::archive::edit_speakers(
            &f.archive,
            &d2,
            SpeakerEdit::Unlink {
                speaker_id: "voice:1".into(),
            },
        )
        .unwrap();
        f.store.document_changed(&f.archive, &d2).unwrap();
        let a = f.store.read(&anna).unwrap().unwrap();
        assert!(!a.documents.contains(&d2));
        assert_eq!(a.speech_ms, 40_000);

        // Deleting d3 removes it from both.
        crate::archive::store::delete_item_with(&f.archive, &d3, |p| {
            std::fs::remove_dir_all(p).map_err(Into::into)
        })
        .unwrap();
        f.store.document_changed(&f.archive, &d3).unwrap();
        assert_eq!(f.store.read(&anna).unwrap().unwrap().documents, vec![d1]);
        let b = f.store.read(&bob).unwrap().unwrap();
        assert!(b.documents.is_empty() && b.centroid.is_empty());
        assert!(f.store.status(&bob).unwrap().enabled, "still on, no data");
    }

    #[test]
    fn rebuild_on_a_new_machine_is_deterministic() {
        let f = fixture();
        let anna = add_person(&f.archive, "Anna");
        for day in 1..=4 {
            let d = meeting(&f.archive, day, 15_000 * u64::from(day));
            link(&f.archive, &d, "voice:1", &anna);
        }
        f.store.enable(&f.archive, &anna).unwrap();
        let first = f.store.read(&anna).unwrap().unwrap();
        // "Another machine": a fresh app data dir, the same archive.
        let other = VoiceStore::in_app_data(&f.app_data.join("elsewhere"));
        other.enable(&f.archive, &anna).unwrap();
        let second = other.read(&anna).unwrap().unwrap();
        assert!(first.same_as(&second));
        assert_eq!(first.centroid, second.centroid);
    }

    #[test]
    fn document_changed_without_profiles_touches_nothing() {
        let f = fixture();
        let anna = add_person(&f.archive, "Anna");
        let d1 = meeting(&f.archive, 1, 40_000);
        link(&f.archive, &d1, "voice:1", &anna);
        f.store.document_changed(&f.archive, &d1).unwrap();
        assert!(!f.store.dir().exists(), "no opt-in, no folder, no profile");
        assert!(f.store.statuses().is_empty());
        assert!(!f.store.status(&anna).unwrap().enabled);
    }

    #[test]
    fn forget_one_and_forget_all_delete_the_files() {
        let f = fixture();
        let anna = add_person(&f.archive, "Anna");
        let bob = add_person(&f.archive, "Bob");
        f.store.enable(&f.archive, &anna).unwrap();
        f.store.enable(&f.archive, &bob).unwrap();
        assert_eq!(f.store.statuses().len(), 2);
        assert!(f.store.forget(&anna).unwrap());
        assert!(!f.store.forget(&anna).unwrap(), "already gone");
        assert!(!f.store.path(&anna).unwrap().exists());
        assert!(!f.store.status(&anna).unwrap().enabled);
        // A stray temp file goes too.
        std::fs::write(f.store.dir().join(".x.json.1.0.tmp"), b"{}").unwrap();
        assert_eq!(f.store.forget_all().unwrap(), 1);
        assert!(!f.store.dir().exists());
        assert!(f.store.statuses().is_empty());
        assert_eq!(f.store.forget_all().unwrap(), 0);
    }

    #[test]
    fn bad_person_ids_never_become_paths() {
        let f = fixture();
        for bad in ["", "../x", "a/b", "a.b", "p\0", &"p".repeat(65)] {
            assert!(f.store.enable(&f.archive, bad).is_err(), "{bad:?}");
            assert!(f.store.forget(bad).is_err(), "{bad:?}");
        }
        assert!(!f.store.dir().exists());
    }

    #[cfg(unix)]
    #[test]
    fn files_are_private() {
        use std::os::unix::fs::PermissionsExt;
        let f = fixture();
        let anna = add_person(&f.archive, "Anna");
        let d1 = meeting(&f.archive, 1, 40_000);
        link(&f.archive, &d1, "voice:1", &anna);
        f.store.enable(&f.archive, &anna).unwrap();
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&f.store.path(&anna).unwrap()), 0o600);
        assert_eq!(mode(f.store.dir()), 0o700);
        // An update keeps it private.
        f.store.document_changed(&f.archive, &d1).unwrap();
        assert_eq!(mode(&f.store.path(&anna).unwrap()), 0o600);
    }

    /// Every file under `dir`, recursively, as text.
    fn all_text(dir: &Path) -> String {
        let mut out = String::new();
        let mut stack = vec![dir.to_path_buf()];
        while let Some(d) = stack.pop() {
            for e in std::fs::read_dir(&d).unwrap().flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                } else {
                    out.push_str(&p.display().to_string());
                    out.push('\n');
                    out.push_str(&String::from_utf8_lossy(&std::fs::read(&p).unwrap()));
                }
            }
        }
        out
    }

    #[test]
    fn profiles_never_reach_the_archive_the_ui_or_exports() {
        let f = fixture();
        let anna = add_person(&f.archive, "Anna");
        let d1 = meeting(&f.archive, 1, 40_000);
        let d2 = meeting(&f.archive, 2, 40_000);
        link(&f.archive, &d1, "voice:1", &anna);
        link(&f.archive, &d2, "voice:1", &anna);
        let archive_before = all_text(&f.archive);
        let st = f.store.enable(&f.archive, &anna).unwrap();
        assert!(st.ready);
        let p = f.store.read(&anna).unwrap().unwrap();
        // The centroid's components as they are written to the profile
        // file: none of them may show up anywhere else.
        let profile_json = serde_json::to_string(&p.centroid).unwrap();
        let needles: Vec<String> = profile_json
            .trim_matches(['[', ']'])
            .split(',')
            .filter(|x| x.len() > 6)
            .map(str::to_string)
            .collect();
        assert!(!needles.is_empty());
        let leaks = |text: &str| needles.iter().find(|n| text.contains(n.as_str())).cloned();

        // Nothing was written into the archive.
        assert_eq!(all_text(&f.archive), archive_before);
        assert!(!f.store.dir().starts_with(&f.archive));

        // The status the UI gets.
        let status = serde_json::to_string(&f.store.statuses()).unwrap();
        assert!(
            !status.contains("centroid") && leaks(&status).is_none(),
            "{status}"
        );

        // Items as the UI gets them, and every item export format.
        for id in [&d1, &d2] {
            let item = crate::archive::read_item(&f.archive, id).unwrap();
            let ui = serde_json::to_string(&item.without_embeddings()).unwrap();
            assert!(!ui.contains("embedding") && leaks(&ui).is_none());
            for fmt in [
                crate::archive::export::ExportFormat::Md,
                crate::archive::export::ExportFormat::Txt,
                crate::archive::export::ExportFormat::Srt,
                crate::archive::export::ExportFormat::Vtt,
            ] {
                let out = crate::archive::export::export_item(&f.archive, id, fmt).unwrap();
                assert!(leaks(&out).is_none(), "{fmt:?}");
            }
        }

        // The portable config export, People included.
        let settings = crate::settings::Settings::default();
        let people = crate::archive::people::read_people(&f.archive);
        let bundle = f.app_data.join("export.json");
        crate::config_io::export_to(&bundle, &settings, &people).unwrap();
        let text = std::fs::read_to_string(&bundle).unwrap();
        assert!(leaks(&text).is_none() && !text.contains("centroid"));
    }
}
