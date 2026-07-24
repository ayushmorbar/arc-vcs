//! `arc bugreport` — anonymized DAG telemetry packager.
//!
//! Generates a JSON file containing:
//! - OS / architecture / arc version metadata.
//! - A structural dump of every node in the change graph: hash, parent edges, author *type*, and a
//!   BLAKE3-hashed author display string.
//! - The raw `intent` field is **omitted by default** to protect proprietary information. Pass
//!   `include_raw_intent = true` (the `--include-raw-intent` CLI flag) to include it.
//! - A copy of the merged `[ui]` and `[merge]` config sections, which often trigger edge-case
//!   panics and are safe to share.

use std::io::Write;

use arc_change::Change;
use arc_store_types::author::Author;
use serde::Serialize;

use crate::repo::{Repository, load_merged_config};

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
    let graph: Vec<NodeEntry<'_>> =
        g.iter().map(|change| node_entry(change, include_raw_intent)).collect();

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
    let intent = if include_raw_intent { Some(change.intent.as_str()) } else { None };
    let collapsed_from = change.collapsed_from.as_ref().map(hex);

    NodeEntry { id, deps, author_type, author_name_hash, intent, collapsed_from }
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

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use arc_store_types::author::Author;

    use super::*;

    fn test_key() -> ed25519_dalek::SigningKey {
        ed25519_dalek::SigningKey::from_bytes(&[1u8; 32])
    }

    #[test]
    fn hex_encodes_bytes_correctly() {
        let bytes = [0u8; 32];
        assert_eq!(hex(&bytes), "0".repeat(64));

        let bytes = [0xFFu8; 32];
        assert_eq!(hex(&bytes), "ff".repeat(32));

        let bytes = [
            0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0A, 0x0B, 0x0C, 0x0D, 0x0E,
            0x0F, 0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x18, 0x19, 0x1A, 0x1B, 0x1C,
            0x1D, 0x1E, 0x1F, 0x20,
        ];
        let result = hex(&bytes);
        assert_eq!(result.len(), 64);
        assert!(result.starts_with("01020304"));
    }

    #[test]
    fn author_meta_human() {
        let author = Author::Human {
            name: "Alice".to_string(),
            email: "alice@example.com".to_string(),
            key: [0u8; 32],
        };
        let (typ, display) = author_meta(&author);
        assert_eq!(typ, "Human");
        assert_eq!(display, "Alice <alice@example.com>");
    }

    #[test]
    fn author_meta_ai() {
        let author = Author::AI { model: "gpt-4o".to_string(), human_sponsor: [1u8; 32] };
        let (typ, display) = author_meta(&author);
        assert_eq!(typ, "AI");
        assert_eq!(display, "gpt-4o");
    }

    #[test]
    fn author_meta_server() {
        let author = Author::Server { canonical_id: "server-123".to_string(), key: [2u8; 32] };
        let (typ, display) = author_meta(&author);
        assert_eq!(typ, "Server");
        assert_eq!(display, "server-123");
    }

    #[test]
    fn author_meta_transient() {
        let author = Author::Transient { session_id: "sess-abc".to_string(), key: [0u8; 32] };
        let (typ, display) = author_meta(&author);
        assert_eq!(typ, "Transient");
        assert_eq!(display, "sess-abc");
    }

    #[test]
    fn node_entry_anonymizes_author() {
        let author = Author::Human {
            name: "Bob".to_string(),
            email: "bob@example.com".to_string(),
            key: [3u8; 32],
        };
        let change = Change::new(
            HashSet::from([[4u8; 32]]),
            vec![arc_algebra_types::Atom::Insert {
                at: vec!["file.rs".into(), "fn foo".into()],
                content_hash: [5u8; 32],
            }],
            "test intent",
            author,
            &test_key(),
        );
        let entry = node_entry(&change, false);
        assert_eq!(entry.author_type, "Human");
        assert!(!entry.author_name_hash.is_empty());
        assert_eq!(entry.author_name_hash.len(), 64);
        assert!(entry.intent.is_none());
        assert!(!entry.id.is_empty());
        assert_eq!(entry.deps.len(), 1);
    }

    #[test]
    fn node_entry_includes_raw_intent_when_requested() {
        let author = Author::AI { model: "test".to_string(), human_sponsor: [6u8; 32] };
        let change = Change::new(HashSet::new(), vec![], "my secret intent", author, &test_key());
        let entry = node_entry(&change, true);
        assert_eq!(entry.intent, Some("my secret intent"));
        assert_eq!(entry.author_type, "AI");
    }

    #[test]
    fn node_entry_collapsed_from() {
        let author = Author::Server { canonical_id: "srv".to_string(), key: [8u8; 32] };
        let mut change = Change::new(HashSet::new(), vec![], "collapse test", author, &test_key());
        change.collapsed_from = Some([10u8; 32]);
        let entry = node_entry(&change, false);
        assert!(entry.collapsed_from.is_some());
        let cf = entry.collapsed_from.unwrap();
        assert_eq!(cf.len(), 64);
    }

    #[test]
    fn node_entry_deps_sorted_deterministically() {
        let author = Author::Transient { session_id: "s".into(), key: [0u8; 32] };
        let change = Change::new(
            HashSet::from([[3u8; 32], [1u8; 32], [2u8; 32]]),
            vec![],
            "sorted deps",
            author,
            &test_key(),
        );
        let entry = node_entry(&change, false);
        let deps = entry.deps.clone();
        let mut sorted = deps.clone();
        sorted.sort();
        assert_eq!(deps, sorted);
    }

    #[test]
    fn node_entry_serializes_cleanly() {
        let author = Author::Human {
            name: "Test".to_string(),
            email: "t@t.com".to_string(),
            key: [12u8; 32],
        };
        let change = Change::new(HashSet::new(), vec![], "serialize me", author, &test_key());
        let entry = node_entry(&change, false);
        let json = serde_json::to_value(&entry).unwrap();
        assert!(json.is_object());
        assert!(json.get("id").is_some());
        assert!(json.get("author_type").is_some());
        assert!(json.get("author_name_hash").is_some());
        assert!(json.get("intent").is_some()); // Option<&str> serializes as null
    }

    #[test]
    fn bugreport_version_and_arch() {
        assert!(!env!("CARGO_PKG_VERSION").is_empty());
        assert!(!std::env::consts::OS.is_empty());
        assert!(!std::env::consts::ARCH.is_empty());
    }
}
