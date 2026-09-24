use crate::llm::{KeyStorage, LlmProfile, DEFAULT_OLLAMA_MODEL, DEFAULT_OLLAMA_URL, LOCAL_PROFILE_ID};
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

/// When `transcript.srt` is written next to a meeting's or a
/// transcription's transcript (P7; notes never get subtitles, P10).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum SubtitlesMode {
    /// Only when the user asks ("Create .srt", or an export). The default.
    #[default]
    OnRequest,
    /// Written and kept up to date every time the transcript is saved (end
    /// of a run, line edits, metadata edits).
    Always,
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
    /// LLM profiles (0.8, #119): named chat-model connections that cleanup
    /// (and later recipes and Ask) pick from. Missing in a pre-0.8 file: the
    /// field-level default leaves it empty and [`Settings::normalize`]
    /// migrates the flat `ollama_url`/`ollama_model`/`cleanup_api`/`api_key`
    /// into one "Local" profile.
    #[serde(default)]
    pub llm_profiles: Vec<LlmProfile>,
    /// Id of the profile that cleans dictations, long-form runs and the
    /// local API's `/clean`. See [`Settings::cleanup_llm`].
    pub cleanup_profile: String,
    /// The user's own recipes (#120). The built-in ones live in code
    /// ([`crate::recipes::builtin_recipes`]) and are never stored here.
    pub recipes: Vec<crate::recipes::Recipe>,
    /// Pre-0.8 flat cleanup settings (`ollama_url`, `ollama_model`,
    /// `cleanup_api`, `api_key`): **read once for the migration, never
    /// written**. `None` when absent (the 0.6 defaults then apply).
    #[serde(default, rename = "ollama_url", skip_serializing)]
    pub legacy_ollama_url: Option<String>,
    #[serde(default, rename = "ollama_model", skip_serializing)]
    pub legacy_ollama_model: Option<String>,
    #[serde(default, rename = "cleanup_api", skip_serializing)]
    pub legacy_cleanup_api: Option<CleanupApi>,
    #[serde(default, rename = "api_key", skip_serializing)]
    pub legacy_api_key: Option<String>,
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
    /// Preview of the 0.9 meeting features (E12): meetings, speaker labels
    /// ("Voice N") and the speaker panel. Off by default; removed when 0.9
    /// ships (#138).
    pub meetings_enabled: bool,
    /// Subtitles setting (P7, #133): `transcript.srt` on request (default)
    /// or on every save. Meetings and transcriptions only.
    pub subtitles: SubtitlesMode,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            hotkey: "CommandOrControl+Shift+Space".into(),
            push_to_talk: true,
            whisper_model: "ggml-large-v3-turbo-q5_0.bin".into(),
            engine: SttEngine::Whisper,
            llm_profiles: vec![LlmProfile::default()],
            cleanup_profile: LOCAL_PROFILE_ID.into(),
            recipes: Vec::new(),
            legacy_ollama_url: None,
            legacy_ollama_model: None,
            legacy_cleanup_api: None,
            legacy_api_key: None,
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
            meetings_enabled: false,
            subtitles: SubtitlesMode::OnRequest,
        }
    }
}

impl Settings {
    /// Missing or unreadable file yields defaults — the app must always start.
    /// The result is [normalized](Settings::normalize): a pre-0.8 file comes
    /// back with its cleanup settings as the "Local" profile.
    pub fn load(path: &Path) -> Self {
        Self::load_migrating(path).0
    }

