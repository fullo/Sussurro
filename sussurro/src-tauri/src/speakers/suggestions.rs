//! Voice suggestions (0.11, #242; plan P12, 4.1): which person an unlinked
//! "Voice N" of an open document sounds like, and the *Not Anna* answers
//! the user gave per document.
//!
//! - A suggestion is only ever shown: linking takes the user's click on
//!   the existing Link path (participants and emails follow as today).
//! - Only unlinked acoustic voices (`voice:N`) are matched — "You" and
//!   Meet names have their own sources — and only against ready profiles
//!   of people still in People ([`VoiceStore::ready_profiles`]), by the
//!   rules of [`super::profiles::suggest`]: when the best match was
//!   dismissed for that voice, nothing is suggested (never the runner-up).
//! - The UI gets `{speaker_id, person_id}` pairs, never a score or a
//!   vector.
//!
//! **Dismissals** (*Not Anna*) are remembered per document and voice in
//! `<app data>/voice_dismissals.json` (0600): next to the profiles, on
//! this machine, never in the archive. They name item, voice and person
//! ids only. A deleted item drops its entries; *Forget all voices* drops
//! them all.

use super::doc::voice_number;
use super::profiles::{suggest_for_speaker, VoiceProfile};
use super::voices::VoiceStore;
use crate::archive::SegmentsFile;
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// File of the *Not X* answers inside the app data dir.
pub const DISMISSALS_FILE: &str = "voice_dismissals.json";
/// Longest id (item, speaker or person) a dismissal accepts.
const MAX_ID_LEN: usize = 200;

static DISMISSALS_LOCK: Mutex<()> = Mutex::new(());

fn lock() -> std::sync::MutexGuard<'static, ()> {
    DISMISSALS_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

/// "Voice N sounds like this person" — what the speaker panel shows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct VoiceSuggestion {
    pub speaker_id: String,
    pub person_id: String,
}

/// *Not X*: `person_id` is not the voice `speaker_id` of one document.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Dismissal {
    pub speaker_id: String,
    pub person_id: String,
}

/// The suggestions for a document's speakers (pure): each unlinked
/// `voice:N` whose voice matches a profile, unless the user said *Not X*
/// for that voice and that person.
pub fn suggestions_for(
    profiles: &[VoiceProfile],
    file: &SegmentsFile,
    source: &str,
    dismissed: &[Dismissal],
) -> Vec<VoiceSuggestion> {
    if profiles.is_empty() {
        return Vec::new();
    }
    file.speakers
        .iter()
        .filter(|sp| sp.person_id.is_none() && voice_number(&sp.id).is_some())
        .filter_map(|sp| {
            let s = suggest_for_speaker(profiles, file, source, &sp.id)?;
            let no = dismissed
                .iter()
                .any(|d| d.speaker_id == sp.id && d.person_id == s.person_id);
            (!no).then(|| VoiceSuggestion {
                speaker_id: sp.id.clone(),
                person_id: s.person_id,
            })
        })
        .collect()
}

/// The suggestions for archive item `id`: nothing while it records, or
/// when no profile is ready.
pub fn item_suggestions(
    archive: &Path,
    store: &VoiceStore,
    dismissals: &Dismissals,
    id: &str,
) -> Result<Vec<VoiceSuggestion>> {
    let people: Vec<String> = crate::archive::people::list_people(archive)?
        .into_iter()
        .map(|p| p.id)
        .collect();
    let profiles = store.ready_profiles(&people);
    if profiles.is_empty() {
        return Ok(Vec::new());
    }
    let item = crate::archive::read_item(archive, id)?;
    if item.recording {
        return Ok(Vec::new());
    }
    Ok(suggestions_for(
        &profiles,
        &item.segments,
        &item.meta.source,
        &dismissals.for_item(id),
    ))
}

#[derive(Default, Serialize, Deserialize)]
struct DismissalsFile {
    #[serde(default)]
    items: BTreeMap<String, BTreeSet<Dismissal>>,
}

fn check(id: &str) -> Result<()> {
    if id.is_empty() || id.len() > MAX_ID_LEN || id.chars().any(char::is_control) {
        bail!("not a valid id");
    }
    Ok(())
}

/// The *Not X* answers on disk.
#[derive(Debug, Clone)]
pub struct Dismissals {
    path: PathBuf,
}

impl Dismissals {
    /// `<app data>/voice_dismissals.json`.
    pub fn in_app_data(app_data: &Path) -> Self {
        Self::at(app_data.join(DISMISSALS_FILE))
    }

    pub fn at(path: PathBuf) -> Self {
        Dismissals { path }
    }

