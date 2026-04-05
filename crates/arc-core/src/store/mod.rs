//! arc object store: CAS, changes, graph, views, author identity, and oplog.

/// Deterministic DAG bisect state machine and persistence.
pub mod bisect;
/// Zero-overhead identity hasher for [`crate::algebra::Blake3Hash`] keys.
pub mod blake3_hasher;
/// Content-addressable object store (BLAKE3 CAS).
pub mod cas;
/// Change dependency graph and ancestry algorithms.
pub mod graph;
/// Crash-consistent synthesized architecture snapshots.
pub mod synthesis;
/// Signal-safe temporary-file registry.
pub mod tempfile;

pub use arc_store_cas::cas::*;
pub use arc_change::change;
pub use arc_change::change::*;
pub use arc_change::content_hash;
pub use arc_change::content_hash::*;
pub use blake3_hasher::{Blake3HashMap, Blake3Hasher};
pub use cas::CasBytes;
/// Append-only spacetime operation log for O(1) undo.
pub mod oplog;
/// Virtual views (branches) over the change DAG.
pub mod view;

pub use arc_store_types::author;
pub use arc_store_types::author::*;
pub use arc_store_types::newtypes;
pub use arc_store_types::newtypes::*;
pub use arc_store_types::refs;
pub use arc_store_types::refs::*;
pub use arc_store_types::tag;
pub use arc_store_types::tag::*;

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
