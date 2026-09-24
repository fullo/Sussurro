//! Companion documents (0.8, #120): markdown files a recipe writes next to
//! `transcript.md` — `document.md` for the formatted document,
//! `<recipe-slug>.md` for the others.
//!
//! ```text
//! <item>/
//!   transcript.md
//!   document.md                 recipe output, provenance in its frontmatter
//!   action-items.md
//!   .sussurro/companions.json   SHA-256 of each companion the app last wrote
//! ```
//!
//! The same content-hash rule as the transcript (plan §4.4): the app
//! replaces a companion only while it is byte-identical to what the app
//! last wrote *and* it is the output of the same recipe (a saved Ask answer
//! that happens to share the name is not). A companion edited by the user
//! (or of unknown provenance) is never overwritten — the new output goes to
//! the first free `<stem>-2.md`, `<stem>-3.md`… instead. Saved answers
//! ([`write_companion_new`], #121) never replace anything.

use super::frontmatter;
use super::store::{existing_item_dir, lock_items, sha256_hex, write_atomic, META_DIR, TRANSCRIPT_FILE};
use super::types::SessionState;
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

/// File of the *Formatted document* recipe.
pub const DOCUMENT_FILE: &str = "document.md";
/// Hashes of the companions the app wrote, inside the item's `.sussurro/`.
pub const COMPANIONS_STATE_FILE: &str = "companions.json";
/// Collision suffixes tried before giving up (`-2` … `-99`).
const MAX_VARIANTS: u32 = 99;

/// Frontmatter of a companion document. Everything the app writes is
/// provenance: which recipe, on which profile and model, from which file.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct CompanionMeta {
    #[serde(default)]
    pub title: String,
    /// `<recipe> / <profile> / <model>` (plan §4.5).
    #[serde(default)]
    pub generated_by: String,
    /// Recipe id (`formatted-document`, a user recipe's id…).
    #[serde(default)]
    pub recipe: String,
    /// Profile name as shown in the app when the document was generated.
    #[serde(default)]
    pub profile: String,
    #[serde(default)]
    pub model: String,
    /// The transcript went to a profile marked external (#122 marks these
    /// in the Library).
    #[serde(default)]
    pub external: bool,
    /// RFC 3339.
    #[serde(default)]
    pub date: String,
    /// The transcript the document was generated from (relative link).
    #[serde(default)]
    pub transcript: String,
    /// Keys the user (or another tool) added; kept as read.
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

/// A companion document as returned to the UI.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct CompanionDoc {
    /// File name inside the item folder (`document.md`).
    pub file: String,
    pub meta: CompanionMeta,
    /// Markdown after the frontmatter.
    pub body: String,
    /// Changed since the app wrote it (or not written by the app at all):
    /// a regeneration will not overwrite it.
    pub edited_externally: bool,
}

/// Whether `name` can be a companion document of an item: a plain `.md`
/// file name — no separators, no dot-file, not the transcript. Names come
/// back from the UI, so this is the confinement check.
pub fn validate_companion_name(name: &str) -> Result<()> {
    let bad = |why: &str| -> Result<()> { bail!("invalid document name '{name}': {why}") };
    if name.is_empty() || name.len() > 255 {
        return bad("empty or too long");
    }
    if name.contains(['/', '\\', ':', '\0']) {
        return bad("forbidden character");
    }
    if name.starts_with('.') {
        return bad("hidden file");
    }
    if !name.to_ascii_lowercase().ends_with(".md") || name.len() <= 3 {
        return bad("not a .md file");
    }
    if name.eq_ignore_ascii_case(TRANSCRIPT_FILE) {
        return bad("the transcript is not a companion document");
    }
    Ok(())
}

/// `<stem>.md`, then `<stem>-2.md`, `<stem>-3.md`… (`n` = 1 is the base).
fn variant(base: &str, n: u32) -> String {
    if n <= 1 {
        return base.to_string();
    }
    let stem = base.strip_suffix(".md").unwrap_or(base);
    format!("{stem}-{n}.md")
}

fn state_path(dir: &Path) -> std::path::PathBuf {
    dir.join(META_DIR).join(COMPANIONS_STATE_FILE)
}

