//! AI orchestration: conflict resolution and AST-aware commit message generation.
//!
//! Two capabilities live here:
//! - [`AiResolver`] / [`MockResolver`]: deterministic CRDT conflict resolution (testable without a network).
//! - [`generate_message`]: async call to any OpenAI-schema LLM endpoint (local or cloud) that turns
//!   a human-readable AST-atom diff summary into a conventional commit message.
//!
//! The three environment variables that control [`generate_message`]:
//!
//! | Variable | Default | Purpose |
//! |---|---|---|
//! | `ARC_AI_KEY` | — (required) | Bearer token for the provider |
//! | `ARC_AI_URL` | `https://api.openai.com` | Base URL — any OpenAI-schema endpoint |
//! | `ARC_AI_MODEL` | `gpt-4o-mini` | Model identifier |

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
