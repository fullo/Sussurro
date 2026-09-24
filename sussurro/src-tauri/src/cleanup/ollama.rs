use crate::cleanup::prompt::build_messages;
use crate::llm::LlmProfile;
use crate::settings::CleanupApi;
use anyhow::{Context, Result};
use serde_json::{json, Value};
use std::time::Duration;

/// Clean `transcript` on the settings' cleanup profile
/// ([`crate::settings::Settings::cleanup_llm`]), driven by the user settings
/// (level, dictionary, translation, voice commands). NEVER fails the
/// pipeline: any error (server down, model missing, timeout, empty reply)
/// returns the raw transcript.
pub fn cleanup(
    settings: &crate::settings::Settings,
    style: Option<&str>,
    transcript: &str,
) -> String {
    let Some(messages) = build_messages(settings, style, transcript) else {
        return transcript.to_string();
    };
    run_cleanup(settings, &messages, transcript)
}

/// Chunked cleanup for the long-form engine (#113): clean one segment with
/// the previous segment as read-only context. Same guarantees as
/// [`cleanup`] — never fails, falls back to the raw segment.
pub fn cleanup_with_context(
    settings: &crate::settings::Settings,
    previous: Option<&str>,
    transcript: &str,
) -> String {
    let Some(messages) =
        crate::cleanup::prompt::build_messages_with_context(settings, previous, transcript)
    else {
        return transcript.to_string();
    };
    run_cleanup(settings, &messages, transcript)
}

/// Send the messages and apply the fallbacks shared by every cleanup: any
/// error, an empty reply or a hallucinated one returns `transcript`.
///
/// The privacy gate for cleanup (#122) sits here, the one place every
/// cleanup goes through (hotkey dictation, streaming, the long-form engine,
/// `/clean`, re-clean, translate): on an external profile without the
/// user's opt-in nothing is sent and the raw text comes back. There is no
/// fallback to another profile.
fn run_cleanup(
    settings: &crate::settings::Settings,
    messages: &[Value],
    transcript: &str,
) -> String {
    let profile = settings.cleanup_llm();
    if !profile.cleanup_allowed() {
        eprintln!(
            "cleanup: “{}” is external and not enabled for cleanup — keeping the raw text, nothing sent",
            profile.name
        );
        return transcript.to_string();
    }
    match chat(&profile, messages) {
        Ok(text) if !text.trim().is_empty() => {
            let cleaned = text.trim().to_string();
            // Small models sometimes ANSWER short dictations instead of
            // cleaning them; keep the raw transcript when the output doesn't
            // derive from the input. Translation changes every word, so the
            // guard only runs when no translation is requested.
            if crate::cleanup::prompt::output_language_name(&settings.output_language).is_none()
                && crate::cleanup::prompt::looks_hallucinated(
                    &settings.cleanup_level,
                    transcript,
                    &cleaned,
                )
            {
                eprintln!("cleanup output unrelated to transcript — keeping raw");
                return transcript.to_string();
            }
            cleaned
        }
        Ok(_) => transcript.to_string(),
        Err(e) => {
            eprintln!("LLM cleanup failed, using raw transcript: {e:#}");
            transcript.to_string()
        }
    }
}

/// Model ids available on a profile's server. Ollama: GET /api/tags.
/// OpenAI-compatible: GET /v1/models. Also the profile editor's "test
/// connection".
pub fn list_models(profile: &LlmProfile) -> Result<Vec<String>> {
    // The bundled profile (#118): its one model once the sidecar and the
    // file are there — without starting the server.
    if profile.bundled {
        return crate::llm::bundled::list_models();
    }
    match profile.api {
        CleanupApi::Ollama => list_models_ollama(&profile.base_url),
        CleanupApi::Openai => list_models_openai(&profile.base_url, &profile.api_key),
    }
}

fn http_client(timeout_secs: u64) -> Result<reqwest::blocking::Client> {
    Ok(reqwest::blocking::Client::builder()
        .connect_timeout(Duration::from_secs(2))
        .timeout(Duration::from_secs(timeout_secs))
        .build()?)
}