fn read_hashes(dir: &Path) -> BTreeMap<String, String> {
    std::fs::read_to_string(state_path(dir))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn write_hashes(dir: &Path, hashes: &BTreeMap<String, String>) -> Result<()> {
    std::fs::create_dir_all(dir.join(META_DIR))?;
    write_atomic(&state_path(dir), serde_json::to_string_pretty(hashes)?.as_bytes())
}

/// A regular file (symlinks are never followed out of the item folder).
fn is_plain_file(path: &Path) -> bool {
    std::fs::symlink_metadata(path)
        .map(|m| m.file_type().is_file())
        .unwrap_or(false)
}

/// Full file for `meta` + `body`: frontmatter, the body, and a closing link
/// back to the transcript.
pub fn render_companion(meta: &CompanionMeta, body: &str) -> Result<String> {
    let yaml = serde_saphyr::to_string(meta).context("serializing document frontmatter")?;
    let yaml = if yaml.ends_with('\n') { yaml } else { format!("{yaml}\n") };
    let mut out = format!("---\n{yaml}---\n\n{}\n", body.trim());
    if !meta.transcript.is_empty() {
        out.push_str(&format!(
            "\n---\n\n*Generated from [the transcript]({}) by {}.*\n",
            meta.transcript, meta.generated_by
        ));
    }
    Ok(out)
}

/// Parse a companion file leniently: unreadable YAML still shows the body
/// (with empty provenance) rather than hiding the user's document.
pub fn parse_companion(doc: &str) -> (CompanionMeta, String) {
    match frontmatter::split(doc) {
        Some((yaml, body)) => {
            let map: BTreeMap<String, serde_json::Value> = if yaml.trim().is_empty() {
                BTreeMap::new()
            } else {
                serde_saphyr::from_str(yaml).unwrap_or_default()
            };
            (meta_from_map(map), body.to_string())
        }
        None => (CompanionMeta::default(), doc.to_string()),
    }
}

fn meta_from_map(mut map: BTreeMap<String, serde_json::Value>) -> CompanionMeta {
    let mut take = |k: &str| -> String {
        match map.remove(k) {
            Some(serde_json::Value::String(s)) => s,
            Some(serde_json::Value::Null) | None => String::new(),
            Some(serde_json::Value::Bool(b)) => b.to_string(),
            Some(serde_json::Value::Number(n)) => n.to_string(),
            Some(other) => other.to_string(),
        }
    };
    let title = take("title");
    let generated_by = take("generated_by");
    let recipe = take("recipe");
    let profile = take("profile");
    let model = take("model");
    let date = take("date");
    let transcript = take("transcript");
    let external = match map.remove("external") {
        Some(serde_json::Value::Bool(b)) => b,
        Some(serde_json::Value::String(s)) => s.trim().eq_ignore_ascii_case("true"),
        _ => false,
    };
    CompanionMeta {
        title,
        generated_by,
        recipe,
        profile,
        model,
        external,
        date,
        transcript,
        extra: map,
    }
}

/// Write a companion document for item `id`, preferring the file name
/// `base` (see the module docs for when a `-N` variant is used instead).
/// Refused for an item a capture session is still writing. Returns the file
/// name written.
pub fn write_companion(
    archive: &Path,
    id: &str,
    base: &str,
    meta: &CompanionMeta,
    body: &str,
) -> Result<String> {
    write_with(archive, id, base, meta, body, true)
}

/// Frontmatter key that marks a companion as a saved Ask answer (#121).
pub const KIND_KEY: &str = "kind";
/// [`KIND_KEY`] value of a saved answer.
pub const KIND_ANSWER: &str = "answer";

/// [`write_companion`] that never replaces an existing file, not even the
/// app's own output: the document goes to the first free name. For saved
/// Ask answers (#121) — two answers saved under one name are two files.
pub fn write_companion_new(
    archive: &Path,
    id: &str,
    base: &str,
    meta: &CompanionMeta,
    body: &str,
) -> Result<String> {
    write_with(archive, id, base, meta, body, false)
}

/// Whether an existing app-written companion (`current`) is an earlier
/// output of what `meta` describes: same recipe and same kind, so a
/// regeneration replaces its own document but never a saved answer that
/// happens to share the name.
fn same_output(current: &[u8], meta: &CompanionMeta) -> bool {
    let (old, _) = parse_companion(&String::from_utf8_lossy(current));
    old.recipe == meta.recipe && old.extra.get(KIND_KEY) == meta.extra.get(KIND_KEY)
}

fn write_with(
    archive: &Path,
    id: &str,
    base: &str,
    meta: &CompanionMeta,
    body: &str,
    replace_own: bool,
) -> Result<String> {
    validate_companion_name(base)?;
    let doc = render_companion(meta, body)?;
    let _lock = lock_items();
    let dir = existing_item_dir(archive, id)?;
    let transcript = std::fs::read(dir.join(TRANSCRIPT_FILE)).unwrap_or_default();
    if let Ok((m, _)) = frontmatter::parse(&String::from_utf8_lossy(&transcript)) {
        if m.session_state() == Some(SessionState::Recording) {
            bail!("'{id}' is still being recorded — run recipes when the session ends");
        }
    }
    let mut hashes = read_hashes(&dir);
    let mut chosen = None;
    for n in 1..=MAX_VARIANTS {
        let name = variant(base, n);
        let path = dir.join(&name);
        match std::fs::symlink_metadata(&path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                chosen = Some(name);
                break;
            }
            Err(e) => return Err(e).with_context(|| format!("checking {}", path.display())),
            Ok(m) if m.file_type().is_file() => {
                let current = std::fs::read(&path)
                    .with_context(|| format!("reading {}", path.display()))?;
                let ours = hashes
                    .get(&name)
                    .is_some_and(|h| *h == sha256_hex(&current));
                if replace_own && ours && same_output(&current, meta) {
                    chosen = Some(name);
                    break;
                }
            }
            // A folder or a symlink by that name: not ours, try the next.
            Ok(_) => {}
        }
    }
    let Some(name) = chosen else {
        bail!("too many edited copies of {base} in '{id}'");
    };
    write_atomic(&dir.join(&name), doc.as_bytes())?;
    hashes.insert(name.clone(), sha256_hex(doc.as_bytes()));
    // Forget files that are gone (deleted by hand).
    hashes.retain(|f, _| is_plain_file(&dir.join(f)));
    write_hashes(&dir, &hashes)?;
    Ok(name)
}

