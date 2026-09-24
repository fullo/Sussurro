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

/// Which chat API the cleanup LLM is driven through.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "snake_case")]
pub enum CleanupApi {
    /// Ollama's native schema (`/api/chat`, `/api/tags`). The default.
    #[default]
    Ollama,
    /// OpenAI-compatible (`/v1/chat/completions`, `/v1/models`) — drives any
    /// local runtime that speaks it: llama.cpp-server, LM Studio, DS4, and
    /// Ollama's own `/v1` endpoint.
    Openai,
}

/// Optional user overrides for the per-level cleanup instructions sent to the
/// LLM. Empty string = use the built-in default for that level.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct PromptOverrides {
    pub light: String,
    pub medium: String,
    pub high: String,
}

/// Tone rule applied when dictating into a matching application.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct AppStyle {
    /// Case-insensitive substring of the focused app's name (e.g. "slack").
    pub app_match: String,
    /// Style instruction appended to the cleanup prompt.
    pub style: String,
    /// Per-app output language (ISO code). Empty = follow the global
    /// "Translate to" setting. E.g. slack → "en", whatsapp → "it".
    #[serde(default)]
    pub language: String,
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
    /// Which chat API drives cleanup. Ollama native by default;
    /// OpenAI-compatible reinterprets `ollama_url` as the server base and
    /// `ollama_model` as the model id, talking `/v1/chat/completions`.
    pub cleanup_api: CleanupApi,
    /// Bearer token for the OpenAI-compatible API. Optional — most local
    /// servers ignore it. Never sent to the Ollama native API.
    pub api_key: String,
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
    /// Input device name for capture. Empty = system default microphone.
    pub input_device: String,
    /// Quiet-speech mode: boosts mic gain and lowers the silence gate.
    pub whisper_mode: bool,
    /// EXPERIMENTAL: type text into the app while speaking. With cleanup
    /// None it streams word by word; with cleanup on, sentence by sentence
    /// (each completed sentence is LLM-cleaned before being typed).
    pub stream_injection: bool,
    /// Interpret spoken editing commands ("a capo"/"new line", and with
    /// cleanup on also "scratch that"/"cancella quello").
    pub voice_commands: bool,
    /// Advanced: custom per-level cleanup instructions (empty = default).
    pub prompt_overrides: PromptOverrides,
    /// Auto-delete history entries older than N days (0 = keep forever).
    pub history_retention_days: u32,
    /// Local HTTP API on 127.0.0.1 for scripting (applied at startup).
    pub api_enabled: bool,
    pub api_port: u16,
    /// Dictate-to-file mode: when set, completed dictations are APPENDED to
    /// this file (note-taking) instead of being pasted into the focused app.
    pub output_file: String,
    /// Archive folder for notes, meetings and transcriptions. Empty = the
    /// default `<Documents>/Sussurro` (see `archive::resolve_archive_dir`).
    pub archive_dir: String,
    /// Preview of the 0.7 workspace UI (left rail: New, Library, Models,
    /// Settings). Off = today's single-column window. Removed when 0.7 ships.
    pub ui_v2: bool,
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
            cleanup_api: CleanupApi::Ollama,
            api_key: String::new(),
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
            input_device: String::new(),
            whisper_mode: false,
            stream_injection: false,
            voice_commands: true,
            prompt_overrides: PromptOverrides::default(),
            history_retention_days: 0,
            api_enabled: false,
            api_port: 4525,
            output_file: String::new(),
            archive_dir: String::new(),
            ui_v2: false,
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

    /// Save settings as pretty JSON, creating parent directories as needed.
    /// Serialization failures are mapped into the returned `io::Error` instead
    /// of panicking — a settings write must degrade to an error the caller can
    /// report, never take down the app.
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let json = serde_json::to_string_pretty(self)
            .map_err(|e| std::io::Error::other(format!("serialize settings: {e}")))?;
        std::fs::write(path, json)
    }
}

/// Whether a user-entered cleanup endpoint URL points at this machine:
/// `localhost`, the loopback addresses (`127.0.0.1` / `::1`) or mDNS `.local`
/// hostnames. Anything else — including unparseable input — counts as remote,
/// so the privacy warning errs on the safe side.
pub fn is_local_endpoint(url: &str) -> bool {
    let Some(host) = endpoint_host(url) else {
        return false;
    };
    host == "localhost" || host == "127.0.0.1" || host == "::1" || host.ends_with(".local")
}

