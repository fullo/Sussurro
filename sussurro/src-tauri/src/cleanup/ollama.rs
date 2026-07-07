use crate::cleanup::prompt::build_messages;
use crate::settings::CleanupApi;
use anyhow::{Context, Result};
use serde_json::{json, Value};
use std::time::Duration;

/// Clean `transcript` via Ollama, driven by the user settings (level,
/// dictionary, translation, voice commands). NEVER fails the pipeline: any
/// error (server down, model missing, timeout, empty reply) returns the raw
/// transcript.
pub fn cleanup(
    settings: &crate::settings::Settings,
    style: Option<&str>,
    transcript: &str,
) -> String {
    let Some(messages) = build_messages(settings, style, transcript) else {
        return transcript.to_string();
    };
    match chat(settings, &messages) {
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
            eprintln!("ollama cleanup failed, using raw transcript: {e:#}");
            transcript.to_string()
        }
    }
}

/// Model ids available on the configured cleanup backend. Ollama:
/// GET /api/tags. OpenAI-compatible: GET /v1/models.
pub fn list_models(settings: &crate::settings::Settings) -> Result<Vec<String>> {
    match settings.cleanup_api {
        CleanupApi::Ollama => list_models_ollama(&settings.ollama_url),
        CleanupApi::Openai => list_models_openai(&settings.ollama_url, &settings.api_key),
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

/// Command mode: apply a spoken instruction to the selected text. Unlike
/// cleanup(), errors propagate — silently pasting the untouched selection
/// back would look like success.
pub fn command_edit(
    settings: &crate::settings::Settings,
    instruction: &str,
    text: &str,
) -> Result<String> {
    let system = "You edit text following a spoken instruction. Apply the instruction to the \
                  text and output only the resulting text, with no preamble, quotes, or \
                  commentary. If the instruction asks a question about the text, output only \
                  the answer.";
    let messages = vec![
        json!({"role": "system", "content": system}),
        json!({"role": "user", "content": format!("Instruction: {instruction}\n\nText:\n{text}")}),
    ];
    let out = chat(settings, &messages)?;
    anyhow::ensure!(!out.trim().is_empty(), "empty response from the cleanup model");
    Ok(out.trim().to_string())
}

/// One non-streaming chat completion, dispatched to the configured backend.
fn chat(settings: &crate::settings::Settings, messages: &[Value]) -> Result<String> {
    match settings.cleanup_api {
        CleanupApi::Ollama => chat_ollama(&settings.ollama_url, &settings.ollama_model, messages),
        CleanupApi::Openai => chat_openai(
            &settings.ollama_url,
            &settings.ollama_model,
            &settings.api_key,
            messages,
        ),
    }
}

fn chat_ollama(url: &str, model: &str, messages: &[Value]) -> Result<String> {
    let body = json!({
        "model": model,
        "messages": messages,
        "stream": false,
        "options": {"temperature": 0.2}
    });
    let resp: Value = http_client(60)?
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

fn chat_openai(url: &str, model: &str, api_key: &str, messages: &[Value]) -> Result<String> {
    let body = json!({
        "model": model,
        "messages": messages,
        "stream": false,
        "temperature": 0.2
    });
    let mut req = http_client(60)?
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

    fn cfg(level: CleanupLevel, url: &str) -> crate::settings::Settings {
        crate::settings::Settings {
            cleanup_level: level,
            ollama_url: url.into(),
            voice_commands: false,
            ..Default::default()
        }
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
    fn list_models_errors_when_unreachable() {
        assert!(list_models(&cfg(CleanupLevel::Light, "http://127.0.0.1:9")).is_err());
    }

    #[test]
    fn unreachable_openai_falls_back_to_raw_transcript() {
        let mut s = cfg(CleanupLevel::Light, "http://127.0.0.1:9");
        s.cleanup_api = CleanupApi::Openai;
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
        let models = list_models(&cfg(CleanupLevel::Light, "http://localhost:11434")).unwrap();
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