fn list_models_ollama(url: &str) -> Result<Vec<String>> {
    let resp: Value = http_client(5)?
        .get(format!("{}/api/tags", url.trim_end_matches('/')))
        .send()
        .context("ollama not reachable")?
        .error_for_status()?
        .json()
        .context("unexpected /api/tags response")?;
    Ok(resp["models"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|m| m["name"].as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default())
}

fn list_models_openai(url: &str, api_key: &str) -> Result<Vec<String>> {
    let mut req = http_client(5)?.get(format!("{}/v1/models", openai_base(url)));
    if !api_key.is_empty() {
        req = req.bearer_auth(api_key);
    }
    let resp: Value = req
        .send()
        .context("OpenAI-compatible server not reachable")?
        .error_for_status()?
        .json()
        .context("unexpected /v1/models response")?;
    Ok(resp["data"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|m| m["id"].as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default())
}

/// Per-call knobs of a chat completion. [`ChatOptions::default`] is what
/// cleanup uses: a 60 s budget, the server's own context window.
#[derive(Debug, Clone, PartialEq)]
pub struct ChatOptions {
    /// Whole-request timeout (a recipe step on a laptop CPU can take minutes).
    pub timeout_secs: u64,
    /// Ollama only: the context window to load the model with (`num_ctx`).
    /// Ignored by the OpenAI-compatible API, where the server decides.
    pub num_ctx: Option<u32>,
    pub temperature: f32,
}

impl Default for ChatOptions {
    fn default() -> Self {
        Self {
            timeout_secs: 60,
            num_ctx: None,
            temperature: 0.2,
        }
    }
}

/// One non-streaming chat completion on a profile, dispatched to its API.
pub fn chat(profile: &LlmProfile, messages: &[Value]) -> Result<String> {
    chat_with(profile, messages, &ChatOptions::default())
}

/// [`chat`] with explicit [`ChatOptions`] (recipes, #120).
pub fn chat_with(profile: &LlmProfile, messages: &[Value], opts: &ChatOptions) -> Result<String> {
    if profile.bundled {
        return chat_bundled(crate::llm::bundled::global(), messages, opts);
    }
    match profile.api {
        CleanupApi::Ollama => chat_ollama(&profile.base_url, &profile.model, messages, opts),
        CleanupApi::Openai => chat_openai(
            &profile.base_url,
            &profile.model,
            &profile.api_key,
            messages,
            opts,
        ),
    }
}

fn chat_ollama(url: &str, model: &str, messages: &[Value], opts: &ChatOptions) -> Result<String> {
    let mut options = json!({"temperature": opts.temperature});
    if let Some(n) = opts.num_ctx {
        options["num_ctx"] = json!(n);
    }
    let body = json!({
        "model": model,
        "messages": messages,
        "stream": false,
        "options": options
    });
    let resp: Value = http_client(opts.timeout_secs)?
        .post(format!("{}/api/chat", url.trim_end_matches('/')))
        .json(&body)
        .send()
        .context("ollama request failed")?
        .error_for_status()
        .context("ollama returned an error status")?
        .json()
        .context("ollama response was not JSON")?;
    Ok(resp["message"]["content"]
        .as_str()
        .unwrap_or_default()
        .to_string())
}

/// The "Local (bundled)" profile's chat (#118): an OpenAI-compatible
/// request to the `llama-server` sidecar that `llm` runs (started on first
/// use; see [`crate::llm::bundled`]), never through a proxy.
pub fn chat_bundled(
    llm: &crate::llm::bundled::BundledLlm,
    messages: &[Value],
    opts: &ChatOptions,
) -> Result<String> {
    let client = reqwest::blocking::Client::builder()
        .no_proxy() // loopback: never through a system proxy
        .connect_timeout(Duration::from_secs(2))
        .timeout(Duration::from_secs(opts.timeout_secs))
        .build()?;
    llm.with_server(|url| {
        chat_openai_with(
            &client,
            url,
            crate::llm::bundled::MODEL_ID,
            "",
            messages,
            opts,
        )
    })
}

fn chat_openai(
    url: &str,
    model: &str,
    api_key: &str,
    messages: &[Value],
    opts: &ChatOptions,
) -> Result<String> {
    chat_openai_with(&http_client(opts.timeout_secs)?, url, model, api_key, messages, opts)
}

fn chat_openai_with(
    client: &reqwest::blocking::Client,
    url: &str,
    model: &str,
    api_key: &str,
    messages: &[Value],
    opts: &ChatOptions,
) -> Result<String> {
    let body = json!({
        "model": model,
        "messages": messages,
        "stream": false,
        "temperature": opts.temperature
    });
    let mut req = client
        .post(format!("{}/v1/chat/completions", openai_base(url)))
        .json(&body);
    if !api_key.is_empty() {
        req = req.bearer_auth(api_key);
    }
    let resp: Value = req
        .send()
        .context("OpenAI-compatible request failed")?
        .error_for_status()
        .context("OpenAI-compatible server returned an error status")?
        .json()
        .context("OpenAI-compatible response was not JSON")?;
    Ok(resp["choices"][0]["message"]["content"]
        .as_str()
        .unwrap_or_default()
        .to_string())
}

/// Normalize a user-entered base URL for the OpenAI-compatible API: drop a
/// trailing slash and an optional trailing `/v1`, so both `http://host:8080`
/// and `http://host:8080/v1` resolve to the same `/v1/...` endpoints.
fn openai_base(url: &str) -> String {
    let u = url.trim().trim_end_matches('/');
    u.strip_suffix("/v1").unwrap_or(u).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::{CleanupApi, CleanupLevel};

    fn profile(api: CleanupApi, url: &str) -> LlmProfile {
        LlmProfile::new("t", "Test", api, url, "", "llama3.2:3b")
    }

    /// Settings whose cleanup profile is `api` at `url`.
    fn cfg_api(level: CleanupLevel, api: CleanupApi, url: &str) -> crate::settings::Settings {
        crate::settings::Settings {
            cleanup_level: level,
            llm_profiles: vec![crate::llm::LlmProfile::default(), profile(api, url)],
            cleanup_profile: "t".into(),
            voice_commands: false,
            ..Default::default()
        }
    }

    fn cfg(level: CleanupLevel, url: &str) -> crate::settings::Settings {
        cfg_api(level, CleanupApi::Ollama, url)
    }

    #[test]
    fn level_none_skips_network_entirely() {
        // Would panic/hang if it tried the network: URL is unroutable.
        let out = cleanup(&cfg(CleanupLevel::None, "http://0.0.0.0:1"), None, "um raw text");
        assert_eq!(out, "um raw text");
    }

    #[test]
    fn unreachable_ollama_falls_back_to_raw_transcript() {
        // Discard port: connection refused instantly.
        let out = cleanup(&cfg(CleanupLevel::Light, "http://127.0.0.1:9"), None, "um raw text");
        assert_eq!(out, "um raw text");
    }

    #[test]
    fn chunked_cleanup_falls_back_to_the_raw_segment() {
        let s = cfg(CleanupLevel::Light, "http://127.0.0.1:9");
        assert_eq!(cleanup_with_context(&s, Some("before"), "um segment"), "um segment");
        let none = cfg(CleanupLevel::None, "http://0.0.0.0:1");
        assert_eq!(cleanup_with_context(&none, None, "um segment"), "um segment");
    }

    #[test]
    fn list_models_errors_when_unreachable() {
        assert!(list_models(&profile(CleanupApi::Ollama, "http://127.0.0.1:9")).is_err());
        assert!(list_models(&profile(CleanupApi::Openai, "http://127.0.0.1:9")).is_err());
    }

    /// A loopback server that counts connections and drops each one (the
    /// cleanup then fails fast and falls back to the raw text).
    fn counting_server() -> (String, std::sync::Arc<std::sync::atomic::AtomicUsize>) {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
        let hits = std::sync::Arc::new(AtomicUsize::new(0));
        let h = hits.clone();
        std::thread::spawn(move || {
            for conn in listener.incoming() {
                h.fetch_add(1, Ordering::SeqCst);
                drop(conn);
            }
        });
        (url, hits)
    }

    /// #122: cleanup on an external profile without the opt-in sends
    /// nothing and keeps the raw text — and never falls back to the other
    /// (local) profile. With the opt-in for its host it does send.
    #[test]
    fn external_cleanup_needs_the_opt_in_and_never_falls_back() {
        use std::sync::atomic::Ordering;
        let (external_url, external_hits) = counting_server();
        let (local_url, local_hits) = counting_server();
        let mut ext = profile(CleanupApi::Openai, &external_url);
        ext.external = true; // e.g. a LAN box, or a URL marked by hand
        let local = LlmProfile::new("local", "Local", CleanupApi::Ollama, &local_url, "", "m");
        let mut s = crate::settings::Settings {
            cleanup_level: CleanupLevel::Light,
            llm_profiles: vec![local, ext],
            cleanup_profile: "t".into(),
            voice_commands: false,
            ..Default::default()
        };
        assert!(s.cleanup_blocked() && !s.cleanup_sends_externally());
        assert_eq!(cleanup(&s, None, "um raw text"), "um raw text");
        assert_eq!(cleanup_with_context(&s, Some("before"), "um segment"), "um segment");
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(external_hits.load(Ordering::SeqCst), 0, "nothing may reach the external host");
        assert_eq!(local_hits.load(Ordering::SeqCst), 0, "no fallback to the local profile");

        // Opted in for its host: cleanup goes there (and only there).
        s.llm_profiles[1].cleanup_opt_in = "127.0.0.1".into();
        assert!(!s.cleanup_blocked() && s.cleanup_sends_externally());
        assert_eq!(cleanup(&s, None, "um raw text"), "um raw text");
        std::thread::sleep(Duration::from_millis(100));
        assert!(external_hits.load(Ordering::SeqCst) >= 1);
        assert_eq!(local_hits.load(Ordering::SeqCst), 0);

        // Cleanup None (no translation) sends nothing either way.
        s.cleanup_level = CleanupLevel::None;
        assert!(!s.cleanup_blocked() && !s.cleanup_sends_externally());
    }

    #[test]
    fn unreachable_openai_falls_back_to_raw_transcript() {
        let s = cfg_api(CleanupLevel::Light, CleanupApi::Openai, "http://127.0.0.1:9");
        assert_eq!(cleanup(&s, None, "um raw text"), "um raw text");
    }


    #[test]
    fn openai_base_strips_trailing_slash_and_v1() {
        assert_eq!(openai_base("http://h:8080"), "http://h:8080");
        assert_eq!(openai_base("http://h:8080/"), "http://h:8080");
        assert_eq!(openai_base("http://h:8080/v1"), "http://h:8080");
        assert_eq!(openai_base("http://h:8080/v1/"), "http://h:8080");
    }

    /// Needs a running Ollama. Run manually:
    /// cargo test live_list_models -- --ignored --nocapture
    #[test]
    #[ignore]
    fn live_list_models() {
        let models = list_models(&profile(CleanupApi::Ollama, "http://localhost:11434")).unwrap();
        println!("models: {models:?}");
        assert!(models.iter().any(|m| m.starts_with("llama3.2")));
    }

    /// The reported regression: a short Italian dictation must never come
    /// back as a conversational REPLY ("Va bene, stiamo per iniziare.").
    /// Faithful cleanup or guarded fallback to raw are both acceptable.
    /// Needs a running Ollama. Run manually:
    /// cargo test live_short_italian -- --ignored --nocapture
    #[test]
    #[ignore]
    fn live_short_italian_is_not_answered() {
        for _ in 0..5 {
            let out = cleanup(
                &cfg(CleanupLevel::Light, "http://localhost:11434"),
                None,
                "Proviamo l'audio. Va.",
            );
            println!("cleaned: {out}");
            assert!(out.to_lowercase().contains("audio"), "reply-like output: {out}");
        }
    }

    /// Regression: a single Italian few-shot example made the 3B model
    /// TRANSLATE English dictations into Italian (the guard then reverted to
    /// raw, silently disabling cleanup for English). With the bilingual
    /// examples the output must stay English AND be cleaned.
    /// Needs a running Ollama. Run manually:
    /// cargo test live_english_stays_english -- --ignored --nocapture
    #[test]
    #[ignore]
    fn live_english_stays_english() {
        for _ in 0..3 {
            let out = cleanup(
                &cfg(CleanupLevel::Light, "http://localhost:11434"),
                None,
                "um so this is uh another test right after",
            );
            println!("cleaned: {out}");
            assert!(out.to_lowercase().contains("test"), "unrelated output: {out}");
            assert!(!out.to_lowercase().contains("questo"), "translated to Italian: {out}");
            // The guard falling back to raw would leave the fillers in.
            assert!(!out.to_lowercase().contains("um"), "cleanup did not run: {out}");
        }
    }

    /// #119: the same local Ollama driven through both profile APIs — its
    /// native `/api/chat` and its OpenAI-compatible `/v1`. Needs a running
    /// Ollama with llama3.2:3b pulled. Run manually:
    /// cargo test live_profiles_clean_via_both_apis -- --ignored --nocapture
    #[test]
    #[ignore]
    fn live_profiles_clean_via_both_apis() {
        for (api, url) in [
            (CleanupApi::Ollama, "http://localhost:11434"),
            (CleanupApi::Openai, "http://localhost:11434/v1"),
        ] {
            let p = profile(api.clone(), url);
            let models = list_models(&p).unwrap();
            assert!(models.iter().any(|m| m.starts_with("llama3.2")), "{api:?}: {models:?}");
            let out = cleanup(
                &cfg_api(CleanupLevel::Light, api.clone(), url),
                None,
                "um so basically i think uh we should ship it",
            );
            println!("{api:?}: {out}");
            assert!(!out.to_lowercase().contains(" uh "), "{api:?} did not clean: {out}");
        }
    }

    /// Needs a running Ollama with the model pulled. Run manually:
    /// cargo test live_ollama_cleans_text -- --ignored --nocapture
    #[test]
    #[ignore]
    fn live_ollama_cleans_text() {
        let out = cleanup(
            &cfg(CleanupLevel::Light, "http://localhost:11434"),
            None,
            "um so basically i think uh we should ship it",
        );
        println!("cleaned: {out}");
        assert!(!out.to_lowercase().contains("um"));
    }
}
