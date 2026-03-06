use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use crate::algebra::{Atom, Blake3Hash};

/// An atomic, replayable change — the fundamental unit in arc.
///
/// A `Change` bundles one or more [`Atom`]s into a single semantic operation
/// whose identity is the BLAKE3 hash of its deterministically-serialized
/// content (sorted deps + atoms + intent).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Change {
    /// Content-addressed identity (BLAKE3 hash of `(sorted_deps, atoms, intent)`).
    pub id: Blake3Hash,
    /// The set of change IDs this change depends on (partial order edges).
    pub deps: HashSet<Blake3Hash>,
    /// The ordered list of AST-level atoms that compose this change.
    pub atoms: Vec<Atom>,
    /// Human- or AI-supplied semantic intent (commit message / goal).
    pub intent: String,
}

impl Change {
    /// Create a new `Change`, computing its content-addressed `id` from
    /// the given dependencies, atoms, and intent.
    ///
    /// Dependencies are sorted before hashing so the `id` is deterministic
    /// regardless of `HashSet` iteration order.
    pub fn new(deps: HashSet<Blake3Hash>, atoms: Vec<Atom>, intent: impl Into<String>) -> Self {
        let intent = intent.into();
        let id = Self::compute_id(&deps, &atoms, &intent);
        Self { id, deps, atoms, intent }
    }

    /// Deterministic id derivation: `blake3(bincode(sorted_deps, atoms, intent))`.
    ///
    /// **Crypto invariant**: `intent` MUST be included in the hash payload.
    /// Omitting it would let an attacker rewrite intent strings without
    /// changing the CAS address, breaking content-addressable integrity.
    fn compute_id(deps: &HashSet<Blake3Hash>, atoms: &[Atom], intent: &str) -> Blake3Hash {
        let mut sorted_deps: Vec<&Blake3Hash> = deps.iter().collect();
        sorted_deps.sort();

        let payload =
            bincode::serialize(&(&sorted_deps, atoms, intent)).expect("bincode serialization is infallible for these types");

        *blake3::hash(&payload).as_bytes()
    }
}
