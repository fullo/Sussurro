use crate::settings::{endpoint_host, is_local_endpoint, CleanupApi};
use serde::{Deserialize, Serialize};

/// Server of the default "Local" profile (Ollama on this machine).
pub const DEFAULT_OLLAMA_URL: &str = "http://localhost:11434";
/// Model of the default "Local" profile.
pub const DEFAULT_OLLAMA_MODEL: &str = "llama3.2:3b";
/// Id of the default profile, and of the one migrated from the pre-0.8
/// flat cleanup settings.
pub const LOCAL_PROFILE_ID: &str = "local";

/// A named connection to a chat model (P6). Stored in `settings.json` as
/// part of [`crate::settings::Settings::llm_profiles`].
///
/// Every field has a default so a hand-edited or partial profile never makes
/// the whole settings file unreadable (which would reset every setting).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct LlmProfile {
    /// Stable identifier, referenced by `Settings::cleanup_profile`.
    pub id: String,
    /// Display name ("Local", "Work", …).
    pub name: String,
    /// Which chat API the server speaks: Ollama native or OpenAI-compatible.
    pub api: CleanupApi,
    /// Server address. Ollama: `http://localhost:11434`. OpenAI-compatible:
    /// the base URL, with or without a trailing `/v1`.
    pub base_url: String,
    /// Bearer token for the OpenAI-compatible API. Optional — most local
    /// servers ignore it. Never sent to the Ollama native API.
    ///
    /// In memory (and over IPC to the settings UI) this is the key itself.
    /// On disk it is written only when [`LlmProfile::api_key_storage`] says
    /// it is not in the OS credential store (#159, see `crate::secrets`).
    pub api_key: String,
    /// Where [`LlmProfile::api_key`] is kept (#159). Set by the backend
    /// (`crate::secrets`), never trusted from the UI.
    #[serde(default, skip_serializing_if = "KeyStorage::is_none")]
    pub api_key_storage: KeyStorage,
    /// Model name (Ollama) or id (OpenAI-compatible).
    pub model: String,
    /// Text sent to this profile leaves the machine. Inferred from
    /// `base_url` when the profile is created or its URL changes
    /// ([`infer_external`]) and overridable by hand. Drives the privacy
    /// warning (#92). A profile saved without the field counts as external:
    /// the warning errs on the safe side.
    #[serde(default = "missing_external")]
    pub external: bool,
    /// Context window of the model, in tokens; 0 = unknown, and recipes
    /// then size their chunks for [`DEFAULT_CONTEXT_TOKENS`] (#120). On
    /// Ollama, recipes also request this window (`num_ctx`), since Ollama's
    /// own default can be smaller than the model's.
    #[serde(default)]
    pub context_tokens: u32,
    /// Persistent opt-in for cleanup on an external profile (#122): the
    /// host the user agreed to send dictations and transcriptions to for
    /// cleanup, set in Settings → Cleanup. Empty = not agreed, and cleanup
    /// on this profile keeps the raw text. Bound to the host, so pointing
    /// the profile at another server needs a new opt-in. Unused on local
    /// profiles.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub cleanup_opt_in: String,
    /// The built-in "Local (bundled)" profile (#118): Sussurro's own
    /// `llama-server` sidecar with a small instruct model. Its other fields
    /// are fixed by [`crate::llm::bundled::profile`] (`Settings::normalize`
    /// puts them back), its server address is the sidecar's loopback port
    /// of the moment, and it is never external.
    #[serde(default, skip_serializing_if = "is_false")]
    pub bundled: bool,
}

fn is_false(b: &bool) -> bool {
    !*b
}

/// Where a profile's API key lives (#159).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum KeyStorage {
    /// Not placed anywhere yet: no key, or a clear-text key from a file
    /// written before #159 that the next start moves to the credential store.
    #[default]
    None,
    /// In the OS credential store (macOS Keychain, Windows Credential
    /// Manager, Secret Service); `settings.json` holds no key.
    Keychain,
    /// In clear text in `settings.json`: no credential store worked when it
    /// was saved. The profile editor warns about it, and every start tries
    /// the store again.
    File,
    /// In the credential store, but reading it failed this session (locked
    /// keychain, denied prompt): the key is unknown until the next start.
    /// Written to disk as [`KeyStorage::Keychain`], so the entry is kept.
    Unreadable,
}

impl KeyStorage {
    pub fn is_none(&self) -> bool {
        *self == KeyStorage::None
    }