/// Host of a user-entered endpoint URL, or `None` when there is none. Accepts
/// an optional scheme (`localhost:11434` works), and strips the port,
/// userinfo and any path/query/fragment.
fn endpoint_host(url: &str) -> Option<String> {
    let s = url.trim();
    // Drop an optional "scheme://" prefix so bare hosts parse too.
    let rest = s.split_once("://").map(|(_, r)| r).unwrap_or(s);
    // Authority only: up to the first path/query/fragment character.
    let mut authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    if authority.is_empty() {
        return None;
    }
    // Strip userinfo ("user@host") and then the port — bracketed IPv6
    // literals keep their address inside the brackets.
    if let Some(at) = authority.rfind('@') {
        authority = &authority[at + 1..];
    }
    let host = if let Some(bracketed) = authority.strip_prefix('[') {
        bracketed.split_once(']').map(|(inner, _)| inner.to_string())
    } else if let Some(colon) = authority.rfind(':') {
        Some(authority[..colon].to_string())
    } else {
        Some(authority.to_string())
    }?;
    let host = host.to_lowercase();
    (!host.is_empty()).then_some(host)
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

    /// Settings always serialize, so the serde-error branch of save() is not
    /// directly reachable; exercise its error-propagation shape instead: an
    /// unwritable location must surface as `Err`, not a panic.
    #[test]
    fn save_propagates_io_errors_instead_of_panicking() {
        let dir = tempfile::tempdir().unwrap();
        // A file where the parent directory is expected: create_dir_all fails,
        // so save() must return the io error.
        let blocker = dir.path().join("blocker");
        std::fs::write(&blocker, "x").unwrap();
        let path = blocker.join("settings.json");
        assert!(Settings::default().save(&path).is_err());
    }

    #[test]
    fn is_local_endpoint_recognizes_local_hosts() {
        for url in [
            "http://localhost:11434",
            "http://localhost",
            "https://localhost:8080/v1/chat/completions",
            "localhost:11434", // no scheme
            "HTTP://LOCALHOST/", // case-insensitive
            "http://user@localhost:11434", // userinfo stripped
            "http://127.0.0.1:11434",
            "http://[::1]:11434",
            "http://[::1]",
            "http://my-mac.local:8080",
            "http://foo.local/path?q=1#f", // path/query/fragment ignored
        ] {
            assert!(is_local_endpoint(url), "{url}");
        }
    }

    #[test]
    fn is_local_endpoint_rejects_remote_hosts() {
        for url in [
            "http://example.com:11434",
            "https://api.openai.com/v1",
            "http://192.168.1.50:8080",
            "http://10.0.0.5",
            "example.com:11434", // no scheme, remote host
        ] {
            assert!(!is_local_endpoint(url), "{url}");
        }
    }

    #[test]
    fn is_local_endpoint_handles_malformed_input() {
        for url in ["", "   ", "http://", "://nope", "not a url", "http://:11434"] {
            assert!(!is_local_endpoint(url), "{url}");
        }
    }

    /// Command mode was removed in 0.7: a settings.json written by an older
    /// version still carries `command_hotkey`. It must load (the unknown key
    /// is ignored, the other fields kept) and the key must be gone after the
    /// next save.
    #[test]
    fn legacy_command_hotkey_is_ignored_and_dropped_on_save() {
        let legacy = r#"{
            "hotkey": "Alt+Space",
            "command_hotkey": "CommandOrControl+Alt+Space",
            "push_to_talk": false
        }"#;
        let s: Settings = serde_json::from_str(legacy).expect("legacy settings must load");
        assert_eq!(
            s,
            Settings {
                hotkey: "Alt+Space".into(),
                push_to_talk: false,
                ..Default::default()
            }
        );

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(&path, legacy).unwrap();
        let loaded = Settings::load(&path);
        assert_eq!(loaded, s, "load() must not fall back to defaults");
        loaded.save(&path).unwrap();
        let saved = std::fs::read_to_string(&path).unwrap();
        assert!(!saved.contains("command_hotkey"), "{saved}");
        assert!(!serde_json::to_string(&s).unwrap().contains("command_hotkey"));
    }

    /// Settings files written before 0.7 have no `archive_dir`: they must
    /// still load (serde default) and get the default archive location.
    #[test]
    fn settings_without_archive_dir_load_with_default() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(&path, r#"{"hotkey":"Alt+Space","models_dir":"/m"}"#).unwrap();
        let s = Settings::load(&path);
        assert_eq!(s.hotkey, "Alt+Space");
        assert_eq!(s.archive_dir, "");
    }

    /// Settings files written before the workspace preview have no `ui_v2`:
    /// they load with the classic UI (serde default).
    #[test]
    fn settings_without_ui_v2_keep_the_classic_ui() {
        let s: Settings = serde_json::from_str(r#"{"hotkey":"Alt+Space"}"#).unwrap();
        assert!(!s.ui_v2);
        assert!(!Settings::default().ui_v2);
        let on: Settings = serde_json::from_str(r#"{"ui_v2":true}"#).unwrap();
        assert!(on.ui_v2);
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
