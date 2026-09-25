//! LLM profiles (0.8, #119): named connections to a chat model that cleanup
//! (and, later, recipes and the Ask panel) pick from. Only two APIs are
//! supported (P6): OpenAI-compatible `/v1` and Ollama native — no
//! provider-specific adapters. The HTTP client lives in `cleanup::ollama`.

pub mod bundled;
pub mod consent;
pub mod profile;

pub use profile::{
    infer_external, KeyStorage, LlmProfile, DEFAULT_CONTEXT_TOKENS, DEFAULT_OLLAMA_MODEL,
    DEFAULT_OLLAMA_URL, LOCAL_PROFILE_ID, MIN_CONTEXT_TOKENS,
};
