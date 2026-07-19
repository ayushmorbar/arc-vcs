//! Spacetime operations: algebraic history rewriting.
//!
//! # `squash_into`
//!
//! Fuses a contiguous linear spine of changes (from an ancestor `target_id`
//! to the current view heads) into a single new [`Change`].  The resulting
//! change retains the original target's `deps` and carries all atoms from
//! every change in the spine.
//!
//! # Error model
//!
//! [`SpacetimeError::NonLinearSpine`] is returned when the chain between
//! `target_id` and `view_heads` contains a fork (more than one child at any
//! node) — this makes the squash ambiguous.
//!
//! [`SpacetimeError::TargetNotFound`] is returned when `target_id` does not
//! exist in the graph.

use std::collections::HashSet;

use arc_algebra_types::Blake3Hash;
use arc_change::Change;
use arc_store_cas::ObjectStore;
use arc_store_graph::ChangeGraph;
use arc_store_types::author::Author;
use thiserror::Error;

/// Errors produced by spacetime operations.
#[derive(Debug, Error)]
pub enum SpacetimeError {
    /// The spine between `target_id` and view heads is not a single linear
    /// chain — a fork was detected at the given change ID.
    #[error("non-linear spine: change {0:?} has multiple children")]
    NonLinearSpine(Blake3Hash),
    /// The target change ID is not present in the graph.
    #[error("target change {0:?} not found in graph")]
    TargetNotFound(Blake3Hash),
    /// The target change is not an ancestor of any view head.
    #[error("target change {0:?} is not an ancestor of the view heads")]
    TargetNotAncestor(Blake3Hash),
}

/// Squash the contiguous linear spine from `target_id` to `view_heads` into a
/// single new [`Change`].
///
/// # Guarantees
///
/// - The squashed change carries the same `deps` as the original `target_id`.
/// - Its atoms are the concatenation of all atoms from every change in the spine, in topological
///   (application) order.
/// - The intent is `"Squash: {count} changes into {target_intent}"`.
/// - The returned change is signed with `signer`.
///
/// # Errors
///
/// Returns [`SpacetimeError::NonLinearSpine`] when any node in the chain has
/// more than one descendant within the reachable head set.
///
/// Returns [`SpacetimeError::TargetNotFound`] when `target_id` is absent from
/// the graph.
pub fn squash_into(
    graph: &ChangeGraph,
    _store: &ObjectStore,
    view_heads: &HashSet<Blake3Hash>,
    target_id: Blake3Hash,
    signer: &(Author, ed25519_dalek::SigningKey),
) -> Result<Change, SpacetimeError> {
    let target = graph.get(&target_id).ok_or(SpacetimeError::TargetNotFound(target_id))?;

    // Verify target is an ancestor of the view heads.
    let all_ancestors = graph.ancestors(view_heads);
    if !all_ancestors.contains(&target_id) && !view_heads.contains(&target_id) {
        return Err(SpacetimeError::TargetNotAncestor(target_id));
    }

    // Collect the linear chain from target_id to view_heads using topological sort.
    let topo_order = graph.topological_sort(view_heads);

    // Find the position of target_id in the topological order.
    let start_idx = topo_order
        .iter()
        .position(|id| *id == target_id)
        .ok_or(SpacetimeError::TargetNotFound(target_id))?;

    // Extract the spine: target + all changes after it (in order).
    let spine: Vec<Blake3Hash> = topo_order[start_idx..].to_vec();

    // Verify linearity: each change in the spine (except the last) must have
    // exactly one direct child that is also in the spine.  A "direct child"
    // of `id` is any spine member whose `deps` set contains `id`.
    for (i, &id) in spine[..spine.len().saturating_sub(1)].iter().enumerate() {
        let children_in_spine = spine[i + 1..]
            .iter()
            .filter(|&&candidate| graph.get(&candidate).is_some_and(|c| c.deps.contains(&id)))
            .count();

        if children_in_spine > 1 {
            return Err(SpacetimeError::NonLinearSpine(id));
        }
    }

    // Collect all atoms from the spine in order.
    let mut all_atoms = Vec::new();
    for &id in &spine {
        if let Some(change) = graph.get(&id) {
            all_atoms.extend(change.atoms.clone());
        }
    }

    let count = spine.len();
    let intent = format!("Squash: {} changes into \"{}\"", count, target.intent);

    let (author, signing_key) = signer;
    let squashed = Change::new(target.deps.clone(), all_atoms, intent, author.clone(), signing_key);

    Ok(squashed)
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use arc_algebra_types::Atom;
    use arc_store_types::author::test_keypair;

    use super::*;

    fn make_store() -> (tempfile::TempDir, ObjectStore) {
        let dir = tempfile::tempdir().unwrap();
        let store = ObjectStore::new(dir.path());
        (dir, store)
    }

    fn make_change_with_hash(
        deps: HashSet<Blake3Hash>,
        label: &str,
        content_hash: Blake3Hash,
    ) -> Change {
        let (author, signing_key) = test_keypair();
        Change::new(
            deps,
            vec![Atom::Insert { at: vec![label.to_string()], content_hash }],
            label,
            author,
            &signing_key,
        )
    }

    #[test]
    fn test_squash_linear_chain() {
        let (_dir, store) = make_store();
        let hash_a = store.write_blob(b"a").unwrap();
        let hash_b = store.write_blob(b"b").unwrap();
        let hash_c = store.write_blob(b"c").unwrap();

        let a = make_change_with_hash(HashSet::new(), "change_a", hash_a);
        let b = make_change_with_hash(HashSet::from([a.id]), "change_b", hash_b);
        let c = make_change_with_hash(HashSet::from([b.id]), "change_c", hash_c);

        let mut graph = ChangeGraph::new();
        graph.add_change(a.clone());
        graph.add_change(b.clone());
        graph.add_change(c.clone());

        let (author, signing_key) = test_keypair();
        let view_heads: HashSet<Blake3Hash> = HashSet::from([c.id]);

        let squashed = squash_into(&graph, &store, &view_heads, a.id, &(author, signing_key))
            .expect("linear chain must squash without error");

        // Squashed change must carry all 3 atoms.
        assert_eq!(squashed.atoms.len(), 3, "squashed change must contain all atoms");
        // Must have the same deps as the target (a has empty deps).
        assert!(squashed.deps.is_empty(), "squashed must inherit target's deps");
        // Must be cryptographically valid.
        assert!(squashed.verify_signature(), "squashed change must have valid signature");
        assert!(squashed.intent.contains("Squash"), "intent must mention Squash");
    }

    #[test]
    fn test_squash_target_not_found_errors() {
        let (_dir, store) = make_store();
        let graph = ChangeGraph::new();
        let (author, signing_key) = test_keypair();
        let nonexistent: Blake3Hash = [0xFF; 32];
        let heads: HashSet<Blake3Hash> = HashSet::new();

        let result = squash_into(&graph, &store, &heads, nonexistent, &(author, signing_key));
        assert!(
            matches!(result, Err(SpacetimeError::TargetNotFound(_))),
            "missing target must yield TargetNotFound"
        );
    }
}
