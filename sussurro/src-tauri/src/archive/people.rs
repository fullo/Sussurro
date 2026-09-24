//! People registry (0.9, #132; plan P5, §4.3): name, email and aliases of
//! the people who show up in meetings and transcriptions, filled once and
//! reused to give participants their email automatically.
//!
//! ```text
//! <archive>/.sussurro/people.json   { "version": 1, "people": [Person…] }
//! ```
//!
//! It lives in the archive so it travels with it (a synced or moved archive
//! keeps its people). Other people's emails are personal data: the registry
//! stays local, is never logged or put in diagnostics, and leaves the
//! machine only when the user explicitly includes it in a config export.
//!
//! **Matching**: a display name (a participant's name, a speaker label)
//! matches a person when it equals the person's name or one of its aliases
//! ignoring case, accents and surrounding / repeated whitespace
//! ([`name_key`]). A name that matches more than one person is ambiguous
//! and links to nobody.
//!
//! **Linking** only ever *adds* an email to a participant that has none;
//! it never changes a name or replaces an email. Deleting or editing a
//! person never touches existing items — only future linking.
//!
//! **Tolerant reads**, like the rest of the archive: a missing file is an
//! empty registry, a hand-edited entry that can't be read is skipped
//! without hiding the others, an entry without an id gets a stable one.
//! A file that isn't JSON at all reads as empty for linking, but writes
//! are refused until it is fixed — overwriting it would lose the user's
//! list.

use super::store::{read_item, write_atomic, META_DIR};
use super::types::{DocSpeaker, ItemMeta, Participant};
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use unicode_normalization::UnicodeNormalization;

/// Registry file inside `<archive>/.sussurro/`.
pub const PEOPLE_FILE: &str = "people.json";
/// `version` written to the file.
pub const PEOPLE_VERSION: u32 = 1;

/// One person of the registry.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct Person {
    /// Stable id (`p-<hex>`); links from documents (`DocSpeaker::person_id`).
    #[serde(default)]
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    /// Other spellings: a Meet display name, a nickname, a renamed voice.
    #[serde(default)]
    pub aliases: Vec<String>,
}

#[derive(Serialize)]
struct PeopleFile<'a> {
    version: u32,
    people: &'a [Person],
}

/// Serializes read-modify-write cycles on the registry within the process.
static PEOPLE_LOCK: Mutex<()> = Mutex::new(());

fn lock_people() -> std::sync::MutexGuard<'static, ()> {
    PEOPLE_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

/// `<archive>/.sussurro/people.json`.
pub fn people_path(archive: &Path) -> PathBuf {
    archive.join(META_DIR).join(PEOPLE_FILE)
}

// ---- Matching ----

/// Comparison key of a name: compatibility-decomposed with the combining
/// marks dropped (è → e, ﬁ → fi), lowercased, whitespace trimmed and
/// collapsed. Letters without a decomposition (ø, ł, CJK) are kept as is.
pub fn name_key(s: &str) -> String {
    let folded: String = s
        .nfkd()
        .filter(|c| !unicode_normalization::char::is_combining_mark(*c))
        .collect::<String>()
        .to_lowercase();
    folded.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn email_key(s: &str) -> String {
    s.trim().to_lowercase()
}

impl Person {
    /// Does `display` match this person's name or one of its aliases?
    pub fn matches(&self, display: &str) -> bool {
        let key = name_key(display);
        !key.is_empty()
            && (name_key(&self.name) == key || self.aliases.iter().any(|a| name_key(a) == key))
    }
}

/// Every person `display` matches.
pub fn find_matches<'a>(people: &'a [Person], display: &str) -> Vec<&'a Person> {
    people.iter().filter(|p| p.matches(display)).collect()
}

/// The one person `display` matches; `None` when nobody or more than one
/// person does (an ambiguous alias links to nobody).
pub fn match_person<'a>(people: &'a [Person], display: &str) -> Option<&'a Person> {
    match find_matches(people, display).as_slice() {
        [one] => Some(one),
        _ => None,
    }
}

