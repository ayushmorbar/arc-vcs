//! Append-only operation log — the spacetime ledger of every view-mutating
//! command in the repository.
//!
//! Every time a command advances a [`View`](super::view::View)'s DAG heads
//! (e.g. `arc snap`, `arc merge`, `arc cherry-pick`, `arc revert`, `arc restore`),
//! an [`Operation`] is appended to `.arc/oplog.json`.  Because the CAS is purely
//! immutable, the underlying graph objects are **never deleted** — only the View
//! pointer moves.  This makes every mutating operation **O(1)-reversible**:
//! [`OpLog::pop`] returns the `before_heads` needed to restore the View pointer
//! with zero data loss.
//!
//! ## Local-only semantics
//!
//! The oplog is **strictly local**.  It is intentionally excluded from all CRDT
//! hashing, network sync, and CAS indexing operations.  Syncing it would cause
//! catastrophic metadata bloat across the network — each developer's pointer
//! history is irrelevant to peers.  Never include `.arc/oplog.json` in any
//! serialized view or change object.
//!
//! ## Compaction
//!
//! To prevent unbounded I/O overhead on highly active repositories,
//! [`OpLog::append`] silently drops the oldest entries when the log exceeds
//! [`MAX_ENTRIES`] (1 000 operations).  This sliding-window eviction ensures the
//! JSON parse cost on every CLI invocation remains bounded and predictable.
//!
//! ## File format
//!
//! `.arc/oplog.json` is a pretty-printed JSON array of [`Operation`] objects.
//! The `before_heads` field carries a `#[serde(alias = "previous_heads")]`
//! annotation so that older oplog files written by pre-Phase-36 builds remain
//! fully readable without a migration step.
//!
//! ## O(1) undo guarantee
//!
//! ```text
//! View pointer:  HEAD  →  (slide backward)
//!
//!   [op₃] snap "fix bug"     after:  {hash₃}
//!   [op₂] merge feature      after:  {hash₁, hash₂}
//!   [op₁] snap "initial"     after:  {hash₁}
//!
//!   arc op undo  →  restores before_heads of op₃  →  {hash₁, hash₂}
//!   arc op undo  →  restores before_heads of op₂  →  {hash₁}
//!   arc op undo  →  restores before_heads of op₁  →  {} (empty view)
//! ```
//!
//! All graph objects (`hash₁`, `hash₂`, `hash₃`) remain in `.arc/store/` and
//! `.arc/blobs/` indefinitely.  "Undo" moves a pointer; it never deletes data.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::algebra::Blake3Hash;

// ── Constants ─────────────────────────────────────────────────────────────────

/// Maximum number of entries retained in the oplog.
///
/// When [`OpLog::append`] would push the entry count above this limit, the
/// oldest entries are silently evicted to keep the JSON parse cost bounded.
pub const MAX_ENTRIES: usize = 1_000;

// ── OperationAgent ────────────────────────────────────────────────────────────

/// The actor that triggered a repository operation.
///
/// Stored inside every [`Operation`] so that `arc op log` can render a
/// `👤 Human` / `🤖 AI` column, allowing developers to immediately identify
/// and audit autonomously-executed mutations.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationAgent {
    /// The operation was triggered directly by a human developer.
    #[default]
    Human,
    /// The operation was triggered by an AI agent (e.g. `arc snap --auto-msg`,
    /// AI conflict resolution, an autonomous background sync).
    Ai,
}

impl OperationAgent {
    ///
    /// ```text
    /// Human  →  "👤 Human"
    /// Ai     →  "🤖 AI"
    /// ```
    pub fn label(&self) -> &'static str {
        match self {
            OperationAgent::Human => "👤 Human",
            OperationAgent::Ai => "🤖 AI",
        }
    }
}

// ── Operation ─────────────────────────────────────────────────────────────────

