use crate::cleanup::prompt::build_messages;
use crate::settings::CleanupLevel;
use anyhow::{Context, Result};
use serde_json::{json, Value};
use std::time::Duration;

/// Clean `transcript` via Ollama. NEVER fails the pipeline: any error
/// (server down, model missing, timeout, empty reply) returns the raw transcript.
pub fn cleanup(
    url: &str,
    model: &str,
    level: &CleanupLevel,
    dictionary: &[String],
    transcript: &str,
) -> String {
    let Some(messages) = build_messages(level, dictionary, transcript) else {
        return transcript.to_string();
    };
    match chat(url, model, &messages) {
        Ok(text) if !text.trim().is_empty() => text.trim().to_string(),
        Ok(_) => transcript.to_string(),
        Err(e) => {
            eprintln!("ollama cleanup failed, using raw transcript: {e:#}");
            transcript.to_string()
        }
    }
}

fn chat(url: &str, model: &str, messages: &[Value]) -> Result<String> {
    let client = reqwest::blocking::Client::builder()
        .connect_timeout(Duration::from_secs(2))
        .timeout(Duration::from_secs(60))
        .build()?;
    let body = json!({
        "model": model,
        "messages": messages,
        "stream": false,
        "options": {"temperature": 0.2}
    });
    let resp: Value = client
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::CleanupLevel;

    #[test]
    fn level_none_skips_network_entirely() {
        // Would panic/hang if it tried the network: URL is unroutable.
        let out = cleanup("http://0.0.0.0:1", "m", &CleanupLevel::None, &[], "um raw text");
        assert_eq!(out, "um raw text");
    }

    #[test]
    fn unreachable_ollama_falls_back_to_raw_transcript() {
        let out = cleanup(
            "http://127.0.0.1:9", // discard port: connection refused instantly
            "llama3.2:3b",
            &CleanupLevel::Light,
            &[],
            "um raw text",
        );
        assert_eq!(out, "um raw text");
    }

    /// Needs a running Ollama with the model pulled. Run manually:
    /// cargo test live_ollama_cleans_text -- --ignored --nocapture
    #[test]
    #[ignore]
    fn live_ollama_cleans_text() {
        let out = cleanup(
            "http://localhost:11434",
            "llama3.2:3b",
            &CleanupLevel::Light,
            &[],
            "um so basically i think uh we should ship it",
        );
        println!("cleaned: {out}");
        assert!(!out.to_lowercase().contains("um"));
    }
}