/// Give each participant without an email the email of the person its name
/// matches, unless `skip` says to leave it alone. Names and existing emails
/// are never changed. Returns how many participants were linked.
pub fn link_participants(
    participants: &mut [Participant],
    people: &[Person],
    skip: impl Fn(&Participant) -> bool,
) -> usize {
    let mut linked = 0;
    for p in participants.iter_mut() {
        if p.email.as_deref().is_some_and(|e| !e.trim().is_empty()) || skip(p) {
            continue;
        }
        if let Some(email) = match_person(people, &p.name).and_then(|m| m.email.clone()) {
            p.email = Some(email);
            linked += 1;
        }
    }
    linked
}

/// Linking on save: participants of `new` that were not already in `old`
/// (by name) get their email from the registry. Participants already on
/// the item are left alone, so a user who removed an email by hand doesn't
/// see it come back; the chip editor offers those a one-click link instead.
/// Notes never have participants (P10) and are not touched.
pub fn link_new_participants(old: &[Participant], new: &mut ItemMeta, people: &[Person]) -> usize {
    if people.is_empty() || !new.item_type.has_participants() {
        return 0;
    }
    let known: HashSet<String> = old.iter().map(|p| name_key(&p.name)).collect();
    link_participants(&mut new.participants, people, |p| known.contains(&name_key(&p.name)))
}

/// [`link_new_participants`] against item `id`'s current frontmatter and
/// the registry of `archive`. An unreadable item or registry links nothing
/// (the update itself reports the item's problems).
pub fn link_on_save(archive: &Path, id: &str, meta: &mut ItemMeta) -> usize {
    if !meta.item_type.has_participants() {
        return 0;
    }
    let people = read_people(archive);
    if people.is_empty() {
        return 0;
    }
    let old = read_item(archive, id)
        .map(|item| item.meta.participants)
        .unwrap_or_default();
    link_new_participants(&old, meta, &people)
}

/// Link document speakers whose label matches a person (sets
/// `person_id` where it is unset). For the speaker panel and the Meet name
/// observer (#130, #131). Returns how many were linked.
pub fn link_speakers(speakers: &mut [DocSpeaker], people: &[Person]) -> usize {
    let mut linked = 0;
    for s in speakers.iter_mut() {
        if s.person_id.is_some() {
            continue;
        }
        if let Some(p) = match_person(people, &s.label) {
            s.person_id = Some(p.id.clone());
            linked += 1;
        }
    }
    linked
}

/// "Appears in N items": for each person, the number of distinct items
/// with a participant that is them — same email, or a name matching them
/// unambiguously. `rows` are `(item id, name, email)` as the index stores
/// them (email empty when unknown).
pub fn usage(people: &[Person], rows: &[(String, String, String)]) -> HashMap<String, usize> {
    let by_email: HashMap<String, &str> = people
        .iter()
        .filter_map(|p| p.email.as_deref().map(|e| (email_key(e), p.id.as_str())))
        .collect();
    let mut items: HashMap<&str, HashSet<&str>> = HashMap::new();
    for (item, name, email) in rows {
        let who = if email.trim().is_empty() {
            None
        } else {
            by_email.get(&email_key(email)).copied()
        }
        .or_else(|| match_person(people, name).map(|p| p.id.as_str()));
        if let Some(id) = who {
            items.entry(id).or_default().insert(item.as_str());
        }
    }
    people
        .iter()
        .map(|p| (p.id.clone(), items.get(p.id.as_str()).map_or(0, HashSet::len)))
        .collect()
}

// ---- Validation and edits (pure) ----

/// Basic shape check, not RFC 5322 (mirrors `isValidEmail` in
/// `src/lib/participants.ts`): `local@domain.tld`, no spaces, `<>`, `,`, `;`.
pub fn is_valid_email(email: &str) -> bool {
    let bad = |c: char| c.is_whitespace() || matches!(c, '<' | '>' | ',' | ';');
    let e = email.trim();
    let Some((local, domain)) = e.split_once('@') else {
        return false;
    };
    if local.is_empty() || domain.contains('@') || e.chars().any(bad) {
        return false;
    }
    match domain.rsplit_once('.') {
        Some((host, tld)) => !host.is_empty() && !tld.is_empty(),
        None => false,
    }
}

