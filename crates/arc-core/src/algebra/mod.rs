pub use arc_algebra::BlobStore;
pub use arc_algebra::commute;
pub use arc_algebra::sparse;

use crate::store::cas::ObjectStore;
use arc_algebra_types::Blake3Hash as AlgebraBlake3Hash;

struct CoreBlobStore<'a>(&'a ObjectStore);

impl arc_algebra::BlobStore for CoreBlobStore<'_> {
    fn read_blob(&self, hash: &AlgebraBlake3Hash) -> Result<Vec<u8>, String> {
        self.0
            .read_blob(hash)
            .map(|bytes| bytes.to_vec())
            .map_err(|e| format!("{e}"))
    }

    fn contains_blob(&self, hash: &AlgebraBlake3Hash) -> bool {
        self.0.contains_blob(hash)
    }
}

/// Algebra application facade with backward-compatible `ObjectStore` adapters.
pub mod apply {
    pub use arc_algebra::apply::*;

    use arc_change::Change;
    use ignore::gitignore::Gitignore;

    use super::CoreBlobStore;
    use crate::store::cas::ObjectStore;

    /// Backward-compatible facade: apply a single change using `ObjectStore`.
    pub fn apply_change(
        state: &mut MaterializedState,
        change: &Change,
        store: &ObjectStore,
        agent_ignore: &Gitignore,
        blame: Option<&mut BlameState>,
    ) -> Result<(), String> {
        arc_algebra::apply::apply_change(state, change, &CoreBlobStore(store), agent_ignore, blame)
    }

    /// Backward-compatible facade: sparse-aware change application.
    pub fn apply_change_scoped(
        state: &mut MaterializedState,
        change: &Change,
        store: &ObjectStore,
        agent_ignore: &Gitignore,
        sparse: Option<&arc_algebra::sparse::SparseMatcher>,
        blame: Option<&mut BlameState>,
    ) -> Result<(), String> {
        arc_algebra::apply::apply_change_scoped(
            state,
            change,
            &CoreBlobStore(store),
            agent_ignore,
            sparse,
            blame,
        )
    }
}

/// Algebra inversion facade with backward-compatible `ObjectStore` adapters.
pub mod inverse {
    pub use arc_algebra::inverse::*;

    use arc_change::Change;
    use arc_store_types::author::Author;

    use super::CoreBlobStore;
    use crate::store::cas::ObjectStore;

    /// Backward-compatible facade: invert an atom with `ObjectStore` availability checks.
    pub fn invert_atom(
        atom: &arc_algebra_types::Atom,
        store: &ObjectStore,
    ) -> Result<arc_algebra_types::Atom, arc_algebra::inverse::InvertError> {
        arc_algebra::inverse::invert_atom(atom, &CoreBlobStore(store))
    }

    /// Backward-compatible facade: invert a change with `ObjectStore` availability checks.
    pub fn invert_change(
        change: &Change,
        store: &ObjectStore,
        signer: &(Author, ed25519_dalek::SigningKey),
    ) -> Result<Change, arc_algebra::inverse::InvertError> {
        arc_algebra::inverse::invert_change(change, &CoreBlobStore(store), signer)
    }
}

pub use arc_algebra_types::{Atom, Blake3Hash, NodePath, SpacetimeCoordinate};