/// A single immutable entry in the spacetime operation log.
///
/// Records every view-mutating command together with the DAG state
/// *before* (`before_heads`) and *after* (`after_heads`) the command, so that
/// [`OpLog::pop`] can restore the prior state in O(1) time.
///
/// The `before_heads` field accepts `"previous_heads"` as a deserialization
/// alias for backward-compatibility with pre-Phase-36 oplog files.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Operation {
    /// Short 8-char hex identifier.
    ///
    /// Computed as the first 8 hex characters of
    /// BLAKE3(`timestamp_le` ‖ `command`).  Deterministic and unique enough
    /// for display and cross-reference; not cryptographically binding.
    pub id: String,
    /// Unix timestamp (seconds since epoch) when the operation was recorded.
    pub timestamp: u64,
    /// CLI command name that triggered the operation.
    ///
    /// Examples: `"snap"`, `"merge"`, `"cherry-pick"`, `"revert"`, `"restore"`.
    pub command: String,
    /// Name of the view that was mutated.
    pub view: String,
    /// The actor that executed this operation.
    ///
    /// Defaults to [`OperationAgent::Human`] when deserializing older oplog
    /// files that predate this field.
    #[serde(default)]
    pub agent: OperationAgent,
    /// Heads of the view **before** the operation.
    ///
    /// Used by [`OpLog::pop`] to restore the pre-mutation state.
    /// Accepts `"previous_heads"` as a deserialization alias so that oplog
    /// files written by pre-Phase-36 builds remain readable without migration.
    #[serde(alias = "previous_heads")]
    pub before_heads: HashSet<Blake3Hash>,
    /// Heads of the view **after** the operation.
    ///
    /// Empty for operations that do not advance the view pointer (e.g.
    /// `restore`, which rewrites working-directory files but leaves heads
    /// unchanged).  Defaults to an empty set so older oplog files lacking
    /// this field remain readable.
    #[serde(default)]
    pub after_heads: HashSet<Blake3Hash>,
}

impl Operation {
    /// Construct a new [`Operation`] for a human-triggered command.
    ///
    /// The `id` is the first 8 hex characters of BLAKE3(`timestamp_le ‖ command`).
    ///
    /// # Arguments
    ///
    /// * `command` — short command name, e.g. `"snap"`.
    /// * `view` — name of the view that was mutated.
    /// * `before_heads` — DAG heads **before** the mutation.
    /// * `after_heads` — DAG heads **after** the mutation.
    pub fn new(
        command: impl Into<String>,
        view: impl Into<String>,
        before_heads: HashSet<Blake3Hash>,
        after_heads: HashSet<Blake3Hash>,
    ) -> Self {
        Self::new_with_agent(
            command,
            view,
            before_heads,
            after_heads,
            OperationAgent::Human,
        )
    }

    /// Construct a new [`Operation`] specifying the triggering agent explicitly.
    ///
    /// Use this variant when recording operations executed by an AI agent so
    /// that `arc op log` can render the `🤖 AI` label.
    pub fn new_with_agent(
        command: impl Into<String>,
        view: impl Into<String>,
        before_heads: HashSet<Blake3Hash>,
        after_heads: HashSet<Blake3Hash>,
        agent: OperationAgent,
    ) -> Self {
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let command = command.into();
        let view = view.into();
        // Stable 8-char op ID: BLAKE3(timestamp_le || command bytes).
        let mut hasher = blake3::Hasher::new();
        hasher.update(&timestamp.to_le_bytes());
        hasher.update(command.as_bytes());
        let id = hasher.finalize().to_hex()[..8].to_string();
        Self {
            id,
            timestamp,
            command,
            view,
            agent,
            before_heads,
            after_heads,
        }
    }

    /// Return the timestamp as a UTC `YYYY-MM-DD HH:MM:SS` string.
    ///
    /// Implemented without the `chrono` crate to avoid an extra dependency.
    /// Accurate for all dates representable as a Unix timestamp (well past 2100).
    pub fn formatted_time(&self) -> String {
        let mut secs = self.timestamp;
        let second = secs % 60;
        secs /= 60;
        let minute = secs % 60;
        secs /= 60;
        let hour = secs % 24;
        secs /= 24; // days since 1970-01-01
        let (year, month, day) = _days_to_ymd(secs);
        format!("{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}:{second:02}")
    }

    /// Return the first 8 hex chars of the first `before_heads` entry, or
    /// `"(empty)"` when the view had no heads before this operation.
    pub fn before_short(&self) -> String {
        _heads_short(&self.before_heads)
    }

    /// Return the first 8 hex chars of the first `after_heads` entry, or
    /// `"(empty)"` when the operation did not advance the view.
    pub fn after_short(&self) -> String {
        _heads_short(&self.after_heads)
    }
}

// ── OpLog ─────────────────────────────────────────────────────────────────────

