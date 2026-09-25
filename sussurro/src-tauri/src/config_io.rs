use crate::archive::people::Person;
use crate::settings::{AppStyle, Settings, Snippet};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Portable subset of the settings — the parts worth carrying between machines
/// (dictionary, voice snippets, per-app styles). Deliberately excludes
/// machine-specific fields like models_dir, the hotkey, and the input device.
///
/// `people` (the archive's People registry, #132) holds other people's
/// emails: it is empty — and absent from the file — unless the user ticked
/// "include People" for that export.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct ConfigBundle {
    #[serde(default)]
    pub dictionary: Vec<String>,
    #[serde(default)]
    pub snippets: Vec<Snippet>,
    #[serde(default)]
    pub app_styles: Vec<AppStyle>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub people: Vec<Person>,
}

impl ConfigBundle {
    pub fn from_settings(s: &Settings) -> Self {
        Self {
            dictionary: s.dictionary.clone(),
            snippets: s.snippets.clone(),
            app_styles: s.app_styles.clone(),
            people: Vec::new(),
        }
    }

    /// Merge into settings: union the dictionary (case-insensitive, order
    /// preserved), and append snippets/styles that aren't already present.
    /// Returns (words_added, snippets_added, styles_added).
    pub fn merge_into(&self, s: &mut Settings) -> (usize, usize, usize) {
        let mut words = 0;
        let existing: std::collections::HashSet<String> =
            s.dictionary.iter().map(|w| w.to_lowercase()).collect();
        let mut seen = existing.clone();
        for w in &self.dictionary {
            if seen.insert(w.to_lowercase()) {
                s.dictionary.push(w.clone());
                words += 1;
            }
        }

        let mut snippets = 0;
        for sn in &self.snippets {
            if !s.snippets.iter().any(|e| e.cue == sn.cue) {
                s.snippets.push(sn.clone());
                snippets += 1;
            }
        }

        let mut styles = 0;
        for st in &self.app_styles {
            if !s.app_styles.iter().any(|e| e.app_match == st.app_match) {
                s.app_styles.push(st.clone());
                styles += 1;
            }
        }
        (words, snippets, styles)
    }
}

/// Write the bundle; `people` is included only when the user opted in (the
/// caller passes an empty slice otherwise).
pub fn export_to(path: &Path, settings: &Settings, people: &[Person]) -> std::io::Result<()> {
    let bundle = ConfigBundle {
        people: people.to_vec(),
        ..ConfigBundle::from_settings(settings)
    };
    std::fs::write(
        path,
        serde_json::to_string_pretty(&bundle).expect("bundle serialize"),
    )
}

pub fn load_bundle(path: &Path) -> anyhow::Result<ConfigBundle> {
    let text = std::fs::read_to_string(path)?;
    Ok(serde_json::from_str(&text)?)
}

/// Largest dictionary/snippet file `read_import_text` accepts. Real lists are
/// a few KB; the cap stops a mis-picked file from being shipped to the webview.
pub const MAX_IMPORT_BYTES: u64 = 1024 * 1024;

/// Which list a bulk import feeds. It only selects the picker's filter and the
/// one extension accepted: the file itself is always chosen by the user in a
/// native dialog opened from Rust (#156), never named by the webview.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ImportKind {
    /// Personal dictionary: `.txt`, one word or phrase per line.
    Dictionary,
    /// Voice snippets: `.csv`, one `cue,text` per line.
    Snippets,
}

impl ImportKind {
    pub fn extension(self) -> &'static str {
        match self {
            ImportKind::Dictionary => "txt",
            ImportKind::Snippets => "csv",
        }
    }

    pub fn dialog_title(self) -> &'static str {
        match self {
            ImportKind::Dictionary => "Import dictionary",
            ImportKind::Snippets => "Import snippets",
        }
    }

    pub fn export_title(self) -> &'static str {
        match self {
            ImportKind::Dictionary => "Export dictionary",
            ImportKind::Snippets => "Export snippets",
        }
    }

    pub fn export_file_name(self) -> &'static str {
        match self {
            ImportKind::Dictionary => "sussurro-dictionary.txt",
            ImportKind::Snippets => "sussurro-snippets.csv",
        }
    }

    pub fn filter_name(self) -> &'static str {
        match self {
            ImportKind::Dictionary => "Text files",
            ImportKind::Snippets => "CSV files",
        }
    }
}

