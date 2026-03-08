//! AI orchestration: conflict resolution, AST-aware commit-message generation,
//! and agentic code generation.
//!
//! Capabilities:
//! - [`AiResolver`] / [`MockResolver`] / [`LlmResolver`]: three-way conflict resolution.
//! - [`generate_message`]: async call to produce a conventional commit message.
//! - [`generate_code`]: async call to produce generated code for `arc generate`.
//! - [`extract_code_fence`]: shared helper to strip code fences from LLM output.
//! - [`embedding`]: local + API embedding providers for semantic intent indexing.
//! - [`vector_store`]: SQLite-backed cosine-similarity search index.
//!
//! Environment variables (shared by all network functions):
//!
//! | Variable | Default | Purpose |
//! |---|---|---|
//! | `ARC_AI_KEY` | — (required) | Bearer token for the provider |
//! | `ARC_AI_URL` | `https://api.openai.com` | Base URL — any OpenAI-schema endpoint |
//! | `ARC_AI_MODEL` | `gpt-4o-mini` | Model identifier |
//! | `ARC_AI_EMBEDDING_MODEL` | `text-embedding-3-small` | Embedding model (API fallback only) |

pub mod embedding;
pub mod vector_store;

use anyhow::{Context, Result};
use reqwest::Client;
use serde_json::json;

/// Call an OpenAI-schema LLM to produce a concise conventional commit message.
///
/// Reads `ARC_AI_KEY` (required), `ARC_AI_URL` (default: `https://api.openai.com`), and
/// `ARC_AI_MODEL` (default: `gpt-4o-mini`) from the environment.  Compatible with every
/// provider that exposes the `/v1/chat/completions` endpoint: OpenAI, Anthropic (compat),
/// Groq, Together, Azure OpenAI, Ollama, LM Studio, and any local inference server.
///
/// # Errors
/// Returns `Err` if `ARC_AI_KEY` is unset (hard configuration failure) or if the HTTP
/// request itself fails (transient — callers should offer an interactive fallback).
pub async fn generate_message(diff_summary: &str) -> Result<String> {
    let api_key = std::env::var("ARC_AI_KEY")
        .context("ARC_AI_KEY environment variable must be set. Export it before using --auto-msg.")?;

    let base_url = std::env::var("ARC_AI_URL")
        .unwrap_or_else(|_| "https://api.openai.com".to_owned());
    let model = std::env::var("ARC_AI_MODEL")
        .unwrap_or_else(|_| "gpt-4o-mini".to_owned());

    let client = Client::builder()
        .user_agent(concat!("arc-vcs/", env!("CARGO_PKG_VERSION")))
        .build()
        .context("failed to build HTTP client")?;

    let response = client
        .post(format!("{base_url}/v1/chat/completions"))
        .header("Authorization", format!("Bearer {api_key}"))
        .json(&json!({
            "model": model,
            "messages": [
                {
                    "role": "system",
                    "content": "You are an expert systems engineer. Write a single concise conventional commit message for the following AST-level code changes. Output ONLY the commit message text, no quotes, no explanation."
                },
                {
                    "role": "user",
                    "content": diff_summary
                }
            ]
        }))
        .send()
        .await
        .context("Failed to communicate with AI provider")?;

    let data: serde_json::Value = response
        .json()
        .await
        .context("Failed to parse AI provider response as JSON")?;

    let message = data["choices"][0]["message"]["content"]
        .as_str()
        .unwrap_or("refactor: update code")
        .trim()
        .trim_matches('"')
        .to_owned();

    Ok(message)
}

/// Trait for AI-powered conflict resolution.
///
/// Implementations receive the base (LCA) content and the two diverging
/// sides, plus their semantic intents, and produce a merged result.
pub trait AiResolver {
    /// Resolve a three-way conflict, returning the merged content.
    fn resolve(
        &self,
        base: &[u8],
        ours: &[u8],
        theirs: &[u8],
        intent_ours: &str,
        intent_theirs: &str,
    ) -> Result<Vec<u8>, String>;
}

/// Extract content from the first code fence (\`\`\` … \`\`\`) in `text`.
///
/// If the text contains no fence, the entire trimmed string is returned.
/// This is shared by [`LlmResolver`] and `generate_code` so both strip
/// fences consistently.
pub fn extract_code_fence(text: &str) -> String {
    let Some(fence_start) = text.find("```") else {
        return text.trim().to_owned();
    };
    let after_open = &text[fence_start + 3..];
    // Skip optional language tag (e.g. ```rust).
    let after_tag = after_open
        .trim_start_matches(|c: char| c.is_alphanumeric() || c == '-' || c == '_');
    let content_start = match after_tag.find('\n') {
        Some(i) => &after_tag[i + 1..],
        None => after_tag,
    };
    match content_start.rfind("```") {
        Some(end) => content_start[..end].trim_end().to_owned(),
        None => content_start.trim_end().to_owned(),
    }
}

