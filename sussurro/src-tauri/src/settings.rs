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

/// Which local speech-to-text engine transcribes the audio.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum SttEngine {
    /// whisper.cpp — GPU-accelerated, any language, pick a model size.
    Whisper,
    /// NVIDIA Parakeet TDT v3 (ONNX) — CPU-optimized, ~10x faster than
    /// Whisper on CPU, auto-detects 25 European languages.
    Parakeet,
}

/// Tone rule applied when dictating into a matching application.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct AppStyle {
    /// Case-insensitive substring of the focused app's name (e.g. "slack").
    pub app_match: String,
    /// Style instruction appended to the cleanup prompt.
    pub style: String,
}

/// A voice shortcut: say the cue, get the full text pasted.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct Snippet {
    pub cue: String,
    pub text: String,
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
    /// Speech-to-text engine (whisper_model only applies to Whisper).
    pub engine: SttEngine,
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
    /// Translate the cleaned text into this language. Empty/"same" = keep the
    /// dictated language. An ISO 639-1 code otherwise (e.g. "en").
    pub output_language: String,
    /// Voice shortcuts: dictating exactly a cue pastes its text instead.
    pub snippets: Vec<Snippet>,
    /// Show a live partial transcript in the overlay while speaking.
    pub live_preview: bool,
    /// Per-app tone rules (Wispr-style tone matching).
    pub app_styles: Vec<AppStyle>,
    /// Where STT models are stored. Empty = the app data dir default.
    /// Point it at a roomier disk (e.g. F:\claude\models) if C: is tight.
    pub models_dir: String,
    /// Command mode shortcut: the spoken instruction is applied to the
    /// currently selected text via the LLM (Wispr's command mode).
    pub command_hotkey: String,
    /// Quiet-speech mode: boosts mic gain and lowers the silence gate.
    pub whisper_mode: bool,
    /// EXPERIMENTAL: type text into the app while speaking. With cleanup
    /// None it streams word by word; with cleanup on, sentence by sentence
    /// (each completed sentence is LLM-cleaned before being typed).
    pub stream_injection: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            hotkey: "CommandOrControl+Shift+Space".into(),
            push_to_talk: true,
            whisper_model: "ggml-large-v3-turbo-q5_0.bin".into(),
            engine: SttEngine::Whisper,
            ollama_url: "http://localhost:11434".into(),
            ollama_model: "llama3.2:3b".into(),
            cleanup_level: CleanupLevel::Light,
            dictionary: Vec::new(),
            autostart: false,
            sound_feedback: true,
            language: "auto".into(),
            output_language: String::new(),
            snippets: Vec::new(),
            live_preview: true,
            app_styles: Vec::new(),
            models_dir: String::new(),
            command_hotkey: "CommandOrControl+Alt+Space".into(),
            whisper_mode: false,
            stream_injection: false,
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
        let s = Settings {
            hotkey: "Alt+Space".into(),
            dictionary: vec!["Sussurro".into(), "Tauri".into()],
            cleanup_level: CleanupLevel::High,
            ..Default::default()
        };
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
