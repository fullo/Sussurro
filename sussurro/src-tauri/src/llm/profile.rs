use crate::settings::{is_local_endpoint, CleanupApi};
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
    pub api_key: String,
    /// Model name (Ollama) or id (OpenAI-compatible).
    pub model: String,
    /// Text sent to this profile leaves the machine. Inferred from
    /// `base_url` when the profile is created or its URL changes
    /// ([`infer_external`]) and overridable by hand. Drives the privacy
    /// warning (#92). A profile saved without the field counts as external:
    /// the warning errs on the safe side.
    #[serde(default = "missing_external")]
    pub external: bool,
}

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
            model: model.to_string(),
            external: infer_external(base_url),
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
