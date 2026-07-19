//! `arc generate` — agentic, context-aware code generation.
//!
//! Flow:
//! 1. Read the target file (capped at [`FILE_CONTENT_BUDGET`] chars to stay within any provider's
//!    context window — the architectural guardrail noted in the Phase 42 plan).
//! 2. Query the local intent vector store for the top-3 semantically similar prior changes so the
//!    AI is grounded in this repository's conventions.
//! 3. Build a structured prompt and call `arc_ai::generate_code`.
//! 4. Write the result back to the file.
//! 5. Save a Ghost Node (`PendingAiChange { kind: Generate }`) to `.arc/ai/pending.json`.
//!
//! The user reviews the diff, then runs `arc ai approve` to cryptographically
//! sign and commit the change as `Author::AI { model, human_sponsor }`.

use std::path::Path;

use anyhow::{Context, Result};
use arc_ai::extract_code_fence;

use crate::{
    ai_pending::{PendingAiChange, has_pending_ai, save_pending_ai},
    repo::Repository,
};

/// Maximum characters of file content to include in the prompt.
///
/// Roughly 1 000 tokens at typical code density.  Prevents context-window
/// overflow for large files without requiring Tree-Sitter extraction.
const FILE_CONTENT_BUDGET: usize = 4_000;

/// Run `arc generate --goal <goal> [--file <path>]`.
///
/// Returns `Ok(())` after writing the generated code to disk and staging the
/// Ghost Node.  Returns `Err` if the AI call fails or file I/O fails.
pub fn run(goal: &str, file: Option<&Path>, repo: &mut Repository) -> Result<()> {
    // State Lock: refuse if another AI change is already staged.
    if has_pending_ai(&repo.shared_root) {
        anyhow::bail!(
            "An AI change is already pending approval.\nRun 'arc ai approve' to sign and commit \
             it, or delete '.arc/ai/pending.json' to discard it."
        );
    }

    // ── 1. Read current file content (bounded) ────────────────────────────────
    let (file_context, target_path) = match file {
        Some(p) => {
            let content = std::fs::read_to_string(p)
                .with_context(|| format!("failed to read '{}'", p.display()))?;
            let bounded: String = content.chars().take(FILE_CONTENT_BUDGET).collect();
            let truncated = bounded.len() < content.len();
            let note = if truncated {
                format!(
                    "\n[Note: file truncated to {FILE_CONTENT_BUDGET} chars for context safety]"
                )
            } else {
                String::new()
            };
            let ctx = format!("Current file ({}):\n```\n{bounded}{note}\n```", p.display());
            (ctx, Some(p))
        }
        None => (String::new(), None),
    };

    // ── 2. Retrieve top-3 semantically similar prior intents ─────────────────
    let prior_context = retrieve_prior_context(goal, repo);

    // ── 3. Build the prompt ───────────────────────────────────────────────────
    let prompt = build_prompt(goal, &file_context, &prior_context);

    // ── 4. Call the AI (async, via an explicit runtime) ───────────────────────
    let rt =
        tokio::runtime::Runtime::new().context("failed to start async runtime for arc generate")?;
    let raw_response =
        rt.block_on(arc_ai::generate_code(&prompt)).context("AI code generation failed")?;

    let generated = extract_code_fence(&raw_response);

    // ── 5. Write to disk ──────────────────────────────────────────────────────
    let Some(path) = target_path else {
        // No --file: print to stdout and exit (no Ghost Node needed).
        println!("{generated}");
        return Ok(());
    };

    std::fs::write(path, &generated)
        .with_context(|| format!("failed to write generated code to '{}'", path.display()))?;
    println!("[arc] Code written to '{}'.", path.display());

    // ── 6. Save Ghost Node ────────────────────────────────────────────────────
    let model = std::env::var("ARC_AI_MODEL").unwrap_or_else(|_| "gpt-4o-mini".to_owned());

    let pending = PendingAiChange::new_generate(model, goal.to_owned(), vec![path.to_path_buf()]);
    save_pending_ai(&repo.shared_root, &pending)?;

    println!("[arc] Review the changes, run your tests, then 'arc ai approve' to sign and commit.");
    Ok(())
}

/// Query the vector index for the 3 most relevant prior change intents.
///
/// Returns an empty string if the index is not yet initialised or if the
/// embedding provider is unavailable.  This is a best-effort operation —
/// `arc generate` still proceeds without context on failure.
fn retrieve_prior_context(goal: &str, repo: &mut Repository) -> String {
    use arc_ai::{
        embedding::{EmbeddingProvider, HybridProvider},
        vector_store::VectorStore,
    };

    let db_path = repo.shared_root.join(".arc").join("ai").join("embeddings.db");
    if !db_path.exists() {
        return String::new();
    }

    let provider = match HybridProvider::new() {
        Ok(p) => p,
        Err(_) => return String::new(),
    };
    let query_vec = match provider.embed(goal) {
        Ok(v) => v,
        Err(_) => return String::new(),
    };
    let store = match VectorStore::open(&db_path) {
        Ok(s) => s,
        Err(_) => return String::new(),
    };
    let results = match store.search(&query_vec, 3) {
        Ok(r) => r,
        Err(_) => return String::new(),
    };
    if results.is_empty() {
        return String::new();
    }

    let mut ctx = String::from("Relevant prior changes in this repository:\n");
    for (id_hex, score) in &results {
        if let Some(hash) = parse_hex_hash(id_hex)
            && let Some(change) = repo.graph.load().get(&hash)
        {
            ctx.push_str(&format!("- (similarity {score:.2}) {}\n", change.intent));
        }
    }
    ctx
}

fn parse_hex_hash(hex: &str) -> Option<arc_algebra_types::Blake3Hash> {
    if hex.len() != 64 {
        return None;
    }
    let mut bytes = [0u8; 32];
    for (i, byte) in bytes.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(bytes)
}

fn build_prompt(goal: &str, file_context: &str, prior_context: &str) -> String {
    let mut prompt = String::new();
    if !prior_context.is_empty() {
        prompt.push_str(prior_context);
        prompt.push('\n');
    }
    if !file_context.is_empty() {
        prompt.push_str(file_context);
        prompt.push('\n');
    }
    prompt.push_str(&format!("\nGoal: {goal}\n"));
    prompt
}