/// Every companion document of item `id`: `document.md` first, then by
/// name. Files that can't be read are skipped.
pub fn list_companions(archive: &Path, id: &str) -> Result<Vec<CompanionDoc>> {
    let dir = existing_item_dir(archive, id)?;
    let hashes = read_hashes(&dir);
    let mut out = Vec::new();
    let entries = std::fs::read_dir(&dir).with_context(|| format!("reading {}", dir.display()))?;
    for entry in entries.flatten() {
        let Some(name) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        if validate_companion_name(&name).is_err() || !is_plain_file(&entry.path()) {
            continue;
        }
        match read_at(&dir, &name, &hashes) {
            Ok(doc) => out.push(doc),
            Err(e) => eprintln!("archive: skipping document {name} of {id}: {e:#}"),
        }
    }
    out.sort_by(|a, b| {
        (a.file != DOCUMENT_FILE)
            .cmp(&(b.file != DOCUMENT_FILE))
            .then_with(|| a.file.cmp(&b.file))
    });
    Ok(out)
}

/// One companion document of item `id`.
pub fn read_companion(archive: &Path, id: &str, file: &str) -> Result<CompanionDoc> {
    validate_companion_name(file)?;
    let dir = existing_item_dir(archive, id)?;
    if !is_plain_file(&dir.join(file)) {
        bail!("no document '{file}' in '{id}'");
    }
    read_at(&dir, file, &read_hashes(&dir))
}

/// Path of a companion document (validated, confined, existing).
pub fn companion_path(archive: &Path, id: &str, file: &str) -> Result<std::path::PathBuf> {
    validate_companion_name(file)?;
    let path = existing_item_dir(archive, id)?.join(file);
    if !is_plain_file(&path) {
        bail!("no document '{file}' in '{id}'");
    }
    Ok(path)
}