    /// The key has (or may have) an entry in the credential store.
    pub fn in_store(self) -> bool {
        matches!(self, KeyStorage::Keychain | KeyStorage::Unreadable)
    }
}

/// Context window assumed when a profile doesn't state one: small enough
/// for the 3–4B models Sussurro suggests on a laptop (#120).
pub const DEFAULT_CONTEXT_TOKENS: u32 = 4096;
/// Smallest window recipes plan for: below this a chunk would hold only a
/// few lines next to the instructions.
pub const MIN_CONTEXT_TOKENS: u32 = 1024;

/// `external` absent from a saved profile: assume the text leaves the machine.
fn missing_external() -> bool {
    true
}

impl Default for LlmProfile {
    /// The default "Local" profile: Ollama on this machine.
    fn default() -> Self {
        Self::new(
            LOCAL_PROFILE_ID,
            "Local",
            CleanupApi::Ollama,
            DEFAULT_OLLAMA_URL,
            "",
            DEFAULT_OLLAMA_MODEL,
        )
    }
}

impl LlmProfile {
    /// A profile whose `external` flag is inferred from `base_url`.
    pub fn new(
        id: &str,
        name: &str,
        api: CleanupApi,
        base_url: &str,
        api_key: &str,
        model: &str,
    ) -> Self {
        Self {
            id: id.to_string(),
            name: name.to_string(),
            api,
            base_url: base_url.to_string(),
            api_key: api_key.to_string(),
            api_key_storage: KeyStorage::None,
            model: model.to_string(),
            external: infer_external(base_url),
            context_tokens: 0,
            cleanup_opt_in: String::new(),
            bundled: false,
        }
    }

    /// Host of the server (`api.example.com`), lowercased, without port;
    /// empty when the URL has none.
    pub fn host(&self) -> String {
        endpoint_host(&self.base_url).unwrap_or_default()
    }

    /// [`LlmProfile::host`] for messages: the quoted URL when it has no host.
    pub fn host_label(&self) -> String {
        match self.host() {
            h if h.is_empty() => format!("“{}”", self.base_url.trim()),
            h => h,
        }
    }

    /// Whether cleanup may send text to this profile: always for a local
    /// one; for an external one only with the opt-in given for its current
    /// host (#122). Without it cleanup keeps the raw text — it never falls
    /// back to another profile.
    pub fn cleanup_allowed(&self) -> bool {
        if !self.external {
            return true;
        }
        let host = self.host();
        !host.is_empty() && self.cleanup_opt_in.trim().eq_ignore_ascii_case(&host)
    }

    /// The context window recipes plan for: the stated one (at least
    /// [`MIN_CONTEXT_TOKENS`]), else [`DEFAULT_CONTEXT_TOKENS`].
    pub fn effective_context_tokens(&self) -> u32 {
        match self.context_tokens {
            0 => DEFAULT_CONTEXT_TOKENS,
            n => n.max(MIN_CONTEXT_TOKENS),
        }
    }

    /// Change the server address and re-infer `external` from it — a hand
    /// override holds until the URL changes again.
    pub fn set_base_url(&mut self, base_url: &str) {
        self.base_url = base_url.to_string();
        self.external = infer_external(base_url);
    }
}

