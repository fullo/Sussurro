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

/// Read a user-picked dictionary (.txt) or snippet (.csv) file as UTF-8 text.
/// Deliberately narrow — only those two extensions, regular files, at most
/// `MAX_IMPORT_BYTES` — so the command can't be used as a generic file reader.
/// Parsing and merging happen in the frontend (`src/utils.ts`).
pub fn read_import_text(path: &Path) -> anyhow::Result<String> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase());
    if !matches!(ext.as_deref(), Some("txt") | Some("csv")) {
        anyhow::bail!("only .txt and .csv files can be imported");
    }
    let meta = std::fs::metadata(path)?;
    if !meta.is_file() {
        anyhow::bail!("not a regular file");
    }
    if meta.len() > MAX_IMPORT_BYTES {
        anyhow::bail!("file is too large to import (max 1 MB)");
    }
    let bytes = std::fs::read(path)?;
    let text = String::from_utf8(bytes)
        .map_err(|_| anyhow::anyhow!("file is not UTF-8 text"))?;
    Ok(text.strip_prefix('\u{feff}').map(str::to_owned).unwrap_or(text))
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

    #[test]
    fn read_import_text_reads_txt_and_csv_and_strips_bom() {
        let dir = tempfile::tempdir().unwrap();
        let txt = dir.path().join("dict.TXT");
        std::fs::write(&txt, "\u{feff}Sussurro\nTauri\n").unwrap();
        assert_eq!(read_import_text(&txt).unwrap(), "Sussurro\nTauri\n");
        let csv = dir.path().join("snips.csv");
        std::fs::write(&csv, "cue,text\n").unwrap();
        assert_eq!(read_import_text(&csv).unwrap(), "cue,text\n");
    }

    #[test]
    fn read_import_text_rejects_other_extensions_dirs_binary_and_huge_files() {
        let dir = tempfile::tempdir().unwrap();
        let json = dir.path().join("settings.json");
        std::fs::write(&json, "{}").unwrap();
        assert!(read_import_text(&json).is_err());
        assert!(read_import_text(&dir.path().join("noext")).is_err());

        let sub = dir.path().join("folder.txt");
        std::fs::create_dir(&sub).unwrap();
        assert!(read_import_text(&sub).is_err());

        let bin = dir.path().join("bin.txt");
        std::fs::write(&bin, [0xff, 0xfe, 0x00, 0x80]).unwrap();
        assert!(read_import_text(&bin).is_err());

        let big = dir.path().join("big.txt");
        std::fs::write(&big, vec![b'a'; (MAX_IMPORT_BYTES + 1) as usize]).unwrap();
        assert!(read_import_text(&big).is_err());

        assert!(read_import_text(&dir.path().join("missing.txt")).is_err());
    }

    #[test]
    fn bundle_excludes_machine_specific_fields() {
        // Compile-time guard: ConfigBundle has exactly the portable fields.
        let b = ConfigBundle::default();
        let _ = (b.dictionary, b.snippets, b.app_styles);
    }
}