/// What the import picker hands the webview: the picked file's name (no
/// directory) and its text. The full path never crosses IPC.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ImportFile {
    pub name: String,
    pub contents: String,
}

/// Read a user-picked dictionary (.txt) or snippet (.csv) file as UTF-8 text.
/// Deliberately narrow so it can't become a generic file reader: only the
/// extension `kind` expects, a regular file that is not a symlink (a link
/// named `*.txt` could point anywhere), at most `MAX_IMPORT_BYTES`, valid
/// UTF-8 (a leading BOM is stripped). Parsing and merging happen in the
/// frontend (`src/utils.ts`).
///
/// Symlinks: the `lstat` check refuses a link up front, and on Unix the file
/// is opened with `O_NOFOLLOW` (#158), so a link swapped in between the
/// check and the open is refused too instead of being followed. Windows has
/// no such flag on this path: `File::open` follows a link swapped in during
/// that window (the regular-file check runs on the target). Exploiting it
/// takes a local process that can already write in the folder the user
/// picked, at the exact moment of the import — accepted and documented.
pub fn read_import_text(path: &Path, kind: ImportKind) -> anyhow::Result<String> {
    read_import_text_with(path, kind, &|| {})
}

/// The open that never follows a final symlink on Unix (see
/// [`read_import_text`]).
fn open_no_follow(path: &Path) -> anyhow::Result<std::fs::File> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    match options.open(path) {
        Ok(f) => Ok(f),
        #[cfg(unix)]
        Err(e) if e.raw_os_error() == Some(libc::ELOOP) => {
            anyhow::bail!("symbolic links can't be imported, pick the file itself")
        }
        Err(e) => Err(e.into()),
    }
}

/// [`read_import_text`] with a hook run between the `lstat` and the open (a
/// test simulates the symlink swap there).
fn read_import_text_with(
    path: &Path,
    kind: ImportKind,
    before_open: &dyn Fn(),
) -> anyhow::Result<String> {
    let bytes = read_picked_file_with(path, kind.extension(), MAX_IMPORT_BYTES, before_open)?;
    let text = String::from_utf8(bytes)
        .map_err(|_| anyhow::anyhow!("file is not UTF-8 text"))?;
    Ok(text.strip_prefix('\u{feff}').map(str::to_owned).unwrap_or(text))
}

/// The bytes of a file the user picked in a native dialog, with the same
/// guards as the list import ([`read_import_text`]): only extension `ext`,
/// a regular file that is not a symlink (`O_NOFOLLOW` on Unix), at most
/// `max_bytes`. For other pickers opened from Rust (a calendar `.ics`,
/// #252).
pub fn read_picked_file(path: &Path, ext: &str, max_bytes: u64) -> anyhow::Result<Vec<u8>> {
    read_picked_file_with(path, ext, max_bytes, &|| {})
}

fn read_picked_file_with(
    path: &Path,
    ext: &str,
    max_bytes: u64,
    before_open: &dyn Fn(),
) -> anyhow::Result<Vec<u8>> {
    use std::io::Read;

    let too_large = || {
        anyhow::anyhow!(
            "file is too large to import (max {} MB)",
            max_bytes / (1024 * 1024)
        )
    };
    let actual = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase());
    if actual.as_deref() != Some(ext) {
        anyhow::bail!("only .{ext} files can be imported here");
    }
    // symlink_metadata does not follow the final component, so a link is
    // refused outright instead of being resolved to whatever it targets.
    let meta = std::fs::symlink_metadata(path)?;
    if meta.file_type().is_symlink() {
        anyhow::bail!("symbolic links can't be imported, pick the file itself");
    }
    if !meta.is_file() {
        anyhow::bail!("not a regular file");
    }
    before_open();
    let file = open_no_follow(path)?;
    // Re-check on the opened handle (the entry could have been swapped since
    // the lstat) and bound the read in case the file grows meanwhile.
    let meta = file.metadata()?;
    if !meta.is_file() {
        anyhow::bail!("not a regular file");
    }
    if meta.len() > max_bytes {
        return Err(too_large());
    }
    let mut bytes = Vec::with_capacity(meta.len() as usize);
    file.take(max_bytes + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > max_bytes {
        return Err(too_large());
    }
    Ok(bytes)
}