fn read_at(dir: &Path, name: &str, hashes: &BTreeMap<String, String>) -> Result<CompanionDoc> {
    let path = dir.join(name);
    let bytes = std::fs::read(&path).with_context(|| format!("reading {}", path.display()))?;
    let (meta, body) = parse_companion(&String::from_utf8_lossy(&bytes));
    let edited = hashes.get(name).is_none_or(|h| *h != sha256_hex(&bytes));
    Ok(CompanionDoc {
        file: name.to_string(),
        meta,
        body,
        edited_externally: edited,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::archive::store::create_item;
    use crate::archive::types::{ItemMeta, SegmentsFile};

    fn item(archive: &Path) -> String {
        let meta = ItemMeta {
            title: "Idee".into(),
            date: "2026-09-24T10:00:00+02:00".into(),
            ..Default::default()
        };
        create_item(archive, &meta, &SegmentsFile::default()).unwrap()
    }

    fn meta() -> CompanionMeta {
        CompanionMeta {
            title: "Idee — Formatted document".into(),
            generated_by: "Formatted document / Local / llama3.2:3b".into(),
            recipe: "formatted-document".into(),
            profile: "Local".into(),
            model: "llama3.2:3b".into(),
            external: false,
            date: "2026-09-24T11:00:00+02:00".into(),
            transcript: TRANSCRIPT_FILE.into(),
            extra: BTreeMap::new(),
        }
    }

    #[test]
    fn names_are_confined_to_plain_md_files() {
        assert!(validate_companion_name("document.md").is_ok());
        assert!(validate_companion_name("action-items.MD").is_ok());
        for bad in [
            "", ".md", "transcript.md", "Transcript.md", ".hidden.md", "../x.md", "a/b.md",
            "a\\b.md", "c:x.md", "notes.txt", "x\0.md",
        ] {
            assert!(validate_companion_name(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn provenance_frontmatter_and_transcript_link() {
        let doc = render_companion(&meta(), "**tl;dr:** ok\n\n# Idee\n").unwrap();
        assert!(doc.starts_with("---\n"), "{doc}");
        assert!(
            doc.contains("generated_by: Formatted document / Local / llama3.2:3b\n"),
            "{doc}"
        );
        assert!(doc.contains("[the transcript](transcript.md)"), "{doc}");
        let (back, body) = parse_companion(&doc);
        assert_eq!(back, meta());
        assert!(body.contains("# Idee"));
    }

    #[test]
    fn parse_is_lenient() {
        let (m, body) = parse_companion("---\ntitle: 2026\nexternal: \"true\"\naliases: [x]\n---\nhi");
        assert_eq!(m.title, "2026");
        assert!(m.external);
        assert_eq!(m.extra["aliases"], serde_json::json!(["x"]));
        assert_eq!(body, "hi");
        // Broken YAML: empty provenance, the body is still shown.
        let (m, body) = parse_companion("---\ntitle: [oops\n---\nbody");
        assert_eq!(m, CompanionMeta::default());
        assert_eq!(body, "body");
        let (_, body) = parse_companion("# no frontmatter");
        assert_eq!(body, "# no frontmatter");
    }

    #[test]
    fn regenerate_replaces_own_output_but_never_a_user_edit() {
        let tmp = tempfile::tempdir().unwrap();
        let archive = tmp.path().join("Sussurro");
        let id = item(&archive);
        let dir = archive.join(&id);

        assert_eq!(write_companion(&archive, &id, DOCUMENT_FILE, &meta(), "v1").unwrap(), "document.md");
        // Regenerating replaces the app's own output in place.
        assert_eq!(write_companion(&archive, &id, DOCUMENT_FILE, &meta(), "v2").unwrap(), "document.md");
        let docs = list_companions(&archive, &id).unwrap();
        assert_eq!(docs.len(), 1);
        assert!(docs[0].body.contains("v2") && !docs[0].edited_externally);

        // The user edits document.md: it is kept, the new output goes next to it.
        std::fs::write(dir.join("document.md"), "my own notes").unwrap();
        assert_eq!(write_companion(&archive, &id, DOCUMENT_FILE, &meta(), "v3").unwrap(), "document-2.md");
        assert_eq!(std::fs::read_to_string(dir.join("document.md")).unwrap(), "my own notes");
        // …and the next regeneration replaces document-2.md, the app's copy.
        assert_eq!(write_companion(&archive, &id, DOCUMENT_FILE, &meta(), "v4").unwrap(), "document-2.md");

        let docs = list_companions(&archive, &id).unwrap();
        let files: Vec<_> = docs.iter().map(|d| d.file.as_str()).collect();
        assert_eq!(files, ["document.md", "document-2.md"]);
        assert!(docs[0].edited_externally);
        assert!(!docs[1].edited_externally && docs[1].body.contains("v4"));
    }

    #[test]
    fn a_regeneration_only_replaces_its_own_recipe_and_never_a_saved_answer() {
        let tmp = tempfile::tempdir().unwrap();
        let archive = tmp.path().join("Sussurro");
        let id = item(&archive);
        let mut answer = meta();
        answer.recipe = "question".into();
        answer.extra.insert(KIND_KEY.into(), KIND_ANSWER.into());
        // A saved answer that took the name summary.md…
        assert_eq!(write_companion_new(&archive, &id, "summary.md", &answer, "a").unwrap(), "summary.md");
        // …is not replaced by the Summary recipe, which writes next to it…
        let mut summary = meta();
        summary.recipe = "summary".into();
        assert_eq!(write_companion(&archive, &id, "summary.md", &summary, "s1").unwrap(), "summary-2.md");
        // …and then regenerates its own file in place.
        assert_eq!(write_companion(&archive, &id, "summary.md", &summary, "s2").unwrap(), "summary-2.md");
        let saved = std::fs::read_to_string(archive.join(&id).join("summary.md")).unwrap();
        let (m, body) = parse_companion(&saved);
        assert_eq!(m.extra[KIND_KEY], "answer");
        assert_eq!(body.trim_start().lines().next(), Some("a"));
    }

    #[test]
    fn saved_answers_never_replace_anything() {
        let tmp = tempfile::tempdir().unwrap();
        let archive = tmp.path().join("Sussurro");
        let id = item(&archive);
        let mut answer = meta();
        answer.extra.insert(KIND_KEY.into(), KIND_ANSWER.into());
        let names: Vec<_> = (0..3)
            .map(|i| write_companion_new(&archive, &id, "who.md", &answer, &format!("a{i}")).unwrap())
            .collect();
        assert_eq!(names, ["who.md", "who-2.md", "who-3.md"]);
        let docs = list_companions(&archive, &id).unwrap();
        assert!(docs.iter().all(|d| !d.edited_externally), "saved answers are the app's own files");
    }

    #[test]
    fn a_hand_made_file_of_unknown_provenance_is_never_overwritten() {
        let tmp = tempfile::tempdir().unwrap();
        let archive = tmp.path().join("Sussurro");
        let id = item(&archive);
        std::fs::write(archive.join(&id).join("summary.md"), "mine").unwrap();
        assert_eq!(write_companion(&archive, &id, "summary.md", &meta(), "x").unwrap(), "summary-2.md");
        assert_eq!(std::fs::read_to_string(archive.join(&id).join("summary.md")).unwrap(), "mine");
    }

    #[test]
    fn listing_skips_the_transcript_hidden_files_and_other_types() {
        let tmp = tempfile::tempdir().unwrap();
        let archive = tmp.path().join("Sussurro");
        let id = item(&archive);
        let dir = archive.join(&id);
        std::fs::write(dir.join("notes.txt"), "x").unwrap();
        std::fs::write(dir.join(".draft.md"), "x").unwrap();
        std::fs::create_dir(dir.join("folder.md")).unwrap();
        write_companion(&archive, &id, "summary.md", &meta(), "s").unwrap();
        write_companion(&archive, &id, DOCUMENT_FILE, &meta(), "d").unwrap();
        let files: Vec<_> = list_companions(&archive, &id)
            .unwrap()
            .into_iter()
            .map(|d| d.file)
            .collect();
        assert_eq!(files, ["document.md", "summary.md"]);
        assert!(read_companion(&archive, &id, "transcript.md").is_err());
        assert!(read_companion(&archive, &id, "../x.md").is_err());
        assert!(read_companion(&archive, &id, "missing.md").is_err());
        assert_eq!(read_companion(&archive, &id, "summary.md").unwrap().meta.recipe, "formatted-document");
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_documents_are_ignored() {
        let tmp = tempfile::tempdir().unwrap();
        let archive = tmp.path().join("Sussurro");
        let id = item(&archive);
        let outside = tmp.path().join("secret.md");
        std::fs::write(&outside, "secret").unwrap();
        std::os::unix::fs::symlink(&outside, archive.join(&id).join("link.md")).unwrap();
        assert!(list_companions(&archive, &id).unwrap().is_empty());
        assert!(read_companion(&archive, &id, "link.md").is_err());
        // Writing never follows it either: the output goes to a fresh name.
        std::os::unix::fs::symlink(&outside, archive.join(&id).join("document.md")).unwrap();
        assert_eq!(write_companion(&archive, &id, DOCUMENT_FILE, &meta(), "x").unwrap(), "document-2.md");
        assert_eq!(std::fs::read_to_string(&outside).unwrap(), "secret");
    }

    #[test]
    fn refused_while_the_item_is_recording() {
        let tmp = tempfile::tempdir().unwrap();
        let archive = tmp.path().join("Sussurro");
        let mut m = ItemMeta {
            title: "Live".into(),
            date: "2026-09-24T10:00:00+02:00".into(),
            ..Default::default()
        };
        m.set_session_state(Some(SessionState::Recording));
        let id = create_item(&archive, &m, &SegmentsFile::default()).unwrap();
        let err = write_companion(&archive, &id, DOCUMENT_FILE, &meta(), "x").unwrap_err();
        assert!(format!("{err:#}").contains("still being recorded"));
    }
}
