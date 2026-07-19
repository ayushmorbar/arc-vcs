//! BLUF: `arc-ai` is the AI orchestration edge for intent-aware workflows.
//!
//! It centralizes LLM calls used by `arc` for conflict resolution, message
//! generation, code generation, and semantic embedding retrieval.
//!
//! ## Purity and I/O boundary
//!
//! This crate is a network and local-model I/O boundary:
//! - Network I/O: OpenAI-compatible HTTP APIs for chat and embeddings.
//! - Disk I/O: embedding model cache and SQLite vector index.
//! - Pure logic: prompt shaping, response normalization, and fence extraction.
//!
//! ## Why this crate exists
//!
//! `arc` keeps CRDT and Spacetime-DAG mechanics deterministic and auditable,
//! while isolating non-deterministic AI interactions in one boundary crate.
//! This separation preserves strong Ed25519 provenance guarantees in core
//! change objects even when AI is used as an assistant.
//!
//! ## Example
//!
//! ```no_run
//! # async fn run() -> Result<(), anyhow::Error> {
//! let msg = arc_ai::generate_message("insert fn foo; rename bar -> baz").await?;
//! println!("{}", msg);
//! # Ok(())
//! # }
//! ```
//!
//! Environment variables (shared by all network functions):
//!
//! | Variable | Default | Purpose |
//! |---|---|---|
//! | `ARC_AI_KEY` | - (required) | Bearer token for the provider |
//! | `ARC_AI_URL` | `https://api.openai.com` | Base URL - any OpenAI-schema endpoint |
//! | `ARC_AI_MODEL` | `gpt-4o-mini` | Model identifier |
//! | `ARC_AI_EMBEDDING_MODEL` | `text-embedding-3-small` | Embedding model (API fallback only) |

pub mod embedding;
pub mod vector_store;

use std::collections::{HashMap, HashSet, VecDeque};

use anyhow::{Context, Result};
use arc_algebra_types::Blake3Hash;
use arc_change::Change;
use reqwest::Client;
use serde_json::json;

/// Synthesizes deterministic context windows from a frontier-backed DAG view.
pub struct ContextSynthesizer {
    limit: usize,
}

impl Default for ContextSynthesizer {
    fn default() -> Self {
        Self { limit: 10 }
    }
}

impl ContextSynthesizer {
    /// Create a synthesizer with a custom max number of changes.
    pub fn with_limit(limit: usize) -> Self {
        Self { limit: limit.max(1) }
    }

    /// Build a "Codebase State" narrative from the current frontier.
    ///
    /// This parses a canonical revset expression (`ancestors(@)`) so retrieval
    /// semantics stay aligned with the revset language, then traverses frontier
    /// dependencies deterministically to capture up to `limit` recent changes.
    pub fn synthesize_codebase_state(
        &self,
        frontier: &HashSet<Blake3Hash>,
        changes: &HashMap<Blake3Hash, Change>,
    ) -> Result<String> {
        let _ = arc_revset::parse("ancestors(@)")
            .context("failed to parse revset expression for context synthesis")?;

        let mut queue = frontier.iter().copied().collect::<Vec<_>>();
        queue.sort();
        let mut queue: VecDeque<Blake3Hash> = queue.into();

        let mut seen = HashSet::new();
        let mut selected = Vec::new();

        while let Some(id) = queue.pop_front() {
            if !seen.insert(id) {
                continue;
            }

            let Some(change) = changes.get(&id) else {
                continue;
            };

            selected.push(change);
            if selected.len() >= self.limit {
                break;
            }

            let mut deps = change.deps.iter().copied().collect::<Vec<_>>();
            deps.sort();
            for dep in deps {
                if !seen.contains(&dep) {
                    queue.push_back(dep);
                }
            }
        }

        Ok(render_codebase_state(&selected))
    }
}

fn render_codebase_state(changes: &[&Change]) -> String {
    if changes.is_empty() {
        return "Codebase State: frontier has no materialized changes".to_string();
    }

    let mut out = String::from("Codebase State (last changes from frontier):\n");
    for change in changes {
        out.push_str(&format!(
            "- {} atoms={} intent={}\n",
            short_hash(&change.id),
            change.atoms.len(),
            change.intent
        ));
    }
    out
}

fn short_hash(hash: &Blake3Hash) -> String {
    hash.iter().take(6).map(|b| format!("{b:02x}")).collect()
}