fn collapse(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// A person as it is stored: name and aliases trimmed and collapsed, the
/// email trimmed (empty = none) and checked, aliases without blanks,
/// repeats or copies of the name. The id is kept.
pub fn clean(person: &Person) -> Result<Person> {
    let name = collapse(&person.name);
    if name.is_empty() {
        bail!("a person needs a name");
    }
    let email = person
        .email
        .as_deref()
        .map(str::trim)
        .filter(|e| !e.is_empty())
        .map(str::to_string);
    if let Some(e) = &email {
        if !is_valid_email(e) {
            bail!("“{e}” doesn't look like an email");
        }
    }
    let mut seen = HashSet::from([name_key(&name)]);
    let aliases = person
        .aliases
        .iter()
        .map(|a| collapse(a))
        .filter(|a| !a.is_empty() && seen.insert(name_key(a)))
        .collect();
    Ok(Person { id: person.id.clone(), name, email, aliases })
}

/// Refuse `p` when another person (not `skip_id`) has the same name or the
/// same email: that is a duplicate to merge, not a second person. Aliases
/// may overlap (an ambiguous alias simply links to nobody).
fn check_conflicts(people: &[Person], p: &Person, skip_id: &str) -> Result<()> {
    let key = name_key(&p.name);
    for other in people.iter().filter(|o| o.id != skip_id) {
        if name_key(&other.name) == key {
            bail!("{} is already in People — edit that entry or merge the two", other.name);
        }
        if let (Some(a), Some(b)) = (&p.email, &other.email) {
            if email_key(a) == email_key(b) {
                bail!("{b} already belongs to {} — merge the two instead", other.name);
            }
        }
    }
    Ok(())
}

/// A fresh, unique-enough id: `p-` + 12 hex digits of a hash of the clock,
/// the process id and a per-process counter.
pub fn new_id() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    let seed = format!("{nanos}-{}-{n}", std::process::id());
    format!("p-{}", &super::store::sha256_hex(seed.as_bytes())[..12])
}

/// Stable id for a hand-written entry without one (same name, same id).
fn derived_id(name: &str) -> String {
    format!("p-{}", &super::store::sha256_hex(name_key(name).as_bytes())[..12])
}

/// Add a new person (a fresh id is assigned). Returns it as stored.
pub fn add_person(people: &mut Vec<Person>, input: &Person) -> Result<Person> {
    let mut p = clean(input)?;
    check_conflicts(people, &p, "")?;
    p.id = loop {
        let id = new_id();
        if !people.iter().any(|o| o.id == id) {
            break id;
        }
    };
    people.push(p.clone());
    Ok(p)
}

/// Replace the person with `input.id`. Returns it as stored.
pub fn update_person(people: &mut [Person], input: &Person) -> Result<Person> {
    let Some(at) = people.iter().position(|p| p.id == input.id) else {
        bail!("that person is no longer in People");
    };
    let p = clean(input)?;
    check_conflicts(people, &p, &p.id)?;
    people[at] = p.clone();
    Ok(p)
}

/// Remove the person `id`. Existing items keep their participants and
/// emails: deleting only stops future linking.
pub fn delete_person(people: &mut Vec<Person>, id: &str) -> Result<()> {
    let before = people.len();
    people.retain(|p| p.id != id);
    if people.len() == before {
        bail!("that person is no longer in People");
    }
    Ok(())
}

/// Merge duplicates into `into`: their names and aliases become aliases of
/// `into`, `into` keeps its email or takes the first one the others have,
/// and the others are removed. Items are not touched. Returns the merged
/// person.
pub fn merge_people(people: &mut Vec<Person>, into: &str, from: &[String]) -> Result<Person> {
    let from: Vec<&String> = from.iter().filter(|f| f.as_str() != into).collect();
    if from.is_empty() {
        bail!("pick at least one other person to merge");
    }
    let Some(target) = people.iter().find(|p| p.id == into).cloned() else {
        bail!("that person is no longer in People");
    };
    let mut merged = target;
    for id in &from {
        let Some(other) = people.iter().find(|p| &p.id == *id) else {
            bail!("that person is no longer in People");
        };
        merged.aliases.push(other.name.clone());
        merged.aliases.extend(other.aliases.iter().cloned());
        if merged.email.is_none() {
            merged.email = other.email.clone();
        }
    }
    let merged = clean(&merged)?;
    people.retain(|p| !from.contains(&&p.id));
    if let Some(slot) = people.iter_mut().find(|p| p.id == into) {
        *slot = merged.clone();
    }
    Ok(merged)
}

