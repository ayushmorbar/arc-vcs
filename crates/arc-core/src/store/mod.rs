//! arc object store: CAS, changes, graph, views, author identity, and oplog.

/// Author identity and signing-key management.
pub mod author;
/// Deterministic DAG bisect state machine and persistence.
pub mod bisect;
/// Zero-overhead identity hasher for [`crate::algebra::Blake3Hash`] keys.
pub mod blake3_hasher;
/// Content-addressable object store (BLAKE3 CAS).
pub mod cas;
/// Immutable semantic changes and their dependency metadata.
pub mod change;
/// Deterministic content-hash trait and derive macro.
pub mod content_hash;
/// Change dependency graph and ancestry algorithms.
pub mod graph;
/// Strongly-typed content IDs (NewType wrappers).
pub mod newtypes;
/// Reference metadata readers for tags and remote-tracking branches.
pub mod refs;
/// Crash-consistent synthesized architecture snapshots.
pub mod synthesis;
/// Signal-safe temporary-file registry.
pub mod tempfile;

pub use blake3_hasher::{Blake3HashMap, Blake3Hasher};
pub use cas::CasBytes;
/// Append-only spacetime operation log for O(1) undo.
pub mod oplog;
/// Cryptographically-signed immutable tags.
pub mod tag;
/// Virtual views (branches) over the change DAG.
pub mod view;

/// Errors produced by the arc object store.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    /// An I/O error from the filesystem.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// A bincode serialization or deserialization error.
    #[error("serialization error: {0}")]
    Serialization(#[from] Box<bincode::ErrorKind>),
}