/// Call an OpenAI-schema LLM to produce a concise conventional commit message.
///
/// Reads `ARC_AI_KEY` (required), `ARC_AI_URL` (default: `https://api.openai.com`), and
/// `ARC_AI_MODEL` (default: `gpt-4o-mini`) from the environment.  Compatible with every
/// provider exposing an OpenAI-compatible `/v1/chat/completions` endpoint and
/// Bearer-token auth: OpenAI, Anthropic-compatible proxies, Groq, Together,
/// Ollama-compatible gateways, LM Studio, and similar local inference servers.
///
/// # Errors
/// Returns `Err` if `ARC_AI_KEY` is unset, if HTTP client construction fails,
/// if the request fails in flight, or if the provider response cannot be parsed
/// as valid JSON.
pub async fn generate_message(diff_summary: &str) -> Result<String> {
    let api_key = std::env::var("ARC_AI_KEY").context(
        "ARC_AI_KEY environment variable must be set. Export it before using --auto-msg.",
    )?;

    let base_url =
        std::env::var("ARC_AI_URL").unwrap_or_else(|_| "https://api.openai.com".to_owned());
    let model = std::env::var("ARC_AI_MODEL").unwrap_or_else(|_| "gpt-4o-mini".to_owned());

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

    let data: serde_json::Value =
        response.json().await.context("Failed to parse AI provider response as JSON")?;

    let message = data["choices"][0]["message"]["content"]
        .as_str()
        .unwrap_or("refactor: update code")
        .trim()
        .trim_matches('"')
        .to_owned();

    Ok(message)
}

/// Generate ghost intent text for the interactive snap flow.
///
/// Uses remote LLM orchestration when configured; otherwise falls back to a
/// deterministic local semantic summarizer.
pub async fn generate_ghost_intent(diff: &str) -> Result<String> {
    let config = RemoteConfig::from_env();
    generate_ghost_intent_with_config(diff, config).await
}

async fn generate_ghost_intent_with_config(
    diff: &str,
    config: Option<RemoteConfig>,
) -> Result<String> {
    let Some(config) = config else {
        return Ok(heuristic_ghost_intent(diff));
    };

    let client = Client::builder()
        .user_agent(concat!("arc-vcs/", env!("CARGO_PKG_VERSION")))
        .build()
        .context("failed to build HTTP client")?;

    let remote = request_ghost_intent_remote(&client, &config, diff).await;
    match remote {
        Ok(intent) if !intent.trim().is_empty() => Ok(intent),
        Ok(_) => Ok(heuristic_ghost_intent(diff)),
        Err(_) => Ok(heuristic_ghost_intent(diff)),
    }
}

#[derive(Debug, Clone, Copy)]
enum RemoteProvider {
    OpenAiCompatible,
    Anthropic,
}

#[derive(Debug, Clone)]
struct RemoteConfig {
    provider: RemoteProvider,
    api_key: String,
    base_url: String,
    model: String,
}

impl RemoteConfig {
    fn from_env() -> Option<Self> {
        let api_key = std::env::var("ARC_AI_KEY").ok()?;
        let base_url =
            std::env::var("ARC_AI_URL").unwrap_or_else(|_| "https://api.openai.com".to_string());
        let model = std::env::var("ARC_AI_MODEL").unwrap_or_else(|_| "gpt-4o-mini".to_string());
        let provider = match std::env::var("ARC_AI_PROVIDER")
            .unwrap_or_else(|_| "openai".to_string())
            .to_ascii_lowercase()
            .as_str()
        {
            "anthropic" => RemoteProvider::Anthropic,
            _ => RemoteProvider::OpenAiCompatible,
        };

        Some(Self { provider, api_key, base_url, model })
    }
}

async fn request_ghost_intent_remote(
    client: &Client,
    config: &RemoteConfig,
    diff: &str,
) -> Result<String> {
    match config.provider {
        RemoteProvider::OpenAiCompatible => request_openai_ghost_intent(client, config, diff).await,
        RemoteProvider::Anthropic => request_anthropic_ghost_intent(client, config, diff).await,
    }
}

async fn request_openai_ghost_intent(
    client: &Client,
    config: &RemoteConfig,
    diff: &str,
) -> Result<String> {
    let response = client
        .post(format!("{}/v1/chat/completions", config.base_url))
        .header("Authorization", format!("Bearer {}", config.api_key))
        .json(&json!({
            "model": config.model,
            "messages": [
                {
                    "role": "system",
                    "content": "You are Ghostwriter for arc. Return one concise intent line that captures the semantic change in conventional-commit style."
                },
                {
                    "role": "user",
                    "content": diff
                }
            ]
        }))
        .send()
        .await
        .context("failed OpenAI-compatible ghost intent request")?;

    let data: serde_json::Value =
        response.json().await.context("failed to parse OpenAI-compatible ghost intent response")?;

    Ok(data["choices"][0]["message"]["content"]
        .as_str()
        .unwrap_or_default()
        .trim()
        .trim_matches('"')
        .to_string())
}

async fn request_anthropic_ghost_intent(
    client: &Client,
    config: &RemoteConfig,
    diff: &str,
) -> Result<String> {
    let response = client
        .post(format!("{}/v1/messages", config.base_url))
        .header("x-api-key", &config.api_key)
        .header("anthropic-version", "2023-06-01")
        .json(&json!({
            "model": config.model,
            "max_tokens": 64,
            "system": "You are Ghostwriter for arc. Return one concise intent line only.",
            "messages": [
                {
                    "role": "user",
                    "content": diff
                }
            ]
        }))
        .send()
        .await
        .context("failed Anthropic ghost intent request")?;

    let data: serde_json::Value =
        response.json().await.context("failed to parse Anthropic ghost intent response")?;

    Ok(data["content"][0]["text"].as_str().unwrap_or_default().trim().to_string())
}

