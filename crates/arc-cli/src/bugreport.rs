//! `arc bugreport` — anonymized DAG telemetry packager.
//!
//! Generates a JSON file containing:
//! - OS / architecture / arc version metadata.
//! - A structural dump of every node in the change graph: hash, parent edges,
//!   author *type*, and a BLAKE3-hashed author display string.
//! - The raw `intent` field is **omitted by default** to protect proprietary
//!   information. Pass `include_raw_intent = true` (the `--include-raw-intent`
//!   CLI flag) to include it.
//! - A copy of the merged `[ui]` and `[merge]` config sections, which often
//!   trigger edge-case panics and are safe to share.

use std::io::Write;

use serde::Serialize;

use crate::repo::{Repository, load_merged_config};
use arc_change::Change;
use arc_store_types::author::Author;

// ── wire-format types ────────────────────────────────────────────────────────

#[derive(Serialize)]
struct BugReport<'r> {
    arc_version: &'static str,
    os: &'static str,
    arch: &'static str,
    config: ConfigSnapshot,
    graph: Vec<NodeEntry<'r>>,
}

#[derive(Serialize)]
struct ConfigSnapshot {
    ui_color: String,
    merge_tool: Option<String>,
}

#[derive(Serialize)]
struct NodeEntry<'r> {
    id: String,
    deps: Vec<String>,
    author_type: &'static str,
    /// BLAKE3 hex of the author's display string — irreversibly anonymized.
    author_name_hash: String,
    /// `None` unless `--include-raw-intent` was supplied.
    intent: Option<&'r str>,
    /// BLAKE3 hex of the `collapsed_from` hash, if set.
    collapsed_from: Option<String>,
}

// ── public API ────────────────────────────────────────────────────────────────

/// Generate and write the bug report to `output_path`.
pub fn generate(
    repo: &Repository,
    output_path: &str,
    include_raw_intent: bool,
) -> anyhow::Result<()> {
    let config = load_merged_config(&repo.shared_root).unwrap_or_default();

    let g = repo.graph.load_full();
    let graph: Vec<NodeEntry<'_>> = g
        .iter()
        .map(|change| node_entry(change, include_raw_intent))
        .collect();

    let report = BugReport {
        arc_version: env!("CARGO_PKG_VERSION"),
        os: std::env::consts::OS,
        arch: std::env::consts::ARCH,
        config: ConfigSnapshot {
            ui_color: config.ui.color.clone(),
            merge_tool: config.merge.tool.clone(),
        },
        graph,
    };

    let json = serde_json::to_string_pretty(&report)
        .map_err(|e| anyhow::anyhow!("failed to serialize bug report: {e}"))?;

    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(output_path)
        .map_err(|e| anyhow::anyhow!("cannot open '{output_path}': {e}"))?;
    file.write_all(json.as_bytes())
        .map_err(|e| anyhow::anyhow!("failed to write bug report: {e}"))?;

    Ok(())
}

// ── helpers ───────────────────────────────────────────────────────────────────

fn node_entry<'c>(change: &'c Change, include_raw_intent: bool) -> NodeEntry<'c> {
    let id = hex(&change.id);
    let deps: Vec<String> = {
        let mut v: Vec<String> = change.deps.iter().map(hex).collect();
        v.sort(); // deterministic ordering
        v
    };
    let (author_type, author_display) = author_meta(&change.author);
    let author_name_hash = hex(blake3::hash(author_display.as_bytes()).as_bytes());
    let intent = if include_raw_intent {
        Some(change.intent.as_str())
    } else {
        None
    };
    let collapsed_from = change.collapsed_from.as_ref().map(hex);

    NodeEntry {
        id,
        deps,
        author_type,
        author_name_hash,
        intent,
        collapsed_from,
    }
}

fn author_meta(author: &Author) -> (&'static str, String) {
    match author {
        Author::Human { name, email, .. } => ("Human", format!("{name} <{email}>")),
        Author::AI { model, .. } => ("AI", model.clone()),
        Author::Server { canonical_id, .. } => ("Server", canonical_id.clone()),
        Author::Transient { session_id, .. } => ("Transient", session_id.clone()),
    }
}

#[inline]
fn hex(b: &[u8; 32]) -> String {
    b.iter().fold(String::with_capacity(64), |mut s, byte| {
        use std::fmt::Write;
        let _ = write!(s, "{byte:02x}");
        s
    })
}