    /// [`Settings::load`], also telling whether the file needed the pre-0.8
    /// → profiles migration (the caller then saves it once, so the file on
    /// disk has the new shape).
    pub fn load_migrating(path: &Path) -> (Self, bool) {
        let Some(mut settings) = std::fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str::<Settings>(&s).ok())
        else {
            return (Self::default(), false);
        };
        let migrated = settings.normalize();
        (settings, migrated)
    }

    /// Bring the LLM profiles into a usable shape; returns true when the
    /// pre-0.8 flat cleanup settings were migrated.
    ///
    /// - No profiles (a pre-0.8 file): one "Local" profile built from the
    ///   legacy `cleanup_api`/`ollama_url`/`ollama_model`/`api_key` (0.6
    ///   defaults for any that are absent), `external` inferred from its URL,
    ///   and selected for cleanup — so migrated users clean exactly as before.
    /// - The legacy fields are then dropped: they are never written back.
    /// - Empty or duplicate ids get a fresh `profile-N`, empty names "Untitled".
    /// - A `cleanup_profile` naming no profile falls back to the first one.
    /// - User recipes get unique ids that don't clash with the built-ins
    ///   (see [`crate::recipes::normalize_user_recipes`]).
    ///
    /// Idempotent. Run on load and on every save from the UI.
    pub fn normalize(&mut self) -> bool {
        let migrated = self.llm_profiles.is_empty();
        if migrated {
            self.llm_profiles.push(self.legacy_profile());
            self.cleanup_profile = LOCAL_PROFILE_ID.into();
        }
        self.legacy_ollama_url = None;
        self.legacy_ollama_model = None;
        self.legacy_cleanup_api = None;
        self.legacy_api_key = None;

        let all: std::collections::HashSet<String> =
            self.llm_profiles.iter().map(|p| p.id.clone()).collect();
        let mut seen = std::collections::HashSet::new();
        let mut next = 1;
        for p in &mut self.llm_profiles {
            p.id = p.id.trim().to_string();
            if p.id.is_empty() || !seen.insert(p.id.clone()) {
                while all.contains(&format!("profile-{next}")) || seen.contains(&format!("profile-{next}")) {
                    next += 1;
                }
                p.id = format!("profile-{next}");
                seen.insert(p.id.clone());
            }
            if p.name.trim().is_empty() {
                p.name = "Untitled".into();
            }
        }
        if !self.llm_profiles.iter().any(|p| p.id == self.cleanup_profile) {
            self.cleanup_profile = self.llm_profiles[0].id.clone();
        }
        crate::recipes::normalize_user_recipes(&mut self.recipes);
        migrated
    }

    /// The "Local" profile a pre-0.8 settings file describes.
    fn legacy_profile(&self) -> LlmProfile {
        LlmProfile::new(
            LOCAL_PROFILE_ID,
            "Local",
            self.legacy_cleanup_api.clone().unwrap_or_default(),
            self.legacy_ollama_url.as_deref().unwrap_or(DEFAULT_OLLAMA_URL),
            self.legacy_api_key.as_deref().unwrap_or(""),
            self.legacy_ollama_model.as_deref().unwrap_or(DEFAULT_OLLAMA_MODEL),
        )
    }

    /// The profile cleanup runs on: the one `cleanup_profile` names, else the
    /// first profile, else (settings never normalized) the one the legacy
    /// fields describe — so cleanup always has somewhere to go.
    pub fn cleanup_llm(&self) -> LlmProfile {
        self.llm_profiles
            .iter()
            .find(|p| p.id == self.cleanup_profile)
            .or_else(|| self.llm_profiles.first())
            .cloned()
            .unwrap_or_else(|| self.legacy_profile())
    }

    /// Cleanup calls an LLM at all: a level other than None, or a
    /// translation (which runs even with cleanup None).
    pub fn cleanup_active(&self) -> bool {
        self.cleanup_level != CleanupLevel::None
            || crate::cleanup::prompt::output_language_name(&self.output_language).is_some()
    }

    /// Cleanup text leaves this machine: cleanup is active on an external
    /// profile the user opted in for (#122). Items cleaned this way are
    /// recorded in their external-send log.
    pub fn cleanup_sends_externally(&self) -> bool {
        let p = self.cleanup_llm();
        self.cleanup_active() && p.external && p.cleanup_allowed()
    }

    /// Cleanup is held back: active, on an external profile without the
    /// opt-in — dictations and transcriptions keep their raw text (#122).
    pub fn cleanup_blocked(&self) -> bool {
        self.cleanup_active() && !self.cleanup_llm().cleanup_allowed()
    }

    /// Save settings as pretty JSON, creating parent directories as needed.
    /// Serialization failures are mapped into the returned `io::Error` instead
    /// of panicking — a settings write must degrade to an error the caller can
    /// report, never take down the app.
    ///
    /// API keys kept in the OS credential store are left out (#159): see
    /// [`Settings::for_disk`].
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let json = serde_json::to_string_pretty(&self.for_disk())
            .map_err(|e| std::io::Error::other(format!("serialize settings: {e}")))?;
        std::fs::write(path, json)
    }

    /// The settings as `settings.json` stores them (#159): a profile whose
    /// key is in the credential store keeps only the reference
    /// (`api_key_storage: "keychain"`, also for a key that could not be read
    /// this session, so its entry is not forgotten) and no key. A key that
    /// is not in the store (fallback, or not migrated yet) stays in clear
    /// text — dropping it would lose it.
    pub fn for_disk(&self) -> Settings {
        let mut out = self.clone();
        for p in &mut out.llm_profiles {
            if p.api_key_storage.in_store() {
                p.api_key.clear();
                p.api_key_storage = KeyStorage::Keychain;
            }
        }
        out
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
pub fn endpoint_host(url: &str) -> Option<String> {
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
        assert_eq!(s.cleanup_llm(), LlmProfile::default());
        assert_eq!(s.cleanup_profile, "local");
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
        let mut s: Settings = serde_json::from_str(legacy).expect("legacy settings must load");
        s.normalize();
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

    /// Settings files written before 0.9 have no `subtitles`: on request.
    #[test]
    fn subtitles_default_to_on_request_and_round_trip() {
        assert_eq!(Settings::default().subtitles, SubtitlesMode::OnRequest);
        let old: Settings = serde_json::from_str(r#"{"ui_v2":true}"#).unwrap();
        assert_eq!(old.subtitles, SubtitlesMode::OnRequest);
        let on: Settings = serde_json::from_str(r#"{"subtitles":"always"}"#).unwrap();
        assert_eq!(on.subtitles, SubtitlesMode::Always);
        let json = serde_json::to_value(&on).unwrap();
        assert_eq!(json["subtitles"], "always");
        assert_eq!(
            serde_json::to_value(SubtitlesMode::OnRequest).unwrap(),
            "on_request"
        );
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

    /// The 0.9 meeting preview (#130, E12) is off unless switched on.
    #[test]
    fn meetings_preview_is_off_by_default() {
        let s: Settings = serde_json::from_str(r#"{"hotkey":"Alt+Space"}"#).unwrap();
        assert!(!s.meetings_enabled);
        assert!(!Settings::default().meetings_enabled);
        let on: Settings = serde_json::from_str(r#"{"meetings_enabled":true}"#).unwrap();
        assert!(on.meetings_enabled);
    }

    /// A settings.json exactly as 0.6.3 writes it (every field, pretty
    /// printed), with a non-default OpenAI-compatible cleanup setup.
    const SETTINGS_063: &str = r#"{
  "hotkey": "CommandOrControl+Shift+Space",
  "push_to_talk": true,
  "whisper_model": "ggml-large-v3-turbo-q5_0.bin",
  "engine": "whisper",
  "ollama_url": "http://localhost:8080/v1",
  "ollama_model": "qwen2.5-3b-instruct",
  "cleanup_api": "openai",
  "api_key": "sk-local",
  "cleanup_level": "medium",
  "dictionary": ["Sussurro", "DarumaHQ"],
  "autostart": false,
  "sound_feedback": true,
  "language": "it",
  "output_language": "",
  "snippets": [{"cue": "firma", "text": "Francesco"}],
  "live_preview": true,
  "app_styles": [{"app_match": "slack", "style": "casual", "language": "en"}],
  "models_dir": "",
  "input_device": "",
  "command_hotkey": "CommandOrControl+Alt+Space",
  "whisper_mode": false,
  "stream_injection": false,
  "voice_commands": true,
  "prompt_overrides": {"light": "", "medium": "", "high": ""},
  "history_retention_days": 30,
  "api_enabled": true,
  "api_port": 4525,
  "output_file": ""
}"#;

    /// #119: the four flat cleanup settings become one "Local" profile,
    /// selected for cleanup, with the same server, model, API and key — so
    /// cleanup behaves exactly as before. The file is rewritten without the
    /// old keys, and the other settings survive untouched.
    #[test]
    fn settings_063_migrate_to_one_local_profile() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(&path, SETTINGS_063).unwrap();

        let (s, migrated) = Settings::load_migrating(&path);
        assert!(migrated);
        assert_eq!(
            s.llm_profiles,
            vec![LlmProfile {
                id: "local".into(),
                name: "Local".into(),
                api: CleanupApi::Openai,
                base_url: "http://localhost:8080/v1".into(),
                api_key: "sk-local".into(),
                api_key_storage: KeyStorage::None,
                model: "qwen2.5-3b-instruct".into(),
                external: false,
                context_tokens: 0,
                cleanup_opt_in: String::new(),
            }]
        );
        assert_eq!(s.cleanup_profile, "local");
        assert_eq!(s.cleanup_llm(), s.llm_profiles[0]);
        // Everything else is kept.
        assert_eq!(s.cleanup_level, CleanupLevel::Medium);
        assert_eq!(s.language, "it");
        assert_eq!(s.dictionary, vec!["Sussurro", "DarumaHQ"]);
        assert_eq!(s.snippets.len(), 1);
        assert_eq!(s.app_styles[0].language, "en");
        assert_eq!(s.history_retention_days, 30);
        assert!(s.api_enabled);

        // Saved once: the old keys are gone, the profile is there, and a
        // second load is not a migration and yields the same settings.
        s.save(&path).unwrap();
        let saved = std::fs::read_to_string(&path).unwrap();
        let json: serde_json::Value = serde_json::from_str(&saved).unwrap();
        for key in ["ollama_url", "ollama_model", "cleanup_api", "api_key", "command_hotkey"] {
            assert!(json.get(key).is_none(), "{key} must not be written: {saved}");
        }
        assert_eq!(json["llm_profiles"][0]["base_url"], "http://localhost:8080/v1");
        assert_eq!(json["cleanup_profile"], "local");
        let (again, migrated_again) = Settings::load_migrating(&path);
        assert!(!migrated_again);
        assert_eq!(again, s);
    }

    /// A 0.6 user who pointed cleanup at a remote host gets a profile marked
    /// external (the #92 warning keeps showing).
    #[test]
    fn migrated_remote_endpoint_is_external() {
        let mut s: Settings = serde_json::from_str(
            r#"{"ollama_url":"https://llm.example.com","cleanup_api":"openai"}"#,
        )
        .unwrap();
        assert!(s.normalize());
        assert_eq!(s.llm_profiles.len(), 1);
        assert_eq!(s.llm_profiles[0].name, "Local");
        assert!(s.llm_profiles[0].external);
    }

    /// A pre-0.8 file that never touched cleanup (no legacy keys) migrates to
    /// the default profile — identical to the 0.6 defaults.
    #[test]
    fn legacy_file_without_cleanup_keys_gets_the_default_profile() {
        let mut s: Settings = serde_json::from_str(r#"{"hotkey":"Alt+Space"}"#).unwrap();
        assert!(s.llm_profiles.is_empty(), "no profiles before normalize");
        // Even un-normalized, cleanup resolves to the legacy (default) profile.
        assert_eq!(s.cleanup_llm(), LlmProfile::default());
        assert!(s.normalize());
        assert_eq!(s.llm_profiles, vec![LlmProfile::default()]);
        assert_eq!(s, Settings { hotkey: "Alt+Space".into(), ..Default::default() });
    }

    /// Once profiles exist, stray legacy keys (e.g. a file hand-merged from
    /// an old backup) are ignored and do not create a second profile.
    #[test]
    fn legacy_keys_are_ignored_when_profiles_exist() {
        let mut s: Settings = serde_json::from_str(
            r#"{"ollama_url":"http://old:1","llm_profiles":[{"id":"a","name":"A","api":"ollama","base_url":"http://localhost:11434","api_key":"","model":"m","external":false}],"cleanup_profile":"a"}"#,
        )
        .unwrap();
        assert!(!s.normalize());
        assert_eq!(s.llm_profiles.len(), 1);
        assert_eq!(s.cleanup_llm().base_url, "http://localhost:11434");
        assert!(s.legacy_ollama_url.is_none());
    }

    #[test]
    fn cleanup_llm_picks_the_selected_profile() {
        let work = LlmProfile::new("work", "Work", CleanupApi::Openai, "https://api.example.com/v1", "k", "gpt");
        let mut s = Settings {
            llm_profiles: vec![LlmProfile::default(), work.clone()],
            cleanup_profile: "work".into(),
            ..Default::default()
        };
        assert_eq!(s.cleanup_llm(), work);
        s.cleanup_profile = "local".into();
        assert_eq!(s.cleanup_llm(), LlmProfile::default());
        // A dangling selection: cleanup_llm falls back to the first profile,
        // and normalize() repairs the id.
        s.cleanup_profile = "deleted".into();
        assert_eq!(s.cleanup_llm().id, "local");
        s.normalize();
        assert_eq!(s.cleanup_profile, "local");
    }

    #[test]
    fn normalize_repairs_ids_and_names() {
        let mut s = Settings {
            llm_profiles: vec![
                LlmProfile { id: "a".into(), ..Default::default() },
                LlmProfile { id: "a".into(), name: " ".into(), ..Default::default() },
                LlmProfile { id: "".into(), ..Default::default() },
                LlmProfile { id: "profile-1".into(), ..Default::default() },
            ],
            cleanup_profile: "a".into(),
            ..Default::default()
        };
        assert!(!s.normalize());
        let ids: Vec<_> = s.llm_profiles.iter().map(|p| p.id.as_str()).collect();
        assert_eq!(ids, ["a", "profile-2", "profile-3", "profile-1"]);
        assert_eq!(s.llm_profiles[1].name, "Untitled");
        assert_eq!(s.cleanup_profile, "a");
        // Idempotent.
        let before = s.clone();
        s.normalize();
        assert_eq!(s, before);
    }

    /// A manual `external` override survives save/load: normalize never
    /// re-infers it (only a URL change does, in the editor).
    #[test]
    fn manual_external_override_survives_a_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        let mut lan = LlmProfile::new("lan", "LAN box", CleanupApi::Ollama, "http://192.168.1.5:11434", "", "m");
        assert!(lan.external);
        lan.external = false;
        let s = Settings {
            llm_profiles: vec![LlmProfile::default(), lan],
            cleanup_profile: "lan".into(),
            ..Default::default()
        };
        s.save(&path).unwrap();
        let loaded = Settings::load(&path);
        assert!(!loaded.cleanup_llm().external);
        assert_eq!(loaded, s);
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
