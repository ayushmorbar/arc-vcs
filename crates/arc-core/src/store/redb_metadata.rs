//! Redb-backed metadata persistence for operation log records.
//!
//! This module is metadata-only: OpLog records and index pointers live in Redb,
//! while blob payload bytes remain in the raw BLAKE3 CAS store.

use std::path::Path;

use redb::{Database, ReadableDatabase, TableDefinition};

use crate::store::oplog::OpRecord;

const OPLOG_TABLE: TableDefinition<&str, &[u8]> = TableDefinition::new("oplog_records");

/// Metadata persistence failures for Redb-backed OpLog state.
#[derive(Debug, thiserror::Error)]
pub enum MetadataError {
    /// Database create/open failed.
    #[error("redb database error: {0}")]
    Database(#[from] redb::DatabaseError),
    /// Underlying Redb operation failed.
    #[error("redb error: {0}")]
    Redb(#[from] redb::Error),
    /// Transaction-related Redb operation failed.
    #[error("redb transaction error: {0}")]
    Transaction(#[from] redb::TransactionError),
    /// Table operation failed.
    #[error("redb table error: {0}")]
    Table(#[from] redb::TableError),
    /// Low-level storage operation failed.
    #[error("redb storage error: {0}")]
    Storage(#[from] redb::StorageError),
    /// Commit failed.
    #[error("redb commit error: {0}")]
    Commit(#[from] redb::CommitError),
    /// Storage path setup failed.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    /// Record serialization failed.
    #[error("serialization error: {0}")]
    Serialize(#[from] Box<bincode::ErrorKind>),
}

/// Redb metadata store containing OpLog records and pointer indexes.
pub struct MetadataStore {
    db: Database,
}

impl MetadataStore {
    /// Open or create metadata database at `<repo>/.arc/metadata.redb`.
    pub fn open(root: &Path) -> Result<Self, MetadataError> {
        let arc_dir = root.join(".arc");
        std::fs::create_dir_all(&arc_dir)?;
        let db_path = arc_dir.join("metadata.redb");
        let db = Database::create(&db_path)?;

        let tx = db.begin_write()?;
        {
            let _ = tx.open_table(OPLOG_TABLE)?;
        }
        tx.commit()?;

        Ok(Self { db })
    }

    /// Append or replace an OpLog metadata record by deterministic key.
    pub fn put_op_record(&self, record: &OpRecord) -> Result<(), MetadataError> {
        let key = record.id.clone();
        let payload = bincode::serialize(record)?;

        let tx = self.db.begin_write()?;
        {
            let mut table = tx.open_table(OPLOG_TABLE)?;
            table.insert(key.as_str(), payload.as_slice())?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Read a single OpLog metadata record by key.
    pub fn get_op_record(&self, key: &str) -> Result<Option<OpRecord>, MetadataError> {
        let tx = self.db.begin_read()?;
        let table = tx.open_table(OPLOG_TABLE)?;
        let Some(value) = table.get(key)? else {
            return Ok(None);
        };

        let bytes: &[u8] = value.value();
        let record = bincode::deserialize(bytes)?;
        Ok(Some(record))
    }
}

#[cfg(test)]
mod tests {
    use super::MetadataStore;
    use crate::store::oplog::{Causality, OpAction, OpRecord};

    fn sample_record() -> OpRecord {
        OpRecord {
            id: "op-1".to_string(),
            action: OpAction::Snap,
            causality: Causality::Local,
            timestamp: 1_700_000_000,
            target_oid: [7u8; 20],
            intent_summary: Some(
                "[auto-snap] Structural changes to core detected via tree-sitter.".to_string(),
            ),
        }
    }

    #[test]
    fn metadata_roundtrip() {
        let dir = tempfile::tempdir().expect("tempdir should be creatable");
        let store = MetadataStore::open(dir.path()).expect("metadata store should open");
        let record = sample_record();

        store.put_op_record(&record).expect("record should persist");

        let key = record.id.clone();
        let loaded =
            store.get_op_record(&key).expect("read should succeed").expect("record should exist");
        assert_eq!(loaded, record);
    }
}
