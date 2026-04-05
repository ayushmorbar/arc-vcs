#![warn(missing_docs)]

//! Pure state model for resumable rewrite transactions.
//!
//! This crate contains serializable checkpoint state and deterministic state
//! transitions. It intentionally does not perform any I/O.

use std::collections::{BTreeMap, BTreeSet};

use arc_store_types::newtypes::ChangeId;
use serde::{Deserialize, Serialize};

/// Checkpoint schema version for persisted rewrite sessions.
pub const CHECKPOINT_VERSION: u32 = 1;

/// Runtime status of a pending rewrite transaction.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RewriteStatus {
    /// Transaction is ready to execute or resume.
    InProgress,
    /// Execution paused due to a recoverable failure.
    Conflict {
        /// Human-readable pause reason shown by the CLI.
        message: String,
    },
}

/// Resumable checkpoint for `arc restack`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingRewrite {
    /// Checkpoint schema version.
    pub version: u32,
    /// Human-readable transaction kind.
    pub command: String,
    /// View being rewritten.
    pub view: String,
    /// View heads before the transaction began.
    pub before_heads: BTreeSet<ChangeId>,
    /// Desired oldest->newest order for the restack.
    pub desired_order: Vec<ChangeId>,
    /// Old->new rewrite bindings produced by prior attempts.
    pub rewrite_map: BTreeMap<ChangeId, ChangeId>,
    /// Number of execution attempts so far.
    pub attempts: u32,
    /// Current runtime status.
    pub status: RewriteStatus,
}

impl PendingRewrite {
    /// Build a fresh `arc restack` checkpoint.
    pub fn new_restack(
        view: impl Into<String>,
        before_heads: BTreeSet<ChangeId>,
        desired_order: Vec<ChangeId>,
    ) -> Self {
        Self {
            version: CHECKPOINT_VERSION,
            command: "restack".to_string(),
            view: view.into(),
            before_heads,
            desired_order,
            rewrite_map: BTreeMap::new(),
            attempts: 0,
            status: RewriteStatus::InProgress,
        }
    }

    /// Return the desired order with any known rewrite bindings projected.
    pub fn resolved_order(&self) -> Vec<ChangeId> {
        self.desired_order
            .iter()
            .copied()
            .map(|id| remap_change(id, &self.rewrite_map))
            .collect()
    }

    /// Return a copy marked with one additional execution attempt.
    pub fn with_attempt_incremented(mut self) -> Self {
        self.attempts = self.attempts.saturating_add(1);
        self
    }

    /// Return a copy updated to conflict status.
    pub fn with_conflict(mut self, message: impl Into<String>) -> Self {
        self.status = RewriteStatus::Conflict {
            message: message.into(),
        };
        self
    }

    /// Return a copy reset to in-progress status.
    pub fn clear_conflict(mut self) -> Self {
        self.status = RewriteStatus::InProgress;
        self
    }

    /// Return a copy with rewrite-map entries merged in.
    pub fn with_rewrite_map(mut self, rewrites: &BTreeMap<ChangeId, ChangeId>) -> Self {
        for (old, new) in rewrites {
            self.rewrite_map.insert(*old, *new);
        }
        self
    }
}

fn remap_change(mut id: ChangeId, map: &BTreeMap<ChangeId, ChangeId>) -> ChangeId {
    let mut visited = BTreeSet::new();
    let max_hops = map.len().saturating_add(1);
    for _ in 0..max_hops {
        let Some(next) = map.get(&id) else {
            break;
        };
        if *next == id || !visited.insert(id) {
            break;
        }
        id = *next;
    }
    id
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cid(byte: u8) -> ChangeId {
        ChangeId::from([byte; 32])
    }

    #[test]
    fn new_restack_initializes_in_progress() {
        let pending =
            PendingRewrite::new_restack("main", BTreeSet::from([cid(1)]), vec![cid(2), cid(1)]);

        assert_eq!(pending.version, CHECKPOINT_VERSION);
        assert_eq!(pending.command, "restack");
        assert_eq!(pending.status, RewriteStatus::InProgress);
        assert_eq!(pending.attempts, 0);
    }

    #[test]
    fn resolved_order_applies_chained_rewrites() {
        let pending = PendingRewrite {
            version: CHECKPOINT_VERSION,
            command: "restack".to_string(),
            view: "main".to_string(),
            before_heads: BTreeSet::from([cid(1)]),
            desired_order: vec![cid(7), cid(8)],
            rewrite_map: BTreeMap::from([(cid(7), cid(9)), (cid(9), cid(10))]),
            attempts: 0,
            status: RewriteStatus::InProgress,
        };

        assert_eq!(pending.resolved_order(), vec![cid(10), cid(8)]);
    }

    #[test]
    fn with_conflict_sets_status_and_attempts_increment() {
        let pending =
            PendingRewrite::new_restack("main", BTreeSet::from([cid(1)]), vec![cid(2), cid(1)])
                .with_attempt_incremented()
                .with_conflict("merge conflict");

        assert_eq!(pending.attempts, 1);
        assert_eq!(
            pending.status,
            RewriteStatus::Conflict {
                message: "merge conflict".to_string()
            }
        );
    }
}