/// The import picker may only be opened by the main window, never by the
/// overlay (or any future webview). `label` is the calling window's label.
pub fn check_import_caller(label: &str) -> Result<(), String> {
    if label == "main" {
        Ok(())
    } else {
        Err("import is only available from the main window".into())
    }
}

/// Largest dictionary/snippet export accepted from the webview (#99). Real
/// lists are a few hundred KB even with thousands of entries.
pub const MAX_EXPORT_BYTES: usize = 16 * 1024 * 1024;

/// `path` with `kind`'s extension: kept when it already has it (any case),
/// appended otherwise (a Linux save dialog may not add it).
pub fn export_path(path: &Path, kind: ImportKind) -> PathBuf {
    let has = path
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case(kind.extension()));
    if has {
        path.to_path_buf()
    } else {
        let mut s = path.as_os_str().to_owned();
        s.push(format!(".{}", kind.extension()));
        s.into()
    }
}

/// Write a dictionary (.txt) or snippet (.csv) export to the file the user
/// picked in a native save dialog (#99). The text is built by the frontend
/// in the same format the import reads, so an export can be imported back.
/// Only `kind`'s extension is written, never through a symbolic link, and
/// at most `MAX_EXPORT_BYTES`. Returns the path written.
pub fn write_list_export(path: &Path, kind: ImportKind, contents: &str) -> anyhow::Result<PathBuf> {
    if contents.len() > MAX_EXPORT_BYTES {
        anyhow::bail!("export is too large (max 16 MB)");
    }
    let path = export_path(path, kind);
    if std::fs::symlink_metadata(&path).is_ok_and(|m| m.file_type().is_symlink()) {
        anyhow::bail!("can't write through a symbolic link, pick another file");
    }
    std::fs::write(&path, contents)?;
    Ok(path)
}

