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
    /// Qwen3-ASR 1.7B Q8 in the bundled `llama-server` sidecar (#117):
    /// optional, never the default (#152) — no dictionary prompt, and
    /// whisper large-v3-turbo is more accurate on Italian (#109).
    Qwen3Asr,
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

/// Where the first-run onboarding stands (#115). Stored in the settings
/// file, so it is shown once per install.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum Onboarding {
    /// A fresh install (no settings file yet): the full guided setup.
    #[default]
    Welcome,
    /// An existing settings file without the flag — an upgrade from 0.6.x
    /// (or a pre-release 0.7 build): one "What's new" screen instead.
    WhatsNew,
    /// Finished or skipped. Settings → About → "Run the setup again"
    /// reopens the setup without changing this.
    Done,
}

impl Onboarding {
    /// Field-level serde default: a settings file that exists but predates
    /// the flag belongs to a user upgrading. A missing file never reaches
    /// serde ([`Settings::load`] returns [`Settings::default`], `Welcome`).
    fn upgrade() -> Self {
        Onboarding::WhatsNew
    }
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
    /// Show a live partial transcript in the overlay while speaking. On by
    /// default where Whisper runs on the GPU, off on a CPU-only build (#98).
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
    /// Local HTTP API on 127.0.0.1 (applied at startup): the browser
    /// extension's routes, and the scripting routes if [`Self::api_scripting`].
    pub api_enabled: bool,
    pub api_port: u16,
    /// The token-less scripting routes (`/clean`, `/transcribe`, `/history`)
    /// answer (#215); read on every request, so it applies at once. Off on a
    /// new install (pairing the extension turns the API on, not these); a
    /// settings file from before the switch takes `api_enabled`'s value
    /// ([`Settings::load_migrating`]), so existing scripts keep working.
    pub api_scripting: bool,
    /// The archive routes (`/archive/…`, E14, #249) answer; read on every
    /// request, so it applies at once. Off by default. Each request also
    /// needs one of [`Self::archive_tokens`].
    pub api_archive: bool,
    /// Archive API tokens (E14): name, scopes, times and the **SHA-256** of
    /// each token — never the token itself ([`crate::api::tokens`]). Only
    /// the backend changes them (`archive_token_create` / `_revoke`, and the
    /// last-used time); a save from the UI keeps the current ones, and the
    /// UI never receives the hashes ([`Settings::for_ui`]).
    pub archive_tokens: Vec<crate::api::tokens::ArchiveToken>,
    /// Dictate-to-file mode: when set, completed dictations are APPENDED to
    /// this file (note-taking) instead of being pasted into the focused app.
    pub output_file: String,
    /// Archive folder for notes, meetings and transcriptions. Empty = the
    /// default `<Documents>/Sussurro` (see `archive::resolve_archive_dir`).
    pub archive_dir: String,
    /// First-run onboarding (#115): the full setup on a fresh install, a
    /// "What's new" screen on an upgrade, nothing once done. The workspace
    /// is the only UI: a pre-0.7 `ui_v2` key is ignored on load (serde skips
    /// unknown keys) and dropped by the next save.
    #[serde(default = "Onboarding::upgrade")]
    pub onboarding: Onboarding,
    /// Pairing token of the browser extension (E6): `Authorization: Bearer`
    /// on HTTP, `?token=` on the `/live` WebSocket. Empty = not paired yet.
    /// Only the backend sets it ([`Settings::regenerate_extension_token`]);
    /// a save from the UI keeps the current one.
    pub extension_token: String,
    /// Subtitles setting (P7, #133): `transcript.srt` on request (default)
    /// or on every save. Meetings and transcriptions only.
    pub subtitles: SubtitlesMode,
    /// "Save audio" preselected in New (P9, #141): WAV is saved only on
    /// request, so this is off by default. Also applies to runs started
    /// without a New screen (a browser meeting from the extension).
    pub save_audio: bool,
    /// The notice before the first recording of other people (#136) was
    /// acknowledged with "Don't show this again". Per install; false (a
    /// fresh or cleared settings file) shows it again, and so does
    /// Settings → Browser extension → "Show the notice again".
    pub meeting_notice_seen: bool,
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
            live_preview: crate::stt::WHISPER_GPU,
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
            api_scripting: false,
            api_archive: false,
            archive_tokens: Vec::new(),
            output_file: String::new(),
            archive_dir: String::new(),
            onboarding: Onboarding::Welcome,
            extension_token: String::new(),
            subtitles: SubtitlesMode::OnRequest,
            save_audio: false,
            meeting_notice_seen: false,
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
    ///
    /// Also migrated (and saved once): a file without `api_scripting` (#215)
    /// keeps the scripting routes it had — on exactly when the API was on.
    pub fn load_migrating(path: &Path) -> (Self, bool) {
        let Some(json) = std::fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        else {
            return (Self::default(), false);
        };
        let Ok(mut settings) = serde_json::from_value::<Settings>(json.clone()) else {
            return (Self::default(), false);
        };
        let mut migrated = settings.normalize();
        if json.is_object() && json.get("api_scripting").is_none() {
            settings.api_scripting = settings.api_enabled;
            migrated = true;
        }
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
        self.normalize_bundled();

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

    /// The built-in "Local (bundled)" profile (#118): at most one, always in
    /// its fixed shape ([`crate::llm::bundled::profile`]: never external,
    /// no key, the sidecar's model) whatever the file or the UI sent. A user
    /// profile holding its reserved id gets a fresh one, and a cleanup
    /// selection on it follows it — unless the built-in profile is there,
    /// which then owns the id.
    fn normalize_bundled(&mut self) {
        use crate::llm::bundled::{profile, PROFILE_ID};
        let mut kept = false;
        self.llm_profiles
            .retain(|p| !p.bundled || !std::mem::replace(&mut kept, true));
        let has_builtin = kept;
        if let Some(i) = self
            .llm_profiles
            .iter()
            .position(|p| !p.bundled && p.id == PROFILE_ID)
        {
            let taken: std::collections::HashSet<&str> =
                self.llm_profiles.iter().map(|p| p.id.as_str()).collect();
            let fresh = (2..)
                .map(|n| format!("{PROFILE_ID}-{n}"))
                .find(|id| !taken.contains(id.as_str()))
                .expect("an unused id");
            if !has_builtin && self.cleanup_profile == PROFILE_ID {
                self.cleanup_profile = fresh.clone();
            }
            self.llm_profiles[i].id = fresh;
        }
        for p in self.llm_profiles.iter_mut().filter(|p| p.bundled) {
            *p = profile();
        }
    }

    /// Add the built-in "Local (bundled)" profile if it is missing (#118;
    /// builds that ship the sidecar). Never selects it: the user's cleanup
    /// profile stays what it was. Returns whether it was added.
    pub fn ensure_bundled_profile(&mut self) -> bool {
        if self.llm_profiles.iter().any(|p| p.bundled) {
            return false;
        }
        // Frees the reserved id first, keeping the cleanup selection on a
        // user profile that had it.
        self.normalize();
        self.llm_profiles.push(crate::llm::bundled::profile());
        true
    }

    /// The user chose the bundled model for cleanup: add the profile if
    /// needed and select it.
    pub fn use_bundled_for_cleanup(&mut self) {
        self.ensure_bundled_profile();
        self.cleanup_profile = crate::llm::bundled::PROFILE_ID.into();
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

    /// Replace the extension token with a fresh random one (the old pairing
    /// stops working) and return it.
    pub fn regenerate_extension_token(&mut self) -> anyhow::Result<String> {
        self.extension_token = crate::api::auth::generate_token()?;
        Ok(self.extension_token.clone())
    }

    /// The extension token, generated on first use.
    pub fn ensure_extension_token(&mut self) -> anyhow::Result<String> {
        if self.extension_token.trim().is_empty() {
            return self.regenerate_extension_token();
        }
        Ok(self.extension_token.clone())
    }

    /// Save settings as pretty JSON, creating parent directories as needed.
    /// Serialization failures are mapped into the returned `io::Error` instead
    /// of panicking — a settings write must degrade to an error the caller can
    /// report, never take down the app.
    ///
    /// API keys kept in the OS credential store are left out (#159): see
    /// [`Settings::for_disk`].
    ///
    /// The file holds the extension token and any fallback API keys (#215):
    /// it is written atomically (a temp file in the same folder, then a
    /// rename over the old one) and, on Unix, with mode 0600 — so a save
    /// also tightens a file an older version left world-readable.
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let json = serde_json::to_string_pretty(&self.for_disk())
            .map_err(|e| std::io::Error::other(format!("serialize settings: {e}")))?;
        write_private_atomic(path, json.as_bytes())
    }

    /// The settings as the UI gets them: the archive tokens without their
    /// hashes (#249) — the UI lists them by id, name, scopes and times.
    pub fn for_ui(&self) -> Settings {
        let mut out = self.clone();
        for t in &mut out.archive_tokens {
            t.sha256.clear();
        }
        out
    }

    /// Carry over what only the backend changes (the extension token, the
    /// archive tokens) from `current`, the settings in effect: a UI holding
    /// an older copy must not undo a regenerate or bring back a revoked
    /// token.
    pub fn keep_backend_owned(&mut self, current: &Settings) {
        self.extension_token = current.extension_token.clone();
        self.archive_tokens = current.archive_tokens.clone();
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

/// Write `bytes` to `path` atomically, readable by the owner only on Unix
/// (0600; Windows keeps the profile folder's ACL): a temp file next to
/// `path` is written, synced and renamed over it. A failed write leaves the
/// old file as it was and removes the temp file.
pub fn write_private_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "settings.json".into());
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let tmp = path.with_file_name(format!(".{name}.{}.{n}.tmp", std::process::id()));
    let write = || -> std::io::Result<()> {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&tmp)?;
        // The umask can only remove bits, but make the mode explicit anyway.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
        }
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&tmp, path)
    };
    let result = write();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
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
        // Qwen3-ASR is optional, never the default (#152).
        assert_eq!(s.engine, SttEngine::Whisper);
        // Live preview (#98): on where Whisper has a GPU, off on CPU-only.
        assert_eq!(s.live_preview, crate::stt::WHISPER_GPU);
    }

    #[test]
    fn an_existing_settings_file_keeps_its_live_preview_choice() {
        let s: Settings = serde_json::from_str(r#"{"live_preview": true}"#).unwrap();
        assert!(s.live_preview);
        let s: Settings = serde_json::from_str(r#"{"live_preview": false}"#).unwrap();
        assert!(!s.live_preview);
    }

    #[test]
    fn engines_serialize_as_the_frontend_names_them() {
        for (engine, name) in [
            (SttEngine::Whisper, "whisper"),
            (SttEngine::Parakeet, "parakeet"),
            (SttEngine::Qwen3Asr, "qwen3_asr"),
        ] {
            let json = serde_json::to_string(&engine).unwrap();
            assert_eq!(json, format!("\"{name}\""));
            assert_eq!(serde_json::from_str::<SttEngine>(&json).unwrap(), engine);
        }
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

    /// #215: settings.json holds the extension token (and fallback API
    /// keys): owner-only on Unix, also when an older build left it 0644,
    /// written atomically (no temp file left behind).
    #[test]
    fn save_writes_an_owner_only_file_atomically() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(&path, "{}").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        }
        let s = Settings {
            extension_token: "secret".into(),
            ..Default::default()
        };
        s.save(&path).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "{mode:o}");
        }
        assert_eq!(Settings::load(&path).extension_token, "secret");
        s.save(&path).unwrap();
        let names: Vec<String> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, ["settings.json"]);
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
                // An existing file without the flag is an upgrade (#115).
                onboarding: Onboarding::WhatsNew,
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

    /// #138: meetings are always on; a settings.json from the 0.9 preview
    /// still carries `meetings_enabled` (either value). It must load with
    /// the other fields kept and the key must be gone after the next save.
    #[test]
    fn legacy_meetings_enabled_is_ignored_and_dropped_on_save() {
        for flag in ["true", "false"] {
            let legacy = format!(
                r#"{{"hotkey":"Alt+Space","meetings_enabled":{flag},"extension_token":"abc","save_audio":true}}"#
            );
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("settings.json");
            std::fs::write(&path, &legacy).unwrap();
            let loaded = Settings::load(&path);
            assert_eq!(loaded.hotkey, "Alt+Space", "load() must not fall back to defaults");
            assert_eq!(loaded.extension_token, "abc");
            assert!(loaded.save_audio);
            loaded.save(&path).unwrap();
            let saved = std::fs::read_to_string(&path).unwrap();
            assert!(!saved.contains("meetings_enabled"), "{saved}");
            assert_eq!(Settings::load(&path), loaded);
        }
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
        let old: Settings = serde_json::from_str(r#"{"hotkey":"Alt+Space"}"#).unwrap();
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

    /// P9 (#141): audio is never saved by default, including for settings
    /// files written before the option existed.
    #[test]
    fn save_audio_defaults_to_off() {
        assert!(!Settings::default().save_audio);
        let old: Settings = serde_json::from_str(r#"{"hotkey":"Alt+Space"}"#).unwrap();
        assert!(!old.save_audio);
        let on: Settings = serde_json::from_str(r#"{"save_audio":true}"#).unwrap();
        assert!(on.save_audio);
    }

    /// #115: the workspace is the only UI. A settings file from the preview
    /// era (`ui_v2` true or false) still loads — nothing else in it is lost
    /// — and the key is gone after the next save.
    #[test]
    fn ui_v2_key_is_ignored_on_load_and_dropped_on_save() {
        for flag in ["true", "false"] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("settings.json");
            std::fs::write(
                &path,
                format!(r#"{{"hotkey":"Alt+Space","ui_v2":{flag},"archive_dir":"/notes"}}"#),
            )
            .unwrap();
            let s = Settings::load(&path);
            assert_eq!(s.hotkey, "Alt+Space", "ui_v2={flag} must not reset the file");
            assert_eq!(s.archive_dir, "/notes");
            s.save(&path).unwrap();
            let saved = std::fs::read_to_string(&path).unwrap();
            assert!(!saved.contains("ui_v2"), "{saved}");
            assert_eq!(Settings::load(&path).hotkey, "Alt+Space");
        }
    }

    /// #115: the upgrade path from a real 0.6.3 settings.json (same keys
    /// and order as one written by the released app; values anonymised).
    /// It loads without falling back to defaults, becomes the "Local"
    /// profile, gets "What's new", and the migration save (as at startup)
    /// keeps that and drops every legacy key.
    #[test]
    fn a_real_0_6_3_settings_file_upgrades_to_whats_new() {
        let v063 = r#"{
  "hotkey": "CommandOrControl+Shift+Space",
  "push_to_talk": true,
  "whisper_model": "ggml-large-v3-turbo-q5_0.bin",
  "engine": "whisper",
  "ollama_url": "http://192.168.1.10:11434/",
  "ollama_model": "qwen2.5:7b",
  "cleanup_api": "ollama",
  "api_key": "",
  "cleanup_level": "none",
  "dictionary": ["Sussurro"],
  "autostart": false,
  "sound_feedback": true,
  "language": "auto",
  "output_language": "",
  "snippets": [],
  "live_preview": true,
  "app_styles": [],
  "models_dir": "",
  "input_device": "",
  "command_hotkey": "CommandOrControl+Alt+Space",
  "whisper_mode": false,
  "stream_injection": false,
  "voice_commands": true,
  "prompt_overrides": { "light": "", "medium": "", "high": "" },
  "history_retention_days": 0,
  "api_enabled": false,
  "api_port": 4525,
  "output_file": ""
}"#;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(&path, v063).unwrap();
        let (s, migrated) = Settings::load_migrating(&path);
        assert!(migrated);
        assert_eq!(s.onboarding, Onboarding::WhatsNew);
        assert_eq!(s.cleanup_level, CleanupLevel::None, "not the defaults");
        assert_eq!(s.dictionary, vec!["Sussurro".to_string()]);
        assert_eq!(s.cleanup_llm().base_url, "http://192.168.1.10:11434/");
        assert_eq!(s.cleanup_llm().model, "qwen2.5:7b");
        s.save(&path).unwrap();
        let saved = std::fs::read_to_string(&path).unwrap();
        for gone in ["ollama_url", "ollama_model", "command_hotkey", "cleanup_api", "ui_v2"] {
            assert!(!saved.contains(gone), "{gone} in {saved}");
        }
        assert_eq!(Settings::load(&path).onboarding, Onboarding::WhatsNew);
    }

    /// #115: a fresh install gets the full onboarding; an existing file
    /// without the flag (0.6.x, or a 0.7 pre-release) gets "What's new";
    /// the stored value round-trips.
    #[test]
    fn onboarding_is_welcome_when_fresh_and_whats_new_on_upgrade() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        assert_eq!(Settings::load(&path).onboarding, Onboarding::Welcome);
        assert_eq!(Settings::default().onboarding, Onboarding::Welcome);

        std::fs::write(&path, r#"{"hotkey":"Alt+Space"}"#).unwrap();
        assert_eq!(Settings::load(&path).onboarding, Onboarding::WhatsNew);

        // A fresh install that saved settings before finishing the setup
        // still gets it on the next start.
        Settings::default().save(&path).unwrap();
        assert_eq!(Settings::load(&path).onboarding, Onboarding::Welcome);

        let done = Settings {
            onboarding: Onboarding::Done,
            ..Settings::default()
        };
        done.save(&path).unwrap();
        assert_eq!(Settings::load(&path).onboarding, Onboarding::Done);
        assert_eq!(
            serde_json::to_value(Onboarding::WhatsNew).unwrap(),
            "whats_new"
        );
    }

    /// #126: the extension is unpaired until the user pairs it; the token
    /// is generated once and replaced only on request.
    #[test]
    fn extension_is_unpaired_by_default() {
        let s: Settings = serde_json::from_str(r#"{"hotkey":"Alt+Space"}"#).unwrap();
        assert!(s.extension_token.is_empty());
        let mut s = Settings::default();
        let first = s.ensure_extension_token().unwrap();
        assert_eq!(first.len(), 64);
        assert_eq!(s.ensure_extension_token().unwrap(), first, "stable once created");
        let second = s.regenerate_extension_token().unwrap();
        assert_ne!(second, first);
        assert_eq!(s.extension_token, second);
    }

    /// #215: the token-less scripting routes are off on a new install; a
    /// settings file from before the switch keeps what it had (on exactly
    /// when the API was on), and the migrated value is saved once.
    #[test]
    fn scripting_routes_are_opt_in_and_migrated_from_api_enabled() {
        assert!(!Settings::default().api_scripting);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        for (api_enabled, expected) in [(true, true), (false, false)] {
            std::fs::write(&path, format!(r#"{{"api_enabled":{api_enabled},"api_port":4525}}"#)).unwrap();
            let (s, migrated) = Settings::load_migrating(&path);
            assert!(migrated, "the new key must be written once");
            assert_eq!(s.api_scripting, expected);
            s.save(&path).unwrap();
            let (again, migrated_again) = Settings::load_migrating(&path);
            assert!(!migrated_again);
            assert_eq!(again.api_scripting, expected);
        }
        // Once the key exists, it is the user's choice, whatever the API.
        std::fs::write(&path, r#"{"api_enabled":true,"api_scripting":false}"#).unwrap();
        assert!(!Settings::load(&path).api_scripting);
    }

    /// #249: the archive API is off by default, even with the local API and
    /// the scripting routes on; its tokens round-trip as hashes, and a file
    /// from before 0.11 loads with none.
    #[test]
    fn the_archive_api_is_off_by_default_and_tokens_round_trip() {
        use crate::api::tokens::{create, Scope};
        assert!(!Settings::default().api_archive);
        assert!(Settings::default().archive_tokens.is_empty());
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(&path, r#"{"api_enabled":true,"api_scripting":true}"#).unwrap();
        let old = Settings::load(&path);
        assert!(!old.api_archive && old.archive_tokens.is_empty());
        let (stored, new) = create(&[], "ci", &[Scope::Read, Scope::People], chrono::Utc::now()).unwrap();
        let s = Settings {
            api_archive: true,
            archive_tokens: vec![stored.clone()],
            ..Default::default()
        };
        s.save(&path).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(!text.contains(&new.token));
        let back = Settings::load(&path);
        assert!(back.api_archive);
        assert_eq!(back.archive_tokens, vec![stored.clone()]);
        // The UI copy has no hashes; nothing else changes.
        let ui = back.for_ui();
        assert!(ui.archive_tokens[0].sha256.is_empty());
        assert_eq!(ui.archive_tokens[0].name, "ci");
        assert_eq!(back.archive_tokens[0].sha256, stored.sha256, "for_ui leaves the original alone");
    }

    /// #136: the recording notice shows until acknowledged; a settings file
    /// without the key (fresh, cleared, or older) shows it again.
    #[test]
    fn meeting_notice_is_unseen_until_acknowledged() {
        assert!(!Settings::default().meeting_notice_seen);
        let s: Settings = serde_json::from_str(r#"{"hotkey":"Alt+Space"}"#).unwrap();
        assert!(!s.meeting_notice_seen);
        let seen: Settings = serde_json::from_str(r#"{"meeting_notice_seen":true}"#).unwrap();
        assert!(seen.meeting_notice_seen);
        let back: Settings = serde_json::from_str(&serde_json::to_string(&seen).unwrap()).unwrap();
        assert!(back.meeting_notice_seen, "round-trips through settings.json");
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
                bundled: false,
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
        assert_eq!(
            s,
            Settings {
                hotkey: "Alt+Space".into(),
                onboarding: Onboarding::WhatsNew,
                ..Default::default()
            }
        );
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

    // ------------------------------------------ bundled LLM profile (#118) --

    #[test]
    fn the_bundled_profile_is_added_but_never_selected() {
        use crate::llm::bundled::{profile, PROFILE_ID};
        let mut s = Settings::default();
        assert_eq!(s.cleanup_profile, "local");
        assert!(s.ensure_bundled_profile());
        assert_eq!(s.llm_profiles, vec![LlmProfile::default(), profile()]);
        assert_eq!(s.cleanup_profile, "local", "the user's choice stays");
        assert!(!s.ensure_bundled_profile(), "only once");
        assert_eq!(s.llm_profiles.len(), 2);

        // Picking it is the user's explicit action.
        s.use_bundled_for_cleanup();
        assert_eq!(s.cleanup_profile, PROFILE_ID);
        let p = s.cleanup_llm();
        assert!(p.bundled && !p.external && p.cleanup_allowed());
        assert!(!s.cleanup_blocked() && !s.cleanup_sends_externally());

        // Saved and loaded as is; normalize keeps it.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        s.save(&path).unwrap();
        let (back, migrated) = Settings::load_migrating(&path);
        assert!(!migrated);
        assert_eq!(back.llm_profiles, s.llm_profiles);
        assert_eq!(back.cleanup_profile, PROFILE_ID);
    }

    #[test]
    fn use_bundled_adds_it_when_missing() {
        let mut s = Settings::default();
        s.use_bundled_for_cleanup();
        assert_eq!(s.llm_profiles.len(), 2);
        assert_eq!(s.cleanup_llm(), crate::llm::bundled::profile());
    }

    #[test]
    fn normalize_restores_the_bundled_profile_shape() {
        use crate::llm::bundled::{profile, PROFILE_ID};
        // A hand-edited file or a stale UI: other URL, key, model, external,
        // and a duplicate.
        let tampered = LlmProfile {
            base_url: "https://api.example.com/v1".into(),
            api_key: "sk-x".into(),
            model: "gpt".into(),
            external: true,
            name: "Renamed".into(),
            ..profile()
        };
        let dup = LlmProfile { id: "other".into(), ..profile() };
        let mut s = Settings {
            llm_profiles: vec![LlmProfile::default(), tampered, dup],
            cleanup_profile: PROFILE_ID.into(),
            ..Default::default()
        };
        s.normalize();
        assert_eq!(s.llm_profiles, vec![LlmProfile::default(), profile()]);
        assert_eq!(s.cleanup_profile, PROFILE_ID);
        assert!(!s.cleanup_llm().external);
        let before = s.clone();
        s.normalize();
        assert_eq!(s, before, "idempotent");
    }

    #[test]
    fn a_user_profile_with_the_reserved_id_is_renamed_and_keeps_cleanup() {
        use crate::llm::bundled::PROFILE_ID;
        let mine = LlmProfile::new(PROFILE_ID, "Bundled", CleanupApi::Openai, "http://localhost:8080/v1", "", "m");
        let mut s = Settings {
            llm_profiles: vec![LlmProfile::default(), mine.clone()],
            cleanup_profile: PROFILE_ID.into(),
            ..Default::default()
        };
        assert!(s.ensure_bundled_profile());
        let ids: Vec<_> = s.llm_profiles.iter().map(|p| p.id.as_str()).collect();
        assert_eq!(ids, ["local", "bundled-2", PROFILE_ID]);
        assert_eq!(s.cleanup_profile, "bundled-2", "cleanup stays on the user's profile");
        assert_eq!(s.cleanup_llm().base_url, mine.base_url);
        assert!(s.llm_profiles.iter().filter(|p| p.bundled).count() == 1);
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
