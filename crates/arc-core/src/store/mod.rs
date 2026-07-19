//! arc object store: CAS, changes, graph, views, author identity, and oplog.

pub use arc_change::change;
pub use arc_change::change::*;
pub use arc_change::content_hash;
pub use arc_change::content_hash::*;
pub use arc_store_cas::blake3_hasher;
pub use arc_store_cas::blake3_hasher::{Blake3HashMap, Blake3Hasher};
pub use arc_store_cas::cas;
pub use arc_store_cas::cas::CasBytes;
pub use arc_store_cas::cas::*;
pub use arc_store_graph::*;
pub use arc_store_view::StoreError;
pub use arc_store_view::oplog::*;
pub use arc_store_view::synthesis::*;
pub use arc_store_view::tempfile::*;
pub use arc_store_view::view::*;

/// Append-only spacetime operation log for O(1) undo.
pub mod oplog;

/// Redb metadata store for OpLog/index persistence (content remains in CAS).
#[cfg(all(feature = "redb-metadata", not(target_arch = "wasm32")))]
pub mod redb_metadata;

#[cfg(all(feature = "redb-metadata", not(target_arch = "wasm32")))]
/// Initialize metadata-only Redb storage under the repository's `.arc` directory.
pub fn init_metadata_backend(
    root: &std::path::Path,
) -> Result<redb_metadata::MetadataStore, redb_metadata::MetadataError> {
    redb_metadata::MetadataStore::open(root)
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