/// Append-only operation log backed by `.arc/oplog.json`.
///
/// ### Usage pattern
///
/// ```text
/// let log = OpLog::new(&shared_root.join(".arc"));
///
/// // Record an operation after a mutation:
/// let op = Operation::new("snap", "main", before_heads, after_heads);
/// log.append(&op)?;
///
/// // Inspect history (newest first):
/// for op in log.read_reversed()? { … }
///
/// // Undo: pop and use before_heads:
/// if let Some(op) = log.pop()? {
///     view.heads = op.before_heads;
///     view.save(…)?;
/// }
/// ```
pub struct OpLog {
    path: PathBuf,
}

impl OpLog {
    /// Create an [`OpLog`] handle for the file at `<arc_dir>/oplog.json`.
    ///
    /// `arc_dir` must be the `.arc/` directory (i.e. `repo_root.join(".arc")`).
    /// The file is created lazily on the first call to [`append`](Self::append).
    pub fn new(arc_dir: &Path) -> Self {
        Self {
            path: arc_dir.join("oplog.json"),
        }
    }

    /// Append `op` to the end of the log, then compact if necessary.
    ///
    /// If the resulting array would exceed [`MAX_ENTRIES`] the oldest entries
    /// are silently evicted so the log file never grows unboundedly.
    ///
    /// Creates the file if it does not yet exist.
    pub fn append(&self, op: &Operation) -> Result<()> {
        let mut entries = self.read_all().unwrap_or_default();
        entries.push(op.clone());
        // Sliding-window compaction: keep the most recent MAX_ENTRIES.
        if entries.len() > MAX_ENTRIES {
            let drop = entries.len() - MAX_ENTRIES;
            entries.drain(..drop);
        }
        std::fs::write(&self.path, serde_json::to_string_pretty(&entries)?)
            .map_err(|e| anyhow::anyhow!("failed to write oplog: {e}"))
    }

    /// Return all operations in **chronological order** (oldest first).
    ///
    /// Returns an empty `Vec` when the file does not exist.
    pub fn read_all(&self) -> Result<Vec<Operation>> {
        if !self.path.exists() {
            return Ok(Vec::new());
        }
        let json = std::fs::read_to_string(&self.path)
            .map_err(|e| anyhow::anyhow!("failed to read oplog: {e}"))?;
        Ok(serde_json::from_str(&json).unwrap_or_default())
    }

    /// Return all operations in **reverse-chronological order** (newest first).
    ///
    /// Convenience wrapper over [`read_all`](Self::read_all) for display use.
    pub fn read_reversed(&self) -> Result<Vec<Operation>> {
        let mut all = self.read_all()?;
        all.reverse();
        Ok(all)
    }

    /// Remove and return the **most-recent** operation, or `None` if the log
    /// is empty.  Rewrites the file without the popped entry.
    pub fn pop(&self) -> Result<Option<Operation>> {
        let mut entries = self.read_all()?;
        let last = entries.pop();
        if last.is_some() {
            std::fs::write(&self.path, serde_json::to_string_pretty(&entries)?)
                .map_err(|e| anyhow::anyhow!("failed to write oplog: {e}"))?;
        }
        Ok(last)
    }
}

// ── Private helpers ───────────────────────────────────────────────────────────

