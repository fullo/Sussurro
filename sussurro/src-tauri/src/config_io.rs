use crate::settings::{AppStyle, Settings, Snippet};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Portable subset of the settings — the parts worth carrying between machines
/// (dictionary, voice snippets, per-app styles). Deliberately excludes
/// machine-specific fields like models_dir, the hotkey, and the input device.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct ConfigBundle {
    #[serde(default)]
    pub dictionary: Vec<String>,
    #[serde(default)]
    pub snippets: Vec<Snippet>,
    #[serde(default)]
    pub app_styles: Vec<AppStyle>,
}

impl ConfigBundle {
    pub fn from_settings(s: &Settings) -> Self {
        Self {
            dictionary: s.dictionary.clone(),
            snippets: s.snippets.clone(),
            app_styles: s.app_styles.clone(),
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

pub fn export_to(path: &Path, settings: &Settings) -> std::io::Result<()> {
    let bundle = ConfigBundle::from_settings(settings);
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
pub fn read_import_text(path: &Path, kind: ImportKind) -> anyhow::Result<String> {
    use std::io::Read;

    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase());
    if ext.as_deref() != Some(kind.extension()) {
        anyhow::bail!("only .{} files can be imported here", kind.extension());
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
    let file = std::fs::File::open(path)?;
    // Re-check on the opened handle (the entry could have been swapped since
    // the lstat) and bound the read in case the file grows meanwhile.
    let meta = file.metadata()?;
    if !meta.is_file() {
        anyhow::bail!("not a regular file");
    }
    if meta.len() > MAX_IMPORT_BYTES {
        anyhow::bail!("file is too large to import (max 1 MB)");
    }
    let mut bytes = Vec::with_capacity(meta.len() as usize);
    file.take(MAX_IMPORT_BYTES + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_IMPORT_BYTES {
        anyhow::bail!("file is too large to import (max 1 MB)");
    }
    let text = String::from_utf8(bytes)
        .map_err(|_| anyhow::anyhow!("file is not UTF-8 text"))?;
    Ok(text.strip_prefix('\u{feff}').map(str::to_owned).unwrap_or(text))
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
        export_to(&path, &s).unwrap();
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

    #[test]
    fn bundle_excludes_machine_specific_fields() {
        // Compile-time guard: ConfigBundle has exactly the portable fields.
        let b = ConfigBundle::default();
        let _ = (b.dictionary, b.snippets, b.app_styles);
    }
}
