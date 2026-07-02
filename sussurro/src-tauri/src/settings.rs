use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum CleanupLevel {
    None,
    Light,
    Medium,
    High,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Settings {
    /// Shortcut string parsed by tauri-plugin-global-shortcut, e.g. "CommandOrControl+Shift+Space".
    pub hotkey: String,
    /// true = hold to record (press starts, release stops); false = tap toggles.
    pub push_to_talk: bool,
    /// GGML model file name inside the app's models dir.
    pub whisper_model: String,
    pub ollama_url: String,
    pub ollama_model: String,
    pub cleanup_level: CleanupLevel,
    /// Personal dictionary: names/jargon fed to both Whisper and the LLM.
    pub dictionary: Vec<String>,
    /// Start Sussurro (hidden in the tray) when the user logs in.
    pub autostart: bool,
    /// Audible tick when recording starts/stops.
    pub sound_feedback: bool,
    /// Whisper language hint: "auto" or an ISO 639-1 code like "it", "en".
    pub language: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            hotkey: "CommandOrControl+Shift+Space".into(),
            push_to_talk: true,
            whisper_model: "ggml-large-v3-turbo-q5_0.bin".into(),
            ollama_url: "http://localhost:11434".into(),
            ollama_model: "llama3.2:3b".into(),
            cleanup_level: CleanupLevel::Light,
            dictionary: Vec::new(),
            autostart: false,
            sound_feedback: true,
            language: "auto".into(),
        }
    }
}

impl Settings {
    /// Missing or unreadable file yields defaults — the app must always start.
    pub fn load(path: &Path) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(path, serde_json::to_string_pretty(self).expect("settings serialize"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_settings_are_sensible() {
        let s = Settings::default();
        assert_eq!(s.hotkey, "CommandOrControl+Shift+Space");
        assert!(s.push_to_talk);
        assert_eq!(s.cleanup_level, CleanupLevel::Light);
        assert_eq!(s.ollama_url, "http://localhost:11434");
        assert!(s.dictionary.is_empty());
        assert!(!s.autostart);
    }

    #[test]
    fn save_then_load_roundtrips() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("settings.json");
        let mut s = Settings::default();
        s.hotkey = "Alt+Space".into();
        s.dictionary = vec!["Sussurro".into(), "Tauri".into()];
        s.cleanup_level = CleanupLevel::High;
        s.save(&path).unwrap();
        assert_eq!(Settings::load(&path), s);
    }

    #[test]
    fn load_missing_or_corrupt_file_falls_back_to_defaults() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(Settings::load(&dir.path().join("nope.json")), Settings::default());
        let bad = dir.path().join("bad.json");
        std::fs::write(&bad, "{not json").unwrap();
        assert_eq!(Settings::load(&bad), Settings::default());
    }
}