/// Convert a count of days since 1970-01-01 to `(year, month, day)`.
///
/// Uses the 400-year Gregorian cycle (146,097 days).  Accurate for all dates
/// representable as a Unix timestamp (well past 2100).
fn _days_to_ymd(mut days: u64) -> (u64, u64, u64) {
    let year400 = days / 146_097;
    days %= 146_097;
    let year100 = (days / 36_524).min(3);
    days -= year100 * 36_524;
    let year4 = days / 1_461;
    days %= 1_461;
    let year1 = (days / 365).min(3);
    days -= year1 * 365;
    let year = year400 * 400 + year100 * 100 + year4 * 4 + year1 + 1970;
    let leap = (year.is_multiple_of(4) && !year.is_multiple_of(100)) || year.is_multiple_of(400);
    let days_in_month: [u64; 12] = [
        31,
        if leap { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    let mut month = 1u64;
    for &dim in &days_in_month {
        if days < dim {
            break;
        }
        days -= dim;
        month += 1;
    }
    (year, month, days + 1)
}

/// Format the first entry in a head-set as an 8-char hex string, or `"(empty)"`.
fn _heads_short(heads: &HashSet<Blake3Hash>) -> String {
    heads
        .iter()
        .next()
        .map(|h| h.iter().map(|b| format!("{b:02x}")).collect::<String>()[..8].to_string())
        .unwrap_or_else(|| "(empty)".to_string())
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_operation_new_has_short_id() {
        let op = Operation::new("snap", "main", HashSet::new(), HashSet::new());
        assert_eq!(op.id.len(), 8, "id must be 8 hex chars");
        assert!(
            op.id.chars().all(|c| c.is_ascii_hexdigit()),
            "id must be hex"
        );
        assert_eq!(op.command, "snap");
        assert_eq!(op.view, "main");
        assert_eq!(op.agent, OperationAgent::Human);
    }

    #[test]
    fn test_operation_ai_agent_label() {
        let op = Operation::new_with_agent(
            "snap",
            "main",
            HashSet::new(),
            HashSet::new(),
            OperationAgent::Ai,
        );
        assert_eq!(op.agent, OperationAgent::Ai);
        assert_eq!(op.agent.label(), "🤖 AI");
        assert_eq!(OperationAgent::Human.label(), "👤 Human");
    }

    #[test]
    fn test_operation_formatted_time_is_plausible() {
        let op = Operation::new("snap", "main", HashSet::new(), HashSet::new());
        let t = op.formatted_time();
        assert_eq!(t.len(), 19, "formatted time must be 19 chars: got '{t}'");
        assert!(t.starts_with("20"), "year must start with 20xx: got '{t}'");
    }

    #[test]
    fn test_days_to_ymd_epoch() {
        let (y, m, d) = _days_to_ymd(0);
        assert_eq!((y, m, d), (1970, 1, 1));
    }

    #[test]
    fn test_days_to_ymd_known_date() {
        // 2025-03-08 ≈ 20155 days after 1970-01-01
        let (y, _m, _d) = _days_to_ymd(20155);
        assert!((2024..=2026).contains(&y), "year should be near 2025, got {y}");
    }

    #[test]
    fn test_oplog_append_read_pop() {
        let dir = tempfile::tempdir().unwrap();
        let log = OpLog::new(dir.path());

        assert!(
            log.read_all().unwrap().is_empty(),
            "fresh log must be empty"
        );

        let op1 = Operation::new("snap", "main", HashSet::new(), HashSet::new());
        let op2 = Operation::new("merge", "main", HashSet::new(), HashSet::new());
        log.append(&op1).unwrap();
        log.append(&op2).unwrap();

        let all = log.read_all().unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].command, "snap");
        assert_eq!(all[1].command, "merge");

        let rev = log.read_reversed().unwrap();
        assert_eq!(rev[0].command, "merge");
        assert_eq!(rev[1].command, "snap");

        let popped = log.pop().unwrap().expect("pop must return the last entry");
        assert_eq!(popped.command, "merge");

        let remaining = log.read_all().unwrap();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].command, "snap");
    }

    #[test]
    fn test_oplog_pop_empty() {
        let dir = tempfile::tempdir().unwrap();
        let log = OpLog::new(dir.path());
        let result = log.pop().unwrap();
        assert!(result.is_none(), "pop on empty log must return None");
    }

    #[test]
    fn test_oplog_sliding_window_compaction() {
        let dir = tempfile::tempdir().unwrap();
        let log = OpLog::new(dir.path());
        // Write MAX_ENTRIES + 5 entries; expect only MAX_ENTRIES to survive.
        for i in 0..=(MAX_ENTRIES + 4) {
            let op = Operation::new(format!("snap-{i}"), "main", HashSet::new(), HashSet::new());
            log.append(&op).unwrap();
        }
        let all = log.read_all().unwrap();
        assert_eq!(
            all.len(),
            MAX_ENTRIES,
            "oplog must be capped at MAX_ENTRIES"
        );
        // The oldest entries were evicted; the most recent entries are retained.
        assert_eq!(
            all[MAX_ENTRIES - 1].command,
            format!("snap-{}", MAX_ENTRIES + 4)
        );
    }

    #[test]
    fn test_operation_backward_compat_previous_heads_alias() {
        // Simulate a pre-Phase-36 oplog entry that uses "previous_heads".
        let json = r#"[{
            "id": "abcd1234",
            "timestamp": 1700000000,
            "command": "snap",
            "view": "main",
            "previous_heads": []
        }]"#;
        let ops: Vec<Operation> = serde_json::from_str(json).expect("must deserialize");
        assert_eq!(ops.len(), 1);
        assert_eq!(ops[0].command, "snap");
        assert!(ops[0].before_heads.is_empty());
        // agent should default to Human
        assert_eq!(ops[0].agent, OperationAgent::Human);
    }
}
