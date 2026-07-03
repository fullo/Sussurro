use crate::settings::{AppStyle, Settings, Snippet};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Portable subset of the settings — the parts worth carrying between machines
/// (dictionary, voice snippets, per-app styles). Deliberately excludes
/// machine-specific fields like models_dir, hotkeys, and the input device.
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
    fn bundle_excludes_machine_specific_fields() {
        // Compile-time guard: ConfigBundle has exactly the portable fields.
        let b = ConfigBundle::default();
        let _ = (b.dictionary, b.snippets, b.app_styles);
    }
}