/// Whether text sent to `base_url` leaves this machine: anything that is not
/// localhost, a loopback address or an mDNS `.local` host — including
/// unparseable input.
pub fn infer_external(base_url: &str) -> bool {
    !is_local_endpoint(base_url)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_profile_is_local_ollama() {
        let p = LlmProfile::default();
        assert_eq!(p.id, "local");
        assert_eq!(p.name, "Local");
        assert_eq!(p.api, CleanupApi::Ollama);
        assert_eq!(p.base_url, "http://localhost:11434");
        assert_eq!(p.model, "llama3.2:3b");
        assert!(p.api_key.is_empty());
        assert!(!p.external);
        assert_eq!(p.context_tokens, 0);
        assert_eq!(p.effective_context_tokens(), DEFAULT_CONTEXT_TOKENS);
    }

    #[test]
    fn effective_context_has_a_floor_and_a_default() {
        let mut p = LlmProfile { context_tokens: 32_768, ..Default::default() };
        assert_eq!(p.effective_context_tokens(), 32_768);
        p.context_tokens = 100;
        assert_eq!(p.effective_context_tokens(), MIN_CONTEXT_TOKENS);
        // A profile saved before #120 has no field: unknown.
        let old: LlmProfile = serde_json::from_str(r#"{"id":"x","external":false}"#).unwrap();
        assert_eq!(old.context_tokens, 0);
    }

    #[test]
    fn external_is_inferred_from_the_url() {
        for (url, external) in [
            ("http://localhost:11434", false),
            ("http://127.0.0.1:8080/v1", false),
            ("http://[::1]:1234", false),
            ("http://studio.local:1234", false),
            ("https://api.openai.com/v1", true),
            ("http://192.168.1.50:11434", true),
            ("", true),
            ("not a url", true),
        ] {
            let p = LlmProfile::new("x", "X", CleanupApi::Openai, url, "", "m");
            assert_eq!(p.external, external, "{url}");
            assert_eq!(infer_external(url), external, "{url}");
        }
    }

    #[test]
    fn set_base_url_reinfers_and_replaces_a_manual_override() {
        // Manual override on a local URL.
        let mut p = LlmProfile { external: true, ..Default::default() };
        assert!(p.external);
        p.set_base_url("https://llm.example.com");
        assert!(p.external);
        p.external = false; // user vouches for it (e.g. their own LAN box)
        p.set_base_url("http://localhost:8080");
        assert!(!p.external);
        p.set_base_url("http://10.0.0.5:8080");
        assert!(p.external, "a new URL re-infers the flag");
    }

    #[test]
    fn serde_roundtrip_uses_snake_case_api() {
        let p = LlmProfile::new("w", "Work", CleanupApi::Openai, "https://x.example/v1", "k", "gpt");
        let json = serde_json::to_value(&p).unwrap();
        assert_eq!(json["api"], "openai");
        assert_eq!(json["external"], true);
        let back: LlmProfile = serde_json::from_value(json).unwrap();
        assert_eq!(back, p);
    }

    #[test]
    fn host_is_the_lowercased_hostname() {
        let p = LlmProfile::new("w", "W", CleanupApi::Openai, "https://API.Example.com:443/v1", "", "m");
        assert_eq!(p.host(), "api.example.com");
        assert_eq!(p.host_label(), "api.example.com");
        let odd = LlmProfile { base_url: "".into(), ..Default::default() };
        assert_eq!(odd.host(), "");
        assert_eq!(odd.host_label(), "“”");
    }

    #[test]
    fn cleanup_needs_an_opt_in_bound_to_the_host_on_external_profiles() {
        // Local: always.
        assert!(LlmProfile::default().cleanup_allowed());
        let mut p = LlmProfile::new("w", "W", CleanupApi::Openai, "https://api.example.com/v1", "", "m");
        assert!(!p.cleanup_allowed(), "external without opt-in");
        p.cleanup_opt_in = "api.example.com".into();
        assert!(p.cleanup_allowed());
        // Another server voids it: the opt-in is not carried over.
        p.set_base_url("https://llm.other.example/v1");
        assert!(!p.cleanup_allowed());
        // A port or path change on the same host keeps it.
        p.set_base_url("https://api.example.com:8443/openai/v1");
        assert!(p.cleanup_allowed());
        // A local URL marked external by hand needs it too.
        let mut lan = LlmProfile { external: true, ..Default::default() };
        assert!(!lan.cleanup_allowed());
        lan.cleanup_opt_in = "localhost".into();
        assert!(lan.cleanup_allowed());
        // Not serialized when empty; a pre-#122 profile has none.
        let json = serde_json::to_value(LlmProfile::default()).unwrap();
        assert!(json.get("cleanup_opt_in").is_none());
        let old: LlmProfile =
            serde_json::from_str(r#"{"id":"w","base_url":"https://api.example.com","external":true}"#).unwrap();
        assert!(!old.cleanup_allowed());
    }

    /// A hand-edited profile missing fields still loads (so the settings file
    /// is not reset); a missing `external` counts as external.
    #[test]
    fn partial_profile_loads_with_safe_defaults() {
        let p: LlmProfile =
            serde_json::from_str(r#"{"id":"lan","base_url":"http://localhost:1234"}"#).unwrap();
        assert_eq!(p.id, "lan");
        assert_eq!(p.base_url, "http://localhost:1234");
        assert_eq!(p.api, CleanupApi::Ollama);
        assert!(p.external, "missing external must default to external");
        // Default::default() is the local profile, so a container default
        // would make `external` false: a missing flag must not mark a URL as
        // local.
        let remote: LlmProfile =
            serde_json::from_str(r#"{"id":"r","base_url":"https://api.example.com"}"#).unwrap();
        assert!(remote.external, "missing external must default to external");
    }
}
