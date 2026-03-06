use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use crate::algebra::{Atom, Blake3Hash};

/// An atomic, replayable change — the fundamental unit in arc.
///
/// A `Change` bundles one or more [`Atom`]s into a single semantic operation
/// whose identity is the BLAKE3 hash of its deterministically-serialized
/// content (sorted deps + atoms).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Change {
    /// Content-addressed identity (BLAKE3 hash of `(sorted_deps, atoms)`).
    pub id: Blake3Hash,
    /// The set of change IDs this change depends on (partial order edges).
    pub deps: HashSet<Blake3Hash>,
    /// The ordered list of AST-level atoms that compose this change.
    pub atoms: Vec<Atom>,
}

impl Change {
    /// Create a new `Change`, computing its content-addressed `id` from
    /// the given dependencies and atoms.
    ///
    /// Dependencies are sorted before hashing so the `id` is deterministic
    /// regardless of `HashSet` iteration order.
    pub fn new(deps: HashSet<Blake3Hash>, atoms: Vec<Atom>) -> Self {
        let id = Self::compute_id(&deps, &atoms);
        Self { id, deps, atoms }
    }

    /// Deterministic id derivation: `blake3(bincode(sorted_deps, atoms))`.
    fn compute_id(deps: &HashSet<Blake3Hash>, atoms: &Vec<Atom>) -> Blake3Hash {
        let mut sorted_deps: Vec<&Blake3Hash> = deps.iter().collect();
        sorted_deps.sort();

        let payload =
            bincode::serialize(&(&sorted_deps, atoms)).expect("bincode serialization is infallible for these types");

        *blake3::hash(&payload).as_bytes()
    }
}