/// Add people from a config bundle: those whose name or email is already in
/// the registry are skipped; an id already taken gets a fresh one. Returns
/// how many were added.
pub fn import_people(people: &mut Vec<Person>, incoming: &[Person]) -> usize {
    let mut added = 0;
    for p in incoming {
        let Ok(mut p) = clean(p) else { continue };
        if check_conflicts(people, &p, "\0").is_err() {
            continue;
        }
        if p.id.trim().is_empty() || people.iter().any(|o| o.id == p.id) {
            p.id = new_id();
        }
        people.push(p);
        added += 1;
    }
    added
}

// ---- File ----

/// A person from one hand-editable JSON entry; `None` when it has no name.
/// Accepts a bare string (just a name) and `aliases` as a string or a list.
fn person_from_value(v: &Value) -> Option<Person> {
    let text = |v: Option<&Value>| match v {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Number(n)) => n.to_string(),
        _ => String::new(),
    };
    let p = match v {
        Value::String(s) => Person { name: s.clone(), ..Default::default() },
        Value::Object(m) => Person {
            id: text(m.get("id")).trim().to_string(),
            name: text(m.get("name")),
            email: Some(text(m.get("email"))),
            aliases: match m.get("aliases") {
                Some(Value::Array(a)) => a.iter().map(|x| text(Some(x))).collect(),
                Some(Value::String(s)) => vec![s.clone()],
                _ => Vec::new(),
            },
        },
        _ => return None,
    };
    let mut p = clean(&p).or_else(|_| clean(&Person { email: None, ..p.clone() })).ok()?;
    if p.id.is_empty() {
        p.id = derived_id(&p.name);
    }
    Some(p)
}

/// Parse the registry file: `{ "people": [...] }` or a bare list. Entries
/// that can't be read are skipped; repeated ids get a suffix so no entry is
/// lost on the next write. `Err` only when the text isn't JSON of either
/// shape.
pub fn parse_people(text: &str) -> Result<Vec<Person>> {
    let v: Value = serde_json::from_str(text).context("people.json is not valid JSON")?;
    let list = match &v {
        Value::Array(a) => a,
        Value::Object(m) => match m.get("people") {
            Some(Value::Array(a)) => a,
            None | Some(Value::Null) => return Ok(Vec::new()),
            Some(_) => bail!("people.json: \"people\" is not a list"),
        },
        _ => bail!("people.json is not a list of people"),
    };
    let mut ids = HashSet::new();
    let mut out = Vec::new();
    for entry in list {
        let Some(mut p) = person_from_value(entry) else { continue };
        let base = p.id.clone();
        let mut n = 2;
        while !ids.insert(p.id.clone()) {
            p.id = format!("{base}-{n}");
            n += 1;
        }
        out.push(p);
    }
    Ok(out)
}

/// The registry file as JSON text.
pub fn to_json(people: &[Person]) -> Result<String> {
    Ok(serde_json::to_string_pretty(&PeopleFile { version: PEOPLE_VERSION, people })?)
}

