//! arc object store: CAS, changes, graph, views, author identity, and oplog.

/// Zero-overhead identity hasher for [`crate::algebra::Blake3Hash`] keys.
pub mod blake3_hasher;
/// Content-addressable object store (BLAKE3 CAS).
pub mod cas;

pub use arc_change::change;
pub use arc_change::change::*;
pub use arc_change::content_hash;
pub use arc_change::content_hash::*;
pub use arc_store_cas::cas::*;
pub use arc_store_graph::bisect;
pub use arc_store_graph::bisect::*;
pub use arc_store_graph::graph;
pub use arc_store_graph::graph::*;
pub use arc_store_view::StoreError;
pub use arc_store_view::oplog::*;
pub use arc_store_view::synthesis::*;
pub use arc_store_view::tempfile::*;
pub use arc_store_view::view::*;
pub use blake3_hasher::{Blake3HashMap, Blake3Hasher};
pub use cas::CasBytes;

/// Append-only spacetime operation log for O(1) undo.
pub mod oplog {
    pub use arc_store_view::oplog::*;
}

/// Crash-consistent synthesized architecture snapshots.
pub mod synthesis {
    pub use arc_store_view::synthesis::*;
}

/// Signal-safe temporary-file registry.
pub mod tempfile {
    pub use arc_store_view::tempfile::*;
}

/// Virtual views (branches) over the change DAG.
pub mod view {
    pub use arc_store_view::view::*;
}

pub use arc_store_types::author;
pub use arc_store_types::author::*;
pub use arc_store_types::newtypes;
pub use arc_store_types::newtypes::*;
pub use arc_store_types::refs;
pub use arc_store_types::refs::*;
pub use arc_store_types::tag;
pub use arc_store_types::tag::*;
