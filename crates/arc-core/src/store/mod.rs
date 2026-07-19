//! arc object store: CAS, changes, graph, views, author identity, and oplog.

pub use arc_change::{change, change::*, content_hash, content_hash::*};
pub use arc_store_cas::{
    blake3_hasher,
    blake3_hasher::{Blake3HashMap, Blake3Hasher},
    cas,
    cas::{CasBytes, *},
};
pub use arc_store_graph::*;
pub use arc_store_view::{StoreError, oplog::*, synthesis::*, tempfile::*, view::*};

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

pub use arc_store_types::{author, author::*, newtypes, newtypes::*, refs, refs::*, tag, tag::*};