/// Strict load for a write: missing = empty, unreadable = error.
fn load_for_write(archive: &Path) -> Result<Vec<Person>> {
    let path = people_path(archive);
    match std::fs::read_to_string(&path) {
        Ok(text) => parse_people(&text).map_err(|e| {
            anyhow::anyhow!(
                "{} can't be read ({e:#}), so Sussurro won't overwrite it. Fix it in a text \
                 editor, or move it away to start a new list.",
                path.display()
            )
        }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
    }
}

/// The registry of `archive`, tolerant: missing or unreadable = empty.
pub fn read_people(archive: &Path) -> Vec<Person> {
    std::fs::read_to_string(people_path(archive))
        .ok()
        .and_then(|t| parse_people(&t).ok())
        .unwrap_or_default()
}

/// The registry sorted by name (case- and accent-insensitive) for the UI.
/// Unlike [`read_people`], a file that can't be read is an error, so the
/// People screen can say so instead of showing an empty list.
pub fn list_people(archive: &Path) -> Result<Vec<Person>> {
    let mut people = load_for_write(archive)?;
    people.sort_by_cached_key(|p| (name_key(&p.name), p.id.clone()));
    Ok(people)
}

/// Read-modify-write the registry under the lock; the file is written
/// atomically only when `f` succeeds.
pub fn modify<T>(archive: &Path, f: impl FnOnce(&mut Vec<Person>) -> Result<T>) -> Result<T> {
    let _lock = lock_people();
    let mut people = load_for_write(archive)?;
    let out = f(&mut people)?;
    let dir = archive.join(META_DIR);
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    write_atomic(&people_path(archive), to_json(&people)?.as_bytes())
        .context("saving the People registry")?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::archive::store::{create_item, update_meta};
    use crate::archive::types::{ItemType, SegmentsFile};

    fn person(name: &str, email: Option<&str>, aliases: &[&str]) -> Person {
        Person {
            id: String::new(),
            name: name.into(),
            email: email.map(Into::into),
            aliases: aliases.iter().map(|a| a.to_string()).collect(),
        }
    }

    fn part(name: &str, email: Option<&str>) -> Participant {
        Participant { name: name.into(), email: email.map(Into::into) }
    }

    fn registry(list: &[Person]) -> Vec<Person> {
        let mut people = Vec::new();
        for p in list {
            add_person(&mut people, p).unwrap();
        }
        people
    }

    #[test]
    fn name_key_folds_case_accents_and_spaces() {
        assert_eq!(name_key("  Nicolò   CÀRDENAS "), "nicolo cardenas");
        assert_eq!(name_key("José"), name_key("jose"));
        assert_eq!(name_key("Zoë\tMüller"), "zoe muller");
        assert_eq!(name_key("ﬁona"), "fiona");
        // Letters with no decomposition and non-Latin scripts are kept.
        assert_eq!(name_key("Łukasz Søren"), "łukasz søren");
        assert_eq!(name_key("王 伟"), "王 伟");
        assert_eq!(name_key("   "), "");
    }

    #[test]
    fn matches_name_and_aliases_ignoring_case_and_accents() {
        let people = registry(&[person("Nicolò Rossi", Some("nico@example.com"), &["Nico", "N. Rossi"])]);
        for display in ["nicolo rossi", "NICOLÒ ROSSI", "  Nicolò  Rossi ", "nico", "n. rossi"] {
            assert!(match_person(&people, display).is_some(), "{display}");
        }
        for display in ["Nicola Rossi", "Rossi", "", "   "] {
            assert!(match_person(&people, display).is_none(), "{display}");
        }
    }

    #[test]
    fn an_ambiguous_alias_matches_nobody() {
        let people = registry(&[
            person("Marco Bianchi", Some("mb@example.com"), &["Marco"]),
            person("Marco Verdi", Some("mv@example.com"), &["marco"]),
        ]);
        assert_eq!(find_matches(&people, "Marco").len(), 2);
        assert!(match_person(&people, "Marco").is_none());
        assert!(match_person(&people, "marco verdi").is_some());
    }

    #[test]
    fn linking_adds_emails_only_where_missing() {
        let people = registry(&[
            person("Anna Rossi", Some("anna@example.com"), &["Annie"]),
            person("Bruno", None, &[]),
        ]);
        let mut list = vec![
            part("annie", None),
            part("Anna Rossi", Some("other@example.com")),
            part("Bruno", None),
            part("Voice 2", None),
        ];
        assert_eq!(link_participants(&mut list, &people, |_| false), 1);
        assert_eq!(list[0], part("annie", Some("anna@example.com")), "name kept, email added");
        assert_eq!(list[1].email.as_deref(), Some("other@example.com"), "never replaced");
        assert_eq!(list[2].email, None, "a person without email adds nothing");
        assert_eq!(list[3].email, None);
    }

    #[test]
    fn linking_on_save_only_touches_new_participants() {
        let people = registry(&[person("Anna Rossi", Some("anna@example.com"), &["Annie"])]);
        let old = vec![part("Anna Rossi", None)];
        // Anna was already there without email (the user removed it): kept.
        // A renamed voice that now matches is new: linked.
        let mut meta = ItemMeta {
            item_type: ItemType::Meeting,
            participants: vec![part("anna rossì", None), part("Annie", None)],
            ..Default::default()
        };
        assert_eq!(link_new_participants(&old, &mut meta, &people), 1);
        assert_eq!(meta.participants[0].email, None);
        assert_eq!(meta.participants[1].email.as_deref(), Some("anna@example.com"));

        let mut note = ItemMeta {
            item_type: ItemType::Note,
            participants: vec![part("Anna Rossi", None)],
            ..Default::default()
        };
        assert_eq!(link_new_participants(&[], &mut note, &people), 0, "P10: notes untouched");
    }

    #[test]
    fn link_on_save_writes_the_email_to_the_frontmatter() {
        let tmp = tempfile::tempdir().unwrap();
        let archive = tmp.path().join("Sussurro");
        let meta = ItemMeta {
            item_type: ItemType::Transcription,
            title: "Intervista".into(),
            date: "2026-09-24T10:00:00+02:00".into(),
            ..Default::default()
        };
        let id = create_item(&archive, &meta, &SegmentsFile::default()).unwrap();
        modify(&archive, |p| add_person(p, &person("Anna Rossi", Some("anna@example.com"), &["Annie"])))
            .unwrap();

        let mut next = read_item(&archive, &id).unwrap().meta;
        next.participants = vec![part("Annie", None), part("Ospite", None)];
        assert_eq!(link_on_save(&archive, &id, &mut next), 1);
        update_meta(&archive, &id, &next).unwrap();
        let saved = read_item(&archive, &id).unwrap().meta.participants;
        assert_eq!(saved, vec![part("Annie", Some("anna@example.com")), part("Ospite", None)]);
        let text = std::fs::read_to_string(
            crate::archive::paths::item_dir(&archive, &id).unwrap().join("transcript.md"),
        )
        .unwrap();
        assert!(text.contains("anna@example.com"), "{text}");
    }

    #[test]
    fn speakers_link_by_label_without_overriding() {
        let people = registry(&[person("Anna Rossi", None, &["Anna R."])]);
        let mut speakers = vec![
            DocSpeaker { id: "meet:Anna R.".into(), label: "anna r.".into(), ..Default::default() },
            DocSpeaker { id: "voice:2".into(), label: "Voice 2".into(), ..Default::default() },
            DocSpeaker {
                id: "meet:Anna".into(),
                label: "Anna Rossi".into(),
                person_id: Some("p-kept".into()),
                ..Default::default()
            },
        ];
        assert_eq!(link_speakers(&mut speakers, &people), 1);
        assert_eq!(speakers[0].person_id.as_deref(), Some(people[0].id.as_str()));
        assert_eq!(speakers[1].person_id, None);
        assert_eq!(speakers[2].person_id.as_deref(), Some("p-kept"));
    }

    #[test]
    fn clean_trims_and_dedups_and_checks_email() {
        let p = clean(&person("  Anna   Rossi ", Some("  "), &["Annie", " annie ", "", "ANNA ROSSI", "Anna R."]))
            .unwrap();
        assert_eq!(p.name, "Anna Rossi");
        assert_eq!(p.email, None);
        assert_eq!(p.aliases, vec!["Annie", "Anna R."]);
        assert!(clean(&person("  ", None, &[])).is_err());
        assert!(clean(&person("Anna", Some("anna at example"), &[])).is_err());
        assert!(is_valid_email("anna.rossi+x@mail.example.it"));
        for bad in ["anna@example", "@example.com", "a b@example.com", "a@b@c.com", "a@.com", "a@b."] {
            assert!(!is_valid_email(bad), "{bad}");
        }
    }

    #[test]
    fn add_and_update_refuse_duplicates() {
        let mut people = registry(&[person("Anna Rossi", Some("anna@example.com"), &[])]);
        assert!(add_person(&mut people, &person("anna ROSSÌ", None, &[])).is_err(), "same name");
        assert!(add_person(&mut people, &person("A. Rossi", Some("ANNA@example.com"), &[])).is_err(), "same email");
        let bruno = add_person(&mut people, &person("Bruno", None, &["Anna Rossi"])).unwrap();
        assert!(bruno.id.starts_with("p-") && bruno.id != people[0].id);

        let mut edit = bruno.clone();
        edit.email = Some("anna@example.com".into());
        assert!(update_person(&mut people, &edit).is_err());
        edit.email = Some("bruno@example.com".into());
        edit.name = "Bruno Neri".into();
        assert_eq!(update_person(&mut people, &edit).unwrap().name, "Bruno Neri");
        edit.id = "p-missing".into();
        assert!(update_person(&mut people, &edit).is_err());
    }

    #[test]
    fn merge_folds_names_into_aliases_and_keeps_an_email() {
        let mut people = registry(&[
            person("Anna Rossi", None, &["Annie"]),
            person("Anna R.", Some("anna@example.com"), &["Annie", "A.R."]),
            person("Bruno", Some("bruno@example.com"), &[]),
        ]);
        let (into, from) = (people[0].id.clone(), people[1].id.clone());
        let merged = merge_people(&mut people, &into, &[from.clone(), into.clone()]).unwrap();
        assert_eq!(merged.id, into);
        assert_eq!(merged.name, "Anna Rossi");
        assert_eq!(merged.email.as_deref(), Some("anna@example.com"));
        assert_eq!(merged.aliases, vec!["Annie", "Anna R.", "A.R."]);
        assert_eq!(people.len(), 2);
        assert!(!people.iter().any(|p| p.id == from));
        assert_eq!(people.iter().find(|p| p.id == into), Some(&merged));
        // The target's own email wins over a merged one.
        let bruno = people[1].id.clone();
        let m = merge_people(&mut people, &bruno, std::slice::from_ref(&into)).unwrap();
        assert_eq!(m.email.as_deref(), Some("bruno@example.com"));
        assert!(merge_people(&mut people, &bruno, &[]).is_err());
        assert!(merge_people(&mut people, &bruno, &["p-missing".into()]).is_err());
    }

    #[test]
    fn deleting_a_person_never_edits_items() {
        let tmp = tempfile::tempdir().unwrap();
        let archive = tmp.path().join("Sussurro");
        let anna = modify(&archive, |p| add_person(p, &person("Anna Rossi", Some("anna@example.com"), &[])))
            .unwrap();
        let meta = ItemMeta {
            item_type: ItemType::Meeting,
            title: "Sync".into(),
            date: "2026-09-24T10:00:00+02:00".into(),
            participants: vec![part("Anna Rossi", Some("anna@example.com"))],
            ..Default::default()
        };
        let id = create_item(&archive, &meta, &SegmentsFile::default()).unwrap();
        let dir = crate::archive::paths::item_dir(&archive, &id).unwrap();
        let before = std::fs::read(dir.join("transcript.md")).unwrap();

        modify(&archive, |p| delete_person(p, &anna.id)).unwrap();
        assert!(read_people(&archive).is_empty());
        assert_eq!(std::fs::read(dir.join("transcript.md")).unwrap(), before);
        assert!(modify(&archive, |p| delete_person(p, &anna.id)).is_err());

        // Only future linking stops.
        let mut next = read_item(&archive, &id).unwrap().meta;
        next.participants.push(part("anna rossi 2", None));
        assert_eq!(link_on_save(&archive, &id, &mut next), 0);
    }

    #[test]
    fn registry_roundtrips_through_the_archive() {
        let tmp = tempfile::tempdir().unwrap();
        let archive = tmp.path().join("Sussurro");
        assert!(read_people(&archive).is_empty());
        assert!(list_people(&archive).unwrap().is_empty());

        let z = modify(&archive, |p| add_person(p, &person("Zoë", Some("zoe@example.com"), &["Zo"]))).unwrap();
        let a = modify(&archive, |p| add_person(p, &person("anna", None, &[]))).unwrap();
        let path = people_path(&archive);
        assert_eq!(path, archive.join(".sussurro").join("people.json"));
        let json: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(json["version"], 1);
        assert_eq!(json["people"][0]["email"], "zoe@example.com");
        assert!(json["people"][1].get("email").is_none(), "no email = no key");

        assert_eq!(read_people(&archive), vec![z.clone(), a.clone()]);
        assert_eq!(list_people(&archive).unwrap(), vec![a, z], "listed by name");
        // No temp files left behind; the archive scan doesn't see the folder.
        let leftovers: Vec<_> = std::fs::read_dir(archive.join(".sussurro"))
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().contains(".tmp-"))
            .collect();
        assert!(leftovers.is_empty());
        assert!(crate::archive::list_items(&archive).is_empty());
    }

    #[test]
    fn hand_edited_files_are_read_tolerantly() {
        let text = r#"{ "version": 1, "people": [
            { "id": "p-1", "name": "Anna Rossi", "email": "anna@example.com", "aliases": "Annie" },
            { "name": "  Bruno  ", "email": "not an email", "aliases": ["Bru", 7, null] },
            "Carla",
            { "id": "p-1", "name": "Dario" },
            { "email": "nameless@example.com" },
            42,
            { "name": "Eva", "extra": { "future": true } }
        ] }"#;
        let people = parse_people(text).unwrap();
        let names: Vec<&str> = people.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, vec!["Anna Rossi", "Bruno", "Carla", "Dario", "Eva"]);
        assert_eq!(people[0].aliases, vec!["Annie"]);
        assert_eq!(people[1].email, None, "a bad email is dropped, the person kept");
        assert_eq!(people[1].aliases, vec!["Bru", "7"]);
        assert_eq!(people[3].id, "p-1-2", "repeated id made unique");
        assert_eq!(people[2].id, derived_id("Carla"), "missing id derived");
        assert_eq!(parse_people(text).unwrap()[2].id, people[2].id, "stable");
        // A bare list works; a missing list is empty.
        assert_eq!(parse_people(r#"["Anna"]"#).unwrap().len(), 1);
        assert!(parse_people(r#"{ "version": 1 }"#).unwrap().is_empty());
        assert!(parse_people("{ nope").is_err());
        assert!(parse_people("\"Anna\"").is_err());
    }

    #[test]
    fn a_broken_file_reads_empty_but_is_never_overwritten() {
        let tmp = tempfile::tempdir().unwrap();
        let archive = tmp.path().to_path_buf();
        let path = people_path(&archive);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "{ \"people\": [ { \"name\": \"Anna\" ").unwrap();
        assert!(read_people(&archive).is_empty(), "tolerant for linking");
        assert!(list_people(&archive).is_err(), "the screen says so");
        let err = modify(&archive, |p| add_person(p, &person("Bruno", None, &[]))).unwrap_err();
        assert!(format!("{err:#}").contains("won't overwrite"), "{err:#}");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{ \"people\": [ { \"name\": \"Anna\" ");
    }

    #[test]
    fn a_failed_edit_writes_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let archive = tmp.path().to_path_buf();
        assert!(modify(&archive, |p| add_person(p, &person("", None, &[]))).is_err());
        assert!(!people_path(&archive).exists());
    }

    #[test]
    fn usage_counts_distinct_items_by_email_or_name() {
        let people = registry(&[
            person("Anna Rossi", Some("anna@example.com"), &["Annie"]),
            person("Bruno", None, &[]),
            person("Carla", None, &[]),
        ]);
        let row = |id: &str, name: &str, email: &str| (id.to_string(), name.to_string(), email.to_string());
        let rows = vec![
            row("a", "Anna Rossi", "anna@example.com"),
            row("a", "Annie", ""),
            row("b", "Someone Else", "ANNA@example.com"),
            row("c", "annie", ""),
            row("c", "BRUNO", ""),
            row("d", "Voice 1", ""),
        ];
        let n = usage(&people, &rows);
        assert_eq!(n[&people[0].id], 3);
        assert_eq!(n[&people[1].id], 1);
        assert_eq!(n[&people[2].id], 0);
    }

    #[test]
    fn import_skips_known_people_and_fresh_ids_collisions() {
        let mut people = registry(&[person("Anna Rossi", Some("anna@example.com"), &[])]);
        let taken = people[0].id.clone();
        let incoming = vec![
            Person { id: taken.clone(), ..person("Bruno", None, &[]) },
            person("anna rossi", None, &[]),
            person("A. R.", Some("anna@example.com"), &[]),
            person("", None, &[]),
            Person { id: "p-carla".into(), ..person("Carla", Some("carla@example.com"), &[]) },
        ];
        assert_eq!(import_people(&mut people, &incoming), 2);
        assert_eq!(people.len(), 3);
        assert_ne!(people[1].id, taken);
        assert_eq!(people[2].id, "p-carla");
    }
}