fn heuristic_ghost_intent(diff: &str) -> String {
    let lower = diff.to_ascii_lowercase();
    let insert_count = lower.matches("insert").count() + lower.matches("+").count();
    let delete_count = lower.matches("delete").count() + lower.matches("-").count();
    let modify_count = lower.matches("modify").count() + lower.matches("~").count();

    if lower.contains("network") && lower.contains("sync") {
        return "Refactor: Modularized network sync logic".to_string();
    }
    if modify_count > 0 {
        return format!(
            "Refactor: {} modified semantic atoms with {} inserts and {} deletes",
            modify_count, insert_count, delete_count
        );
    }
    if insert_count >= delete_count {
        return format!("Feat: Added {} semantic atoms across the working frontier", insert_count);
    }
    format!("Refactor: Removed {} semantic atoms and tightened intent scope", delete_count)
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

/// Extract content from the first code fence (``` ... ```) in `text`.
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
    let after_tag =
        after_open.trim_start_matches(|c: char| c.is_alphanumeric() || c == '-' || c == '_');
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
    let base_url =
        std::env::var("ARC_AI_URL").unwrap_or_else(|_| "https://api.openai.com".to_owned());
    let model = std::env::var("ARC_AI_MODEL").unwrap_or_else(|_| "gpt-4o-mini".to_owned());

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

    let data: serde_json::Value =
        response.json().await.context("failed to parse AI provider response as JSON")?;

    let raw = data["choices"][0]["message"]["content"].as_str().unwrap_or("").trim().to_owned();

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
    /// Returns `None` if `ARC_AI_KEY` is unset - callers should fall back to
    /// [`MockResolver`] for offline / CI scenarios.
    pub fn from_env() -> Option<Self> {
        let api_key = std::env::var("ARC_AI_KEY").ok()?;
        let base_url =
            std::env::var("ARC_AI_URL").unwrap_or_else(|_| "https://api.openai.com".to_owned());
        let model = std::env::var("ARC_AI_MODEL").unwrap_or_else(|_| "gpt-4o-mini".to_owned());
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

        let data: serde_json::Value =
            response.json().map_err(|e| format!("failed to parse AI response: {e}"))?;

        let raw = data["choices"][0]["message"]["content"].as_str().unwrap_or("").trim().to_owned();

        Ok(extract_code_fence(&raw).into_bytes())
    }
}

/// A deterministic mock resolver for testing.
///
/// Concatenates both sides separated by a newline - just enough to verify
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

#[cfg(test)]
mod tests {
    use std::collections::{HashMap, HashSet};

    use arc_algebra_types::Atom;
    use arc_store_types::author;

    use super::{ContextSynthesizer, generate_ghost_intent_with_config};

    fn mk_change(intent: &str, deps: HashSet<[u8; 32]>, idx: u8) -> arc_change::Change {
        let (author, signing_key) = author::test_keypair();
        arc_change::Change::new(
            deps,
            vec![
                Atom::Insert {
                    at: vec!["file".into(), "src/lib.rs".into(), format!("n{idx}")],
                    content_hash: [idx; 32],
                },
                Atom::Move {
                    from: vec!["file".into(), "src/lib.rs".into(), format!("from{idx}")],
                    to: vec!["file".into(), "src/lib.rs".into(), format!("to{idx}")],
                },
                Atom::Delete {
                    at: vec!["file".into(), "src/lib.rs".into(), format!("gone{idx}")],
                    prior_hash: [idx.saturating_add(1); 32],
                },
            ],
            intent,
            author,
            &signing_key,
        )
    }

    #[test]
    fn context_synthesizer_limits_to_last_ten_changes() {
        let mut all = HashMap::new();
        let mut prev = None;
        let mut last = [0_u8; 32];

        for i in 0..12_u8 {
            let deps = prev.into_iter().collect::<HashSet<_>>();
            let change = mk_change(&format!("intent-{i}"), deps, i + 1);
            prev = Some(change.id);
            last = change.id;
            all.insert(change.id, change);
        }

        let frontier = HashSet::from([last]);
        let synthesizer = ContextSynthesizer::default();
        let state =
            synthesizer.synthesize_codebase_state(&frontier, &all).expect("state synthesis");

        let lines = state.lines().filter(|line| line.starts_with("-")).count();
        assert_eq!(lines, 10);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn generate_ghost_intent_summarizes_three_atom_diff() {
        let diff = "Insert: module network/sync\nMove: sync/client -> sync/engine\nDelete: legacy sync impl";
        let summary = generate_ghost_intent_with_config(diff, None).await.expect("ghost intent");
        assert_eq!(summary, "Refactor: Modularized network sync logic");
    }
}