/// Validate and read a picked file into the `ImportFile` returned over IPC.
pub fn load_import_file(path: &Path, kind: ImportKind) -> anyhow::Result<ImportFile> {
    let contents = read_import_text(path, kind)?;
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    Ok(ImportFile { name, contents })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snip(cue: &str) -> Snippet {
        Snippet { cue: cue.into(), text: format!("{cue}-text") }
    }
    fn style(m: &str) -> AppStyle {
        AppStyle { app_match: m.into(), style: format!("{m}-style"), ..Default::default() }
    }

    #[test]
    fn roundtrips_through_json() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cfg.json");
        let s = Settings {
            dictionary: vec!["Sussurro".into()],
            snippets: vec![snip("sig")],
            app_styles: vec![style("slack")],
            ..Default::default()
        };
        export_to(&path, &s, &[]).unwrap();
        let bundle = load_bundle(&path).unwrap();
        assert_eq!(bundle, ConfigBundle::from_settings(&s));
    }

    #[test]
    fn merge_unions_without_duplicates() {
        let mut s = Settings {
            dictionary: vec!["Tauri".into()],
            snippets: vec![snip("sig")],
            app_styles: vec![style("slack")],
            ..Default::default()
        };

        let bundle = ConfigBundle {
            dictionary: vec!["tauri".into(), "Sussurro".into()], // "tauri" dup (case)
            snippets: vec![snip("sig"), snip("intro")],          // "sig" dup
            app_styles: vec![style("slack"), style("outlook")],  // "slack" dup
            people: Vec::new(),
        };
        let (w, sn, st) = bundle.merge_into(&mut s);
        assert_eq!((w, sn, st), (1, 1, 1));
        assert_eq!(s.dictionary, vec!["Tauri".to_string(), "Sussurro".into()]);
        assert_eq!(s.snippets.len(), 2);
        assert_eq!(s.app_styles.len(), 2);
    }

    use ImportKind::{Dictionary, Snippets};

    #[test]
    fn read_import_text_reads_txt_and_csv_and_strips_bom() {
        let dir = tempfile::tempdir().unwrap();
        let txt = dir.path().join("dict.TXT");
        std::fs::write(&txt, "\u{feff}Sussurro\nTauri\n").unwrap();
        assert_eq!(read_import_text(&txt, Dictionary).unwrap(), "Sussurro\nTauri\n");
        let csv = dir.path().join("snips.csv");
        std::fs::write(&csv, "cue,text\n").unwrap();
        assert_eq!(read_import_text(&csv, Snippets).unwrap(), "cue,text\n");
    }

    #[test]
    fn read_import_text_accepts_only_the_kinds_extension() {
        let dir = tempfile::tempdir().unwrap();
        let txt = dir.path().join("dict.txt");
        let csv = dir.path().join("snips.csv");
        let json = dir.path().join("settings.json");
        let noext = dir.path().join("noext");
        // A name ending in "txt" without the dot is not a .txt file.
        let fake = dir.path().join("passwordstxt");
        for p in [&txt, &csv, &json, &noext, &fake] {
            std::fs::write(p, "a").unwrap();
        }
        assert!(read_import_text(&txt, Dictionary).is_ok());
        assert!(read_import_text(&csv, Snippets).is_ok());
        // A .csv is not a dictionary, a .txt is not a snippet list.
        assert!(read_import_text(&csv, Dictionary).is_err());
        assert!(read_import_text(&txt, Snippets).is_err());
        for kind in [Dictionary, Snippets] {
            assert!(read_import_text(&json, kind).is_err());
            assert!(read_import_text(&noext, kind).is_err());
            assert!(read_import_text(&fake, kind).is_err());
        }
    }

    #[test]
    fn read_import_text_enforces_the_size_cap() {
        let dir = tempfile::tempdir().unwrap();
        let exact = dir.path().join("exact.txt");
        std::fs::write(&exact, vec![b'a'; MAX_IMPORT_BYTES as usize]).unwrap();
        let text = read_import_text(&exact, Dictionary).unwrap();
        assert_eq!(text.len() as u64, MAX_IMPORT_BYTES);

        let big = dir.path().join("big.txt");
        std::fs::write(&big, vec![b'a'; (MAX_IMPORT_BYTES + 1) as usize]).unwrap();
        let err = read_import_text(&big, Dictionary).unwrap_err().to_string();
        assert!(err.contains("too large"), "{err}");
    }

    #[test]
    fn read_import_text_rejects_non_utf8_dirs_and_missing_files() {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("bin.txt");
        std::fs::write(&bin, [0xff, 0xfe, 0x00, 0x80]).unwrap();
        let err = read_import_text(&bin, Dictionary).unwrap_err().to_string();
        assert!(err.contains("UTF-8"), "{err}");

        let sub = dir.path().join("folder.txt");
        std::fs::create_dir(&sub).unwrap();
        assert!(read_import_text(&sub, Dictionary).is_err());

        assert!(read_import_text(&dir.path().join("missing.txt"), Dictionary).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn read_import_text_refuses_symlinks() {
        let dir = tempfile::tempdir().unwrap();
        // A link named like a dictionary that points at some other file
        // (think an exported passwords file) must not be followed.
        let secret = dir.path().join("secret.json");
        std::fs::write(&secret, "{\"password\":\"x\"}").unwrap();
        let link = dir.path().join("dict.txt");
        std::os::unix::fs::symlink(&secret, &link).unwrap();
        let err = read_import_text(&link, Dictionary).unwrap_err().to_string();
        assert!(err.contains("symbolic link"), "{err}");

        // Even a link to a valid .txt is refused: pick the file itself.
        let real = dir.path().join("real.txt");
        std::fs::write(&real, "Sussurro\n").unwrap();
        let alias = dir.path().join("alias.txt");
        std::os::unix::fs::symlink(&real, &alias).unwrap();
        assert!(read_import_text(&alias, Dictionary).is_err());
        assert_eq!(read_import_text(&real, Dictionary).unwrap(), "Sussurro\n");
    }

    /// #158 finding 8: a symlink swapped in between the lstat check and the
    /// open is not followed.
    #[cfg(unix)]
    #[test]
    fn read_import_text_refuses_a_symlink_swapped_in_after_the_check() {
        let dir = tempfile::tempdir().unwrap();
        let secret = dir.path().join("secret.json");
        std::fs::write(&secret, "{\"password\":\"x\"}").unwrap();
        let picked = dir.path().join("dict.txt");
        std::fs::write(&picked, "Sussurro\n").unwrap();
        let swap = || {
            std::fs::remove_file(&picked).unwrap();
            std::os::unix::fs::symlink(&secret, &picked).unwrap();
        };
        let err = read_import_text_with(&picked, Dictionary, &swap)
            .unwrap_err()
            .to_string();
        assert!(err.contains("symbolic link"), "{err}");
    }

    #[test]
    fn load_import_file_returns_only_name_and_contents() {
        let dir = tempfile::tempdir().unwrap();
        let csv = dir.path().join("snips.csv");
        std::fs::write(&csv, "sig,Best regards\n").unwrap();
        let f = load_import_file(&csv, Snippets).unwrap();
        assert_eq!(
            f,
            ImportFile { name: "snips.csv".into(), contents: "sig,Best regards\n".into() }
        );
        // Nothing but these two fields crosses IPC: no path, no directory.
        let json = serde_json::to_value(&f).unwrap();
        let mut keys: Vec<&String> = json.as_object().unwrap().keys().collect();
        keys.sort();
        assert_eq!(keys, ["contents", "name"]);
        assert!(load_import_file(&csv, Dictionary).is_err());
    }

    /// The calendar picker (#252) reads through the same guards.
    #[test]
    fn picked_files_keep_the_import_guards() {
        let dir = tempfile::tempdir().unwrap();
        let ics = dir.path().join("Team.ICS");
        std::fs::write(&ics, "BEGIN:VCALENDAR\r\nEND:VCALENDAR\r\n").unwrap();
        assert_eq!(read_picked_file(&ics, "ics", 1024).unwrap().len(), 32);
        let e = read_picked_file(&ics, "ics", 10).unwrap_err().to_string();
        assert!(e.contains("too large"), "{e}");
        let e = read_picked_file(&ics, "txt", 1024).unwrap_err().to_string();
        assert!(e.contains("only .txt"), "{e}");
        #[cfg(unix)]
        {
            let link = dir.path().join("link.ics");
            std::os::unix::fs::symlink(&ics, &link).unwrap();
            assert!(read_picked_file(&link, "ics", 1024).is_err());
        }
    }

    #[test]
    fn export_path_adds_the_kinds_extension_only_when_missing() {
        let p = export_path(Path::new("/x/words"), Dictionary);
        assert_eq!(p, Path::new("/x/words.txt"));
        let p = export_path(Path::new("/x/words.TXT"), Dictionary);
        assert_eq!(p, Path::new("/x/words.TXT"));
        let p = export_path(Path::new("/x/snips.txt"), Snippets);
        assert_eq!(p, Path::new("/x/snips.txt.csv"));
    }

    #[test]
    fn list_export_writes_text_that_the_import_reads_back() {
        let dir = tempfile::tempdir().unwrap();
        let written =
            write_list_export(&dir.path().join("snips"), Snippets, "cue,text\nfirma,\"a, b\"\n")
                .unwrap();
        assert_eq!(written, dir.path().join("snips.csv"));
        assert_eq!(
            read_import_text(&written, Snippets).unwrap(),
            "cue,text\nfirma,\"a, b\"\n"
        );
    }

    #[test]
    fn list_export_enforces_the_size_cap() {
        let dir = tempfile::tempdir().unwrap();
        let big = "a".repeat(MAX_EXPORT_BYTES + 1);
        assert!(write_list_export(&dir.path().join("big.txt"), Dictionary, &big).is_err());
        assert!(!dir.path().join("big.txt").exists());
    }

    #[cfg(unix)]
    #[test]
    fn list_export_refuses_to_write_through_a_symlink() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("elsewhere.txt");
        std::fs::write(&target, "keep").unwrap();
        let link = dir.path().join("words.txt");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        assert!(write_list_export(&link, Dictionary, "new\n").is_err());
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "keep");
    }

    #[test]
    fn only_the_main_window_may_import() {
        assert!(check_import_caller("main").is_ok());
        assert!(check_import_caller("overlay").is_err());
        assert!(check_import_caller("Main").is_err());
        assert!(check_import_caller("").is_err());
    }

    #[test]
    fn import_kind_deserializes_from_lowercase_only() {
        assert_eq!(serde_json::from_str::<ImportKind>("\"dictionary\"").unwrap(), Dictionary);
        assert_eq!(serde_json::from_str::<ImportKind>("\"snippets\"").unwrap(), Snippets);
        assert!(serde_json::from_str::<ImportKind>("\"settings\"").is_err());
        assert!(serde_json::from_str::<ImportKind>("\"/etc/passwd\"").is_err());
    }

    /// #159: the portable config never carries LLM profiles, so never an
    /// API key — whether the key is in the credential store or, as a
    /// fallback, in settings.json.
    #[test]
    fn export_never_contains_api_keys() {
        use crate::llm::{KeyStorage, LlmProfile};
        use crate::settings::CleanupApi;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cfg.json");
        let mut in_store =
            LlmProfile::new("a", "A", CleanupApi::Openai, "https://a.example/v1", "sk-store-key", "m");
        in_store.api_key_storage = KeyStorage::Keychain;
        let mut in_file =
            LlmProfile::new("b", "B", CleanupApi::Openai, "https://b.example/v1", "sk-file-key", "m");
        in_file.api_key_storage = KeyStorage::File;
        let s = Settings {
            llm_profiles: vec![in_store, in_file],
            dictionary: vec!["Sussurro".into()],
            ..Default::default()
        };
        export_to(&path, &s, &[]).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(!text.contains("sk-"), "{text}");
        assert!(!text.contains("api_key"), "{text}");
        assert!(!text.contains("llm_profiles"), "{text}");
    }

    /// #249: the portable config never carries the archive tokens (not even
    /// their names or hashes) nor the extension token.
    #[test]
    fn export_never_contains_tokens() {
        use crate::api::tokens::{create, Scope};
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cfg.json");
        let (stored, new) = create(&[], "backup script", &[Scope::Read], chrono::Utc::now()).unwrap();
        let s = Settings {
            archive_tokens: vec![stored.clone()],
            api_archive: true,
            extension_token: "ext-secret-token".into(),
            dictionary: vec!["Sussurro".into()],
            ..Default::default()
        };
        export_to(&path, &s, &[]).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        for secret in [new.token.as_str(), stored.sha256.as_str(), "backup script", "ext-secret-token", "archive_tokens"] {
            assert!(!text.contains(secret), "{secret} in {text}");
        }
    }

    #[test]
    fn bundle_excludes_machine_specific_fields() {
        // Compile-time guard: ConfigBundle has exactly the portable fields.
        let b = ConfigBundle::default();
        let _ = (b.dictionary, b.snippets, b.app_styles, b.people);
    }

    #[test]
    fn people_are_exported_only_when_passed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cfg.json");
        let s = Settings { dictionary: vec!["Sussurro".into()], ..Default::default() };
        export_to(&path, &s, &[]).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(!text.contains("people"), "default: no People key at all\n{text}");

        let anna = Person {
            id: "p-1".into(),
            name: "Anna Rossi".into(),
            email: Some("anna@example.com".into()),
            aliases: vec!["Annie".into()],
        };
        export_to(&path, &s, std::slice::from_ref(&anna)).unwrap();
        assert_eq!(load_bundle(&path).unwrap().people, vec![anna]);
        // An older bundle without the key still loads.
        std::fs::write(&path, r#"{"dictionary":["x"]}"#).unwrap();
        assert!(load_bundle(&path).unwrap().people.is_empty());
    }
}