/// Call an OpenAI-schema LLM to produce generated code.
///
/// Same environment variables as [`generate_message`].  The system prompt is
/// tuned for code generation: the model is instructed to output the complete
/// file content inside a single code fence.
pub async fn generate_code(prompt: &str) -> Result<String> {
    let api_key = std::env::var("ARC_AI_KEY")
        .context("ARC_AI_KEY must be set. Export it before using 'arc generate'.")?;
    let base_url = std::env::var("ARC_AI_URL")
        .unwrap_or_else(|_| "https://api.openai.com".to_owned());
    let model = std::env::var("ARC_AI_MODEL")
        .unwrap_or_else(|_| "gpt-4o-mini".to_owned());

    let client = Client::builder()
        .user_agent(concat!("arc-vcs/", env!("CARGO_PKG_VERSION")))
        .build()
        .context("failed to build HTTP client")?;

    let response = client
        .post(format!("{base_url}/v1/chat/completions"))
        .header("Authorization", format!("Bearer {api_key}"))
        .json(&json!({
            "model": model,
            "messages": [
                {
                    "role": "system",
                    "content": "You are an expert software engineer. Generate code based on the user's goal and context. Output ONLY the complete file content inside a single code fence (``` ... ```). No explanation, no commentary."
                },
                {
                    "role": "user",
                    "content": prompt
                }
            ]
        }))
        .send()
        .await
        .context("failed to communicate with AI provider")?;

    let data: serde_json::Value = response
        .json()
        .await
        .context("failed to parse AI provider response as JSON")?;

    let raw = data["choices"][0]["message"]["content"]
        .as_str()
        .unwrap_or("")
        .trim()
        .to_owned();

    Ok(raw)
}

/// An AI resolver backed by any OpenAI-schema LLM.
///
/// Uses `reqwest::blocking` to implement the synchronous [`AiResolver`] trait
/// without requiring a tokio runtime at the call site.  Falls back to
/// [`MockResolver`] gracefully when `ARC_AI_KEY` is absent.
///
/// # Environment variables
///
/// Same as [`generate_message`]: `ARC_AI_KEY`, `ARC_AI_URL`, `ARC_AI_MODEL`.
pub struct LlmResolver {
    /// AI model identifier used in the request body.
    pub model: String,
    api_key: String,
    base_url: String,
}

impl LlmResolver {
    /// Construct from the standard arc AI environment variables.
    ///
    /// Returns `None` if `ARC_AI_KEY` is unset — callers should fall back to
    /// [`MockResolver`] for offline / CI scenarios.
    pub fn from_env() -> Option<Self> {
        let api_key = std::env::var("ARC_AI_KEY").ok()?;
        let base_url = std::env::var("ARC_AI_URL")
            .unwrap_or_else(|_| "https://api.openai.com".to_owned());
        let model = std::env::var("ARC_AI_MODEL")
            .unwrap_or_else(|_| "gpt-4o-mini".to_owned());
        Some(Self { model, api_key, base_url })
    }
}

impl AiResolver for LlmResolver {
    fn resolve(
        &self,
        base: &[u8],
        ours: &[u8],
        theirs: &[u8],
        intent_ours: &str,
        intent_theirs: &str,
    ) -> Result<Vec<u8>, String> {
        let base_str = String::from_utf8_lossy(base);
        let ours_str = String::from_utf8_lossy(ours);
        let theirs_str = String::from_utf8_lossy(theirs);

        let prompt = format!(
            "BASE (common ancestor):\n```\n{base_str}\n```\n\n\
             OURS (intent: {intent_ours}):\n```\n{ours_str}\n```\n\n\
             THEIRS (intent: {intent_theirs}):\n```\n{theirs_str}\n```\n\n\
             Produce ONLY the resolved content inside a single code fence (``` ... ```). \
             No explanation, no commentary."
        );

        let client = reqwest::blocking::Client::builder()
            .user_agent(concat!("arc-vcs/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|e| format!("failed to build HTTP client: {e}"))?;

        let response = client
            .post(format!("{}/v1/chat/completions", self.base_url))
            .header("Authorization", format!("Bearer {}", self.api_key))
            .json(&serde_json::json!({
                "model": self.model,
                "messages": [
                    {
                        "role": "system",
                        "content": "You are an expert software engineer specializing in semantic merge conflict resolution. Always output ONLY the resolved code inside a single code fence."
                    },
                    {
                        "role": "user",
                        "content": prompt
                    }
                ]
            }))
            .send()
            .map_err(|e| format!("HTTP request failed: {e}"))?;

        let data: serde_json::Value = response
            .json()
            .map_err(|e| format!("failed to parse AI response: {e}"))?;

        let raw = data["choices"][0]["message"]["content"]
            .as_str()
            .unwrap_or("")
            .trim()
            .to_owned();

        Ok(extract_code_fence(&raw).into_bytes())
    }
}

/// A deterministic mock resolver for testing.
///
/// Concatenates both sides separated by a newline — just enough to verify
/// the resolution pipeline without an actual AI model.
pub struct MockResolver;

impl AiResolver for MockResolver {
    fn resolve(
        &self,
        _base: &[u8],
        ours: &[u8],
        theirs: &[u8],
        _intent_ours: &str,
        _intent_theirs: &str,
    ) -> Result<Vec<u8>, String> {
        let mut merged = ours.to_vec();
        merged.push(b'\n');
        merged.extend_from_slice(theirs);
        Ok(merged)
    }
}
