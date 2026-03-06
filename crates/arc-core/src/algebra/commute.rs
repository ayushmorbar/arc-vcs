use crate::algebra::{Atom, NodePath};
use crate::store::change::Change;

/// Two changes commute if and only if:
/// 1. Neither depends on the other (no causal ordering).
/// 2. Their atoms operate on disjoint AST subtrees.
///
/// Sharing a common dependency is harmless — two branches forked from the
/// same ancestor naturally share that ancestor in their dep sets. Only a
/// *direct* causal link (`a ∈ b.deps` or `b ∈ a.deps`) prevents commutativity.
pub fn commutes(a: &Change, b: &Change) -> bool {
    // Fast path: explicit causal dependency
    if b.deps.contains(&a.id) || a.deps.contains(&b.id) {
        return false;
    }

    // Atoms must touch disjoint AST paths
    atoms_disjoint(&a.atoms, &b.atoms)
}

/// Returns `true` when no atom in `a` touches any path reachable by an atom in `b`.
fn atoms_disjoint(atoms_a: &[Atom], atoms_b: &[Atom]) -> bool {
    for a in atoms_a {
        for b in atoms_b {
            for pa in a.paths() {
                for pb in b.paths() {
                    if paths_overlap(pa, pb) {
                        return false;
                    }
                }
            }
        }
    }
    true
}

/// Two AST paths overlap when one is a prefix of (or equal to) the other.
///
/// This models node ownership: deleting `["fn_foo"]` implicitly affects
/// everything beneath it (`["fn_foo", "body", "0"]`).
fn paths_overlap(a: &NodePath, b: &NodePath) -> bool {
    let min_len = a.len().min(b.len());
    a[..min_len] == b[..min_len]
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;
    use crate::algebra::Atom;

    /// Helper: build a `Change` whose atoms are all `Insert`s at the given paths.
    fn make_change(deps: HashSet<[u8; 32]>, paths: Vec<Vec<String>>) -> Change {
        let atoms = paths
            .into_iter()
            .map(|p| Atom::Insert {
                at: p,
                content: vec![],
            })
            .collect();
        let (author, signing_key) = crate::store::author::test_keypair();
        Change::new(deps, atoms, "test", author, &signing_key)
    }

    #[test]
    fn test_commutes_disjoint() {
        let a = make_change(HashSet::new(), vec![vec!["module_a".into(), "fn_x".into()]]);
        let b = make_change(HashSet::new(), vec![vec!["module_b".into(), "fn_y".into()]]);
        assert!(commutes(&a, &b), "disjoint changes must commute");
    }

    #[test]
    fn test_no_commute_overlapping_paths() {
        let a = make_change(HashSet::new(), vec![vec!["fn_foo".into()]]);
        let b = make_change(
            HashSet::new(),
            vec![vec!["fn_foo".into(), "body".into(), "0".into()]],
        );
        assert!(
            !commutes(&a, &b),
            "changes touching overlapping AST subtrees must NOT commute"
        );
    }

    #[test]
    fn test_no_commute_explicit_dependency() {
        let a = make_change(HashSet::new(), vec![vec!["mod_a".into()]]);
        // b explicitly depends on a
        let b = make_change(
            HashSet::from([a.id]),
            vec![vec!["mod_b".into()]],
        );
        assert!(
            !commutes(&a, &b),
            "changes with an explicit dependency must NOT commute"
        );
    }

    #[test]
    fn test_commutes_same_top_level_different_subtrees() {
        // Both operate under "module" but at completely different children
        let a = make_change(
            HashSet::new(),
            vec![vec!["module".into(), "child_a".into()]],
        );
        let b = make_change(
            HashSet::new(),
            vec![vec!["module".into(), "child_b".into()]],
        );
        assert!(
            commutes(&a, &b),
            "changes under different children of the same parent must commute"
        );
    }
}
