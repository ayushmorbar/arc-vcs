//! Causality-aware operation log for local compaction and safe undo boundaries.

use crate::git_bridge::GitOid;

/// Causality boundary for operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Causality {
    /// Operation has not been broadcast and can be compacted locally.
    Local,
    /// Operation has been broadcast and is immutable.
    NetworkStable,
}

/// User intent represented in the operation log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpAction {
    /// Snapshot working-copy changes.
    Snap,
    /// Revert prior state.
    Revert,
    /// Merge two or more heads.
    Merge,
    /// Squash a contiguous range.
    Squash,
}

/// Single operation record in the OpLog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpRecord {
    /// Unique operation id.
    pub id: String,
    /// User action for this record.
    pub action: OpAction,
    /// Causality boundary class.
    pub causality: Causality,
    /// Unix epoch seconds.
    pub timestamp: i64,
    /// Resulting graph state after applying the operation.
    pub target_oid: GitOid,
}

/// Backward-compatible alias for existing docs and older call-sites.
pub type Operation = OpRecord;

/// In-memory operation log.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct OpLog {
    records: Vec<OpRecord>,
}

/// OpLog compaction error type.
pub type Error = std::convert::Infallible;

impl OpLog {
    /// Create an empty operation log.
    pub fn new() -> Self {
        Self {
            records: Vec::new(),
        }
    }

    /// Create an operation log from pre-existing records.
    pub fn from_records(records: Vec<OpRecord>) -> Self {
        Self { records }
    }

    /// Borrow all records in append order.
    pub fn records(&self) -> &[OpRecord] {
        &self.records
    }

    /// Append a new operation record.
    pub fn push(&mut self, record: OpRecord) {
        self.records.push(record);
    }

    /// Compact trailing local-only operation history.
    ///
    /// Walks backward from the tip and removes contiguous records with
    /// [`Causality::Local`]. Compaction stops at the first
    /// [`Causality::NetworkStable`] boundary.
    pub fn compact_local_history(&mut self) -> Result<usize, Error> {
        let mut compacted = 0usize;
        while let Some(last) = self.records.last() {
            if last.causality != Causality::Local {
                break;
            }
            self.records.pop();
            compacted += 1;
        }
        Ok(compacted)
    }

    /// Pop one trailing local record and return the new tip state for undo.
    pub fn pop_local_for_undo(&mut self) -> Option<GitOid> {
        let is_local_tip = self
            .records
            .last()
            .map(|r| r.causality == Causality::Local)
            .unwrap_or(false);
        if !is_local_tip {
            return None;
        }

        self.records.pop();
        self.records.last().map(|r| r.target_oid)
    }
}

#[cfg(test)]
mod tests {
    use super::{Causality, OpAction, OpLog, OpRecord};

    fn oid(seed: u8) -> [u8; 20] {
        [seed; 20]
    }

    fn record(id: &str, causality: Causality, target: u8) -> OpRecord {
        OpRecord {
            id: id.to_string(),
            action: OpAction::Snap,
            causality,
            timestamp: 1_700_000_000,
            target_oid: oid(target),
        }
    }

    #[test]
    fn network_wall_blocks_compaction_boundary() {
        let records = vec![
            record("n1", Causality::NetworkStable, 1),
            record("n2", Causality::NetworkStable, 2),
            record("l1", Causality::Local, 3),
            record("l2", Causality::Local, 4),
            record("l3", Causality::Local, 5),
        ];
        let mut log = OpLog::from_records(records);

        let compacted = log
            .compact_local_history()
            .expect("in-memory compaction is infallible");
        assert_eq!(compacted, 3);
        assert_eq!(log.records().len(), 2);
        assert!(
            log.records()
                .iter()
                .all(|r| r.causality == Causality::NetworkStable)
        );
    }

    #[test]
    fn popping_local_yields_previous_target_oid_for_undo() {
        let records = vec![
            record("stable", Causality::NetworkStable, 7),
            record("local", Causality::Local, 9),
        ];
        let mut log = OpLog::from_records(records);

        let previous = log
            .pop_local_for_undo()
            .expect("local tip should be undoable");
        assert_eq!(previous, oid(7));
        assert_eq!(log.records().len(), 1);
        assert_eq!(log.records()[0].id, "stable");
    }
}