    /// An unreadable file reads as empty (suggestions come back; it is
    /// replaced on the next dismissal).
    fn read(&self) -> DismissalsFile {
        std::fs::read(&self.path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    fn write(&self, f: &DismissalsFile) -> Result<()> {
        if f.items.is_empty() {
            return match std::fs::remove_file(&self.path) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
                    Err(e).context("deleting the voice dismissals")
                }
                _ => Ok(()),
            };
        }
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir).context("creating the app data folder")?;
        }
        let json = serde_json::to_vec(f).context("serializing the voice dismissals")?;
        crate::settings::write_private_atomic(&self.path, &json)
            .context("writing the voice dismissals")
    }

    /// The *Not X* answers of item `item`.
    pub fn for_item(&self, item: &str) -> Vec<Dismissal> {
        let _l = lock();
        self.read()
            .items
            .remove(item)
            .map(|s| s.into_iter().collect())
            .unwrap_or_default()
    }

    /// Remember *Not `person_id`* for voice `speaker_id` of item `item`.
    pub fn dismiss(&self, item: &str, speaker_id: &str, person_id: &str) -> Result<()> {
        check(item)?;
        check(speaker_id)?;
        check(person_id)?;
        let _l = lock();
        let mut f = self.read();
        let added = f
            .items
            .entry(item.to_string())
            .or_default()
            .insert(Dismissal {
                speaker_id: speaker_id.to_string(),
                person_id: person_id.to_string(),
            });
        if added {
            self.write(&f)?;
        }
        Ok(())
    }

    /// Item `item` was deleted: drop its answers.
    pub fn forget_item(&self, item: &str) -> Result<()> {
        let _l = lock();
        let mut f = self.read();
        if f.items.remove(item).is_some() {
            self.write(&f)?;
        }
        Ok(())
    }

    /// *Forget all voices*: drop every answer (the file is deleted).
    pub fn clear(&self) -> Result<()> {
        let _l = lock();
        self.write(&DismissalsFile::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::archive::{create_item, Channel, ItemMeta, ItemType, SpeakerEdit};
    use crate::speakers::profiles::tests::{axis, line, speaker, towards};

    struct Fixture {
        _tmp: tempfile::TempDir,
        archive: PathBuf,
        store: VoiceStore,
        dismissals: Dismissals,
    }

    fn fixture() -> Fixture {
        let tmp = tempfile::tempdir().unwrap();
        let archive = tmp.path().join("archive");
        let app_data = tmp.path().join("app-data");
        std::fs::create_dir_all(&archive).unwrap();
        std::fs::create_dir_all(&app_data).unwrap();
        Fixture {
            store: VoiceStore::in_app_data(&app_data),
            dismissals: Dismissals::in_app_data(&app_data),
            _tmp: tmp,
            archive,
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

    /// A browser meeting: voice:1 sounds like axis 0 (Anna), voice:2 like
    /// axis 1 (Bruno), "you" on the mic like axis 0 too.
    fn meeting(archive: &Path, day: u32) -> String {
        let meta = ItemMeta {
            item_type: ItemType::Meeting,
            title: format!("Weekly {day}"),
            date: format!("2026-09-{day:02}T10:00:00+02:00"),
            source: "browser:meet.google.com".into(),
            ..Default::default()
        };
        let d = day as usize;
        let mut file = SegmentsFile {
            speakers: vec![
                speaker("voice:1", None),
                speaker("voice:2", None),
                speaker("you", None),
            ],
            segments: vec![
                line(
                    1,
                    "voice:1",
                    35_000,
                    towards(&axis(0), &axis(10 + d), 0.9),
                    Channel::Remote,
                ),
                line(
                    2,
                    "voice:2",
                    35_000,
                    towards(&axis(1), &axis(40 + d), 0.9),
                    Channel::Remote,
                ),
                line(
                    3,
                    "you",
                    20_000,
                    towards(&axis(0), &axis(80 + d), 0.9),
                    Channel::Mic,
                ),
            ],
            ..Default::default()
        };
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

    /// Anna and Bruno linked in two meetings, recognition on for both.
    fn enrolled(f: &Fixture) -> (String, String) {
        let anna = add_person(&f.archive, "Anna");
        let bruno = add_person(&f.archive, "Bruno");
        for day in [1, 2] {
            let d = meeting(&f.archive, day);
            link(&f.archive, &d, "voice:1", &anna);
            link(&f.archive, &d, "voice:2", &bruno);
        }
        assert!(f.store.enable(&f.archive, &anna).unwrap().ready);
        assert!(f.store.enable(&f.archive, &bruno).unwrap().ready);
        (anna, bruno)
    }

    fn pair(s: &str, p: &str) -> VoiceSuggestion {
        VoiceSuggestion {
            speaker_id: s.into(),
            person_id: p.into(),
        }
    }

    #[test]
    fn a_new_meeting_gets_a_suggestion_per_unlinked_voice() {
        let f = fixture();
        let (anna, bruno) = enrolled(&f);
        let new = meeting(&f.archive, 3);
        let got = item_suggestions(&f.archive, &f.store, &f.dismissals, &new).unwrap();
        // "you" sounds like Anna too, but only voice:N ids are matched.
        assert_eq!(got, vec![pair("voice:1", &anna), pair("voice:2", &bruno)]);
    }

    #[test]
    fn linked_voices_get_no_suggestion() {
        let f = fixture();
        let (_, bruno) = enrolled(&f);
        let new = meeting(&f.archive, 3);
        link(&f.archive, &new, "voice:1", &bruno); // even a "wrong" link stays
        let got = item_suggestions(&f.archive, &f.store, &f.dismissals, &new).unwrap();
        assert_eq!(got, vec![pair("voice:2", &bruno)]);
    }

    #[test]
    fn a_dismissal_hides_that_person_for_that_voice_of_that_document_only() {
        let f = fixture();
        let (anna, bruno) = enrolled(&f);
        let new = meeting(&f.archive, 3);
        let other = meeting(&f.archive, 4);
        f.dismissals.dismiss(&new, "voice:1", &anna).unwrap();
        f.dismissals.dismiss(&new, "voice:1", &anna).unwrap(); // idempotent
                                                               // Never the runner-up instead: voice:1 simply has no suggestion.
        assert_eq!(
            item_suggestions(&f.archive, &f.store, &f.dismissals, &new).unwrap(),
            vec![pair("voice:2", &bruno)]
        );
        assert_eq!(
            item_suggestions(&f.archive, &f.store, &f.dismissals, &other).unwrap(),
            vec![pair("voice:1", &anna), pair("voice:2", &bruno)]
        );
        // Dismissing another person for that voice doesn't hide Anna.
        f.dismissals.dismiss(&other, "voice:1", &bruno).unwrap();
        assert_eq!(
            item_suggestions(&f.archive, &f.store, &f.dismissals, &other)
                .unwrap()
                .len(),
            2
        );
    }

    #[test]
    fn no_profile_ready_or_person_gone_means_no_suggestion() {
        let f = fixture();
        let anna = add_person(&f.archive, "Anna");
        let d1 = meeting(&f.archive, 1);
        link(&f.archive, &d1, "voice:1", &anna);
        // One document only: not ready.
        assert!(!f.store.enable(&f.archive, &anna).unwrap().ready);
        let new = meeting(&f.archive, 3);
        assert!(item_suggestions(&f.archive, &f.store, &f.dismissals, &new)
            .unwrap()
            .is_empty());

        let f = fixture();
        let (anna, bruno) = enrolled(&f);
        crate::archive::people::modify(&f.archive, |ps| {
            crate::archive::people::delete_person(ps, &anna)
        })
        .unwrap();
        let new = meeting(&f.archive, 3);
        assert_eq!(
            item_suggestions(&f.archive, &f.store, &f.dismissals, &new).unwrap(),
            vec![pair("voice:2", &bruno)]
        );
        // Recognition off (profile forgotten): gone.
        f.store.forget(&bruno).unwrap();
        assert!(item_suggestions(&f.archive, &f.store, &f.dismissals, &new)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn an_unknown_item_is_an_error_without_profiles_it_is_empty() {
        let f = fixture();
        assert!(
            item_suggestions(&f.archive, &f.store, &f.dismissals, "nope")
                .unwrap()
                .is_empty()
        );
        enrolled(&f);
        assert!(item_suggestions(&f.archive, &f.store, &f.dismissals, "nope").is_err());
    }

    #[test]
    fn dismissals_persist_privately_and_are_forgotten() {
        let f = fixture();
        let d = &f.dismissals;
        d.dismiss("item-a", "voice:1", "p-1").unwrap();
        d.dismiss("item-a", "voice:2", "p-2").unwrap();
        d.dismiss("item-b", "voice:1", "p-1").unwrap();
        let again = Dismissals::at(d.path.clone());
        assert_eq!(again.for_item("item-a").len(), 2);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&d.path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        d.forget_item("item-a").unwrap();
        assert!(d.for_item("item-a").is_empty());
        assert_eq!(d.for_item("item-b").len(), 1);
        d.clear().unwrap();
        assert!(!d.path.exists());
        assert!(d.for_item("item-b").is_empty());
        d.clear().unwrap(); // nothing there: fine
    }

    #[test]
    fn dismissals_refuse_bad_ids_and_survive_a_broken_file() {
        let f = fixture();
        let d = &f.dismissals;
        assert!(d.dismiss("", "voice:1", "p").is_err());
        assert!(d
            .dismiss("i", "voice:1", &"x".repeat(MAX_ID_LEN + 1))
            .is_err());
        assert!(d.dismiss("i", "voice\n1", "p").is_err());
        std::fs::write(&d.path, b"{not json").unwrap();
        assert!(d.for_item("i").is_empty());
        d.dismiss("i", "voice:1", "p").unwrap();
        assert_eq!(d.for_item("i").len(), 1);
    }

    #[test]
    fn the_ui_payload_has_ids_only() {
        let json = serde_json::to_string(&pair("voice:1", "p-1")).unwrap();
        assert_eq!(json, r#"{"speaker_id":"voice:1","person_id":"p-1"}"#);
    }
}
