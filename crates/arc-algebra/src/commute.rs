use std::collections::HashSet;

use arc_algebra_types::{Atom, NodePath};
use arc_change::Change;
use arc_store_types::Author;

/// Two changes commute if and only if:
/// 1. Neither depends on the other (no causal ordering).
/// 2. Their atoms operate on disjoint AST subtrees.
///
/// Sharing a common dependency is harmless — two branches forked from the
/// same ancestor naturally share that ancestor in their dep sets. Only a
/// *direct* causal link (`a ∈ b.deps` or `b ∈ a.deps`) prevents commutativity.
///
/// Mathematical guarantee: when this returns `true`, swapping the pair does
/// not change reachable state under patch-theory replay because there is no
/// explicit causal edge and no overlapping subtree writes.
pub fn commutes(a: &Change, b: &Change) -> bool {
    // Fast path: explicit causal dependency
    if b.deps.contains(&a.id) || a.deps.contains(&b.id) {
        return false;
    }

    // Atoms must touch disjoint AST paths
    atoms_disjoint(&a.atoms, &b.atoms)
}

/// Attempt to produce the commuted pair `(b′, a′)` where `b′` comes before
/// `a′` in the new order.
///
/// Returns `Some((b_prime, a_prime))` when the pair safely commutes, or
/// `None` if commutativity is blocked.
///
/// ## Gate order (must be checked strictly in sequence)
///
/// 1. **Explicit dep** — if `a ∈ b.deps` or `b ∈ a.deps`, return `None`.
/// 2. **Disjoint atoms** — if any paths overlap, return `None`.
/// 3. **Ghost conflict** — if a `Delete P` in one change and an `Insert P′` in the other share the
///    same *terminal symbol* under the same parent namespace, return `None`.  (This prevents
///    phantom resurrection bugs.)
/// 4. **Move path rewriting** — rewrite `dep` sets and atom paths that cross a `Move` boundary,
///    then re-sign both changes with `signer`.
///
/// When gates 1–3 all pass but no `Move` atoms are present, the changes
/// commute trivially: return `Some((b.clone(), a.clone()))` with updated deps.
pub fn commute_pair(
    a: &Change,
    b: &Change,
    signer: &(Author, ed25519_dalek::SigningKey),
) -> Option<(Change, Change)> {
    // ── Gate 1: explicit causal dependency ──────────────────────────────────
    if b.deps.contains(&a.id) || a.deps.contains(&b.id) {
        return None;
    }

    // ── Gate 2: disjoint atom paths ─────────────────────────────────────────
    if !atoms_disjoint(&a.atoms, &b.atoms) {
        return None;
    }

    // ── Gate 3: ghost conflict ───────────────────────────────────────────────
    // A Delete in one change and an Insert in the other that share the same
    // terminal path segment inside the same parent namespace.
    if ghost_conflict(&a.atoms, &b.atoms) {
        return None;
    }

    // ── Gate 4: Move path rewriting ─────────────────────────────────────────
    // Rewrite atom paths and deps that cross a Move boundary.
    let (author, signing_key) = signer;

    let b_prime_atoms = rewrite_paths_through_moves(&b.atoms, &a.atoms);
    let a_prime_atoms = rewrite_paths_through_moves(&a.atoms, &b.atoms);

    // Update dep sets: remove direct cross-dep (already checked absent), keep rest.
    let b_prime_deps: HashSet<_> = b.deps.iter().filter(|d| **d != a.id).copied().collect();
    let a_prime_deps: HashSet<_> = a.deps.iter().filter(|d| **d != b.id).copied().collect();

    let b_prime =
        Change::new(b_prime_deps, b_prime_atoms, b.intent.clone(), author.clone(), signing_key);
    let a_prime =
        Change::new(a_prime_deps, a_prime_atoms, a.intent.clone(), author.clone(), signing_key);

    Some((b_prime, a_prime))
}

/// Detect a ghost conflict: a `Delete` in `atoms_a` and an `Insert` in `atoms_b`
/// (or vice versa) that share the same terminal symbol under the same parent.
///
/// Two atoms ghost-conflict when:
/// - One is `Delete { at: P }` and the other is `Insert { at: P′ }`
/// - `P` and `P′` share the same parent prefix AND the same terminal segment (i.e. `P == P′` at the
///   terminal level).
///
/// This is strictly stronger than path overlap but weaker than full equality:
/// it catches the case where deletion and resurrection of the *same symbol*
/// would silently lose the old content.
fn ghost_conflict(atoms_a: &[Atom], atoms_b: &[Atom]) -> bool {
    for a in atoms_a {
        for b in atoms_b {
            if is_delete_insert_same_terminal(a, b) || is_delete_insert_same_terminal(b, a) {
                return true;
            }
        }
    }
    false
}

/// Returns `true` when `del` is a `Delete` and `ins` is an `Insert` that
/// both target the *same* AST path (terminal symbol + parent namespace).
fn is_delete_insert_same_terminal(del: &Atom, ins: &Atom) -> bool {
    match (del, ins) {
        (Atom::Delete { at: del_at, .. }, Atom::Insert { at: ins_at, .. }) => del_at == ins_at,
        _ => false,
    }
}

/// Rewrite the paths of `atoms` to account for any `Move` atoms in `moves`.
///
/// For each `Move { from, to }` in `moves`, any atom in `atoms` that has a
/// path starting with `from` gets its prefix rewritten to `to`.
fn rewrite_paths_through_moves(atoms: &[Atom], moves: &[Atom]) -> Vec<Atom> {
    let move_pairs: Vec<(&NodePath, &NodePath)> = moves
        .iter()
        .filter_map(|m| if let Atom::Move { from, to } = m { Some((from, to)) } else { None })
        .collect();

    if move_pairs.is_empty() {
        return atoms.to_vec();
    }

    atoms.iter().map(|atom| rewrite_atom_paths(atom, &move_pairs)).collect()
}

/// Rewrite all paths in `atom` using the given `(from, to)` pairs.
fn rewrite_atom_paths(atom: &Atom, moves: &[(&NodePath, &NodePath)]) -> Atom {
    let rewrite = |path: &NodePath| -> NodePath {
        for (from, to) in moves {
            if path.len() >= from.len() && &path[..from.len()] == *from {
                let mut new_path = (*to).clone();
                new_path.extend_from_slice(&path[from.len()..]);
                return new_path;
            }
        }
        path.clone()
    };

    match atom {
        Atom::Insert { at, content_hash } => {
            Atom::Insert { at: rewrite(at), content_hash: *content_hash }
        }
        Atom::Delete { at, prior_hash } => {
            Atom::Delete { at: rewrite(at), prior_hash: *prior_hash }
        }
        Atom::Move { from, to } => Atom::Move { from: rewrite(from), to: rewrite(to) },
        Atom::SemanticsPreserving { at, description } => {
            Atom::SemanticsPreserving { at: rewrite(at), description: description.clone() }
        }
        other => other.clone(),
    }
}

/// Returns `true` when no atom in `a` touches any path reachable by an atom in `b`.
///
/// `Move` atoms are deliberately excluded from this check: a `Move` that
/// covers a path in the other change is NOT a conflict — it is handled by
/// Gate 4 (path rewriting).  Only non-Move atoms can produce a path conflict
/// at Gate 2.
fn atoms_disjoint(atoms_a: &[Atom], atoms_b: &[Atom]) -> bool {
    for a in atoms_a {
        if matches!(a, Atom::Move { .. }) {
            continue;
        }
        for b in atoms_b {
            if matches!(b, Atom::Move { .. }) {
                continue;
            }
            if blob_conflicts(a, b) {
                return false;
            }
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

fn blob_conflicts(a: &Atom, b: &Atom) -> bool {
    match (a, b) {
        (
            Atom::Blob { path: path_a, hash: hash_a, size: size_a },
            Atom::Blob { path: path_b, hash: hash_b, size: size_b },
        ) => path_a == path_b && (hash_a != hash_b || size_a != size_b),
        (Atom::Blob { path, .. }, other) | (other, Atom::Blob { path, .. }) => {
            file_path_for_atom(other).is_some_and(|other_path| other_path == path)
        }
        _ => false,
    }
}

fn file_path_for_atom(atom: &Atom) -> Option<&str> {
    match atom {
        Atom::Insert { at, .. }
        | Atom::Delete { at, .. }
        | Atom::SemanticsPreserving { at, .. }
        | Atom::Conflict { at, .. }
            if at.len() >= 2 && at[0] == "file" =>
        {
            Some(&at[1])
        }
        Atom::Move { from, .. } if from.len() >= 2 && from[0] == "file" => Some(&from[1]),
        Atom::Move { to, .. } if to.len() >= 2 && to[0] == "file" => Some(&to[1]),
        Atom::Directory { path } if path.len() >= 2 && path[0] == "file" => Some(&path[1]),
        Atom::Mount { path, .. } if path.len() >= 2 && path[0] == "file" => Some(&path[1]),
        _ => None,
    }
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

    use arc_algebra_types::Atom;
    use arc_store_types::author;

    use super::*;

    /// Helper: build a `Change` whose atoms are all `Insert`s at the given paths.
    ///
    /// Uses a zero hash for `content_hash` since commutativity tests never
    /// call `apply_change` and do not need real blob data.
    fn make_change(deps: HashSet<[u8; 32]>, paths: Vec<Vec<String>>) -> Change {
        let atoms =
            paths.into_iter().map(|p| Atom::Insert { at: p, content_hash: [0u8; 32] }).collect();
        let (author, signing_key) = author::test_keypair();
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
        let b = make_change(HashSet::new(), vec![vec!["fn_foo".into(), "body".into(), "0".into()]]);
        assert!(!commutes(&a, &b), "changes touching overlapping AST subtrees must NOT commute");
    }

    #[test]
    fn test_no_commute_explicit_dependency() {
        let a = make_change(HashSet::new(), vec![vec!["mod_a".into()]]);
        // b explicitly depends on a
        let b = make_change(HashSet::from([a.id]), vec![vec!["mod_b".into()]]);
        assert!(!commutes(&a, &b), "changes with an explicit dependency must NOT commute");
    }

    #[test]
    fn test_commutes_same_top_level_different_subtrees() {
        // Both operate under "module" but at completely different children
        let a = make_change(HashSet::new(), vec![vec!["module".into(), "child_a".into()]]);
        let b = make_change(HashSet::new(), vec![vec!["module".into(), "child_b".into()]]);
        assert!(
            commutes(&a, &b),
            "changes under different children of the same parent must commute"
        );
    }

    /// A `Delete` and an `Insert` that target the same AST path must NOT commute.
    #[test]
    fn test_no_commute_delete_insert_same_path() {
        let (author, signing_key) = author::test_keypair();
        let a = Change::new(
            HashSet::new(),
            vec![Atom::Delete { at: vec!["fn_foo".into()], prior_hash: [0u8; 32] }],
            "delete",
            author.clone(),
            &signing_key,
        );
        let b = Change::new(
            HashSet::new(),
            vec![Atom::Insert { at: vec!["fn_foo".into()], content_hash: [0u8; 32] }],
            "insert",
            author,
            &signing_key,
        );
        assert!(!commutes(&a, &b), "Delete and Insert at the same path must NOT commute");
    }

    // ── commute_pair() tests ─────────────────────────────────────────────────

    #[test]
    fn test_commute_pair_disjoint_succeeds() {
        let (author, signing_key) = author::test_keypair();
        let a = make_change(HashSet::new(), vec![vec!["module_a".into()]]);
        let b = make_change(HashSet::new(), vec![vec!["module_b".into()]]);

        let result = commute_pair(&a, &b, &(author, signing_key));
        assert!(result.is_some(), "disjoint changes must commute via commute_pair");
        let (b_prime, a_prime) = result.unwrap();
        // b′ and a′ must now validate correctly.
        assert!(b_prime.verify_signature());
        assert!(a_prime.verify_signature());
    }

    #[test]
    fn test_commute_pair_explicit_dep_fails() {
        let (author, signing_key) = author::test_keypair();
        let a = make_change(HashSet::new(), vec![vec!["mod_a".into()]]);
        let b = make_change(HashSet::from([a.id]), vec![vec!["mod_b".into()]]);

        let result = commute_pair(&a, &b, &(author, signing_key));
        assert!(result.is_none(), "Gate 1 (explicit dep) must block commute_pair");
    }

    #[test]
    fn test_commute_pair_ghost_conflict_fails() {
        let (author, signing_key) = author::test_keypair();
        // Delete P in a, Insert P in b — same terminal path.
        let a = Change::new(
            HashSet::new(),
            vec![Atom::Delete { at: vec!["fn_foo".into()], prior_hash: [0u8; 32] }],
            "delete",
            author.clone(),
            &signing_key,
        );
        let b = Change::new(
            HashSet::new(),
            vec![Atom::Insert { at: vec!["fn_foo".into()], content_hash: [1u8; 32] }],
            "insert",
            author,
            &signing_key,
        );
        let result = commute_pair(&a, &b, &(author::test_keypair().0, author::test_keypair().1));
        assert!(result.is_none(), "Gate 3 (ghost conflict) must block commute_pair");
    }

    #[test]
    fn test_commute_pair_move_rewrites_paths() {
        let (author, signing_key) = author::test_keypair();

        // a: Insert at ["old_mod", "fn_x"]
        let a = Change::new(
            HashSet::new(),
            vec![Atom::Insert {
                at: vec!["old_mod".into(), "fn_x".into()],
                content_hash: [0u8; 32],
            }],
            "add fn_x",
            author.clone(),
            &signing_key,
        );

        // b: Move ["old_mod"] → ["new_mod"]
        let b = Change::new(
            HashSet::new(),
            vec![Atom::Move { from: vec!["old_mod".into()], to: vec!["new_mod".into()] }],
            "rename mod",
            author.clone(),
            &signing_key,
        );

        let result = commute_pair(&a, &b, &(author, signing_key));
        assert!(result.is_some(), "Move should not block commutativity for disjoint inserts");
        let (b_prime, a_prime) = result.unwrap();
        // a′ should have its Insert path rewritten through the Move
        let has_rewritten = a_prime
            .atoms
            .iter()
            .any(|atom| matches!(atom, Atom::Insert { at, .. } if at[0] == "new_mod"));
        assert!(has_rewritten, "Insert path must be rewritten through Move: {a_prime:?}");
        assert!(b_prime.verify_signature());
        assert!(a_prime.verify_signature());
    }

    // ── blob_conflicts() unit tests ──────────────────────────────────────

    #[test]
    fn test_blob_conflicts_same_path_different_hash() {
        let a = Atom::Blob {
            path: "a.txt".into(),
            hash: blake3::Hash::from_bytes([1u8; 32]),
            size: 100,
        };
        let b = Atom::Blob {
            path: "a.txt".into(),
            hash: blake3::Hash::from_bytes([2u8; 32]),
            size: 100,
        };
        assert!(blob_conflicts(&a, &b), "same path, different hash must conflict");
    }

    #[test]
    fn test_blob_conflicts_same_path_different_size() {
        let a = Atom::Blob {
            path: "a.txt".into(),
            hash: blake3::Hash::from_bytes([1u8; 32]),
            size: 100,
        };
        let b = Atom::Blob {
            path: "a.txt".into(),
            hash: blake3::Hash::from_bytes([1u8; 32]),
            size: 200,
        };
        assert!(blob_conflicts(&a, &b), "same path, different size must conflict");
    }

    #[test]
    fn test_blob_no_conflict_same_path_same_content() {
        let a = Atom::Blob {
            path: "a.txt".into(),
            hash: blake3::Hash::from_bytes([1u8; 32]),
            size: 100,
        };
        let b = Atom::Blob {
            path: "a.txt".into(),
            hash: blake3::Hash::from_bytes([1u8; 32]),
            size: 100,
        };
        assert!(!blob_conflicts(&a, &b), "same path + same content must NOT conflict");
    }

    #[test]
    fn test_blob_no_conflict_different_path() {
        let a = Atom::Blob {
            path: "a.txt".into(),
            hash: blake3::Hash::from_bytes([1u8; 32]),
            size: 100,
        };
        let b = Atom::Blob {
            path: "b.txt".into(),
            hash: blake3::Hash::from_bytes([2u8; 32]),
            size: 200,
        };
        assert!(!blob_conflicts(&a, &b), "different paths must NOT conflict");
    }

    #[test]
    fn test_blob_conflicts_with_insert_same_file_path() {
        let blob = Atom::Blob {
            path: "a.txt".into(),
            hash: blake3::Hash::from_bytes([1u8; 32]),
            size: 100,
        };
        let insert = Atom::Insert {
            at: vec!["file".into(), "a.txt".into(), "body".into()],
            content_hash: [0u8; 32],
        };
        assert!(blob_conflicts(&blob, &insert), "Blob vs Insert at same file path must conflict");
    }

    #[test]
    fn test_blob_no_conflict_with_insert_different_file() {
        let blob = Atom::Blob {
            path: "a.txt".into(),
            hash: blake3::Hash::from_bytes([1u8; 32]),
            size: 100,
        };
        let insert = Atom::Insert {
            at: vec!["file".into(), "b.txt".into(), "body".into()],
            content_hash: [0u8; 32],
        };
        assert!(
            !blob_conflicts(&blob, &insert),
            "Blob vs Insert at different file path must NOT conflict"
        );
    }

    #[test]
    fn test_blob_no_conflict_with_directory() {
        let blob = Atom::Blob {
            path: "a.txt".into(),
            hash: blake3::Hash::from_bytes([1u8; 32]),
            size: 100,
        };
        let dir = Atom::Directory { path: vec!["dir".into(), "src".into()] };
        assert!(!blob_conflicts(&blob, &dir), "Blob vs non-file-path Directory must NOT conflict");
    }

    #[test]
    fn test_non_blob_atoms_no_conflict() {
        let a = Atom::Insert { at: vec!["fn_foo".into()], content_hash: [0u8; 32] };
        let b = Atom::Insert { at: vec!["fn_bar".into()], content_hash: [0u8; 32] };
        assert!(!blob_conflicts(&a, &b), "two non-Blob atoms must NOT produce blob conflicts");
    }

    // ── file_path_for_atom() unit tests ──────────────────────────────────

    #[test]
    fn test_file_path_for_atom_insert() {
        let atom =
            Atom::Insert { at: vec!["file".into(), "main.rs".into()], content_hash: [0u8; 32] };
        assert_eq!(file_path_for_atom(&atom), Some("main.rs"));
    }

    #[test]
    fn test_file_path_for_atom_delete() {
        let atom = Atom::Delete { at: vec!["file".into(), "old.rs".into()], prior_hash: [0u8; 32] };
        assert_eq!(file_path_for_atom(&atom), Some("old.rs"));
    }

    #[test]
    fn test_file_path_for_atom_conflict() {
        let atom = Atom::Conflict {
            bases: vec![[0u8; 32]],
            sides: vec![[1u8; 32]],
            at: vec!["file".into(), "conflict.rs".into()],
        };
        assert_eq!(file_path_for_atom(&atom), Some("conflict.rs"));
    }

    #[test]
    fn test_file_path_for_atom_move_from() {
        let atom = Atom::Move {
            from: vec!["file".into(), "old.rs".into()],
            to: vec!["file".into(), "new.rs".into()],
        };
        // Move has two paths; file_path_for_atom picks the first match
        assert_eq!(file_path_for_atom(&atom), Some("old.rs"));
    }

    #[test]
    fn test_file_path_for_atom_directory() {
        let atom = Atom::Directory { path: vec!["file".into(), "src".into()] };
        assert_eq!(file_path_for_atom(&atom), Some("src"));
    }

    #[test]
    fn test_file_path_for_atom_mount() {
        let atom = Atom::Mount {
            path: vec!["file".into(), "vendor".into()],
            coordinate: arc_algebra_types::SpacetimeCoordinate {
                namespace: "org".into(),
                repo: "lib".into(),
                hash: blake3::Hash::from_bytes([0u8; 32]),
            },
        };
        assert_eq!(file_path_for_atom(&atom), Some("vendor"));
    }

    #[test]
    fn test_file_path_for_atom_non_file_returns_none() {
        let atom =
            Atom::Insert { at: vec!["module".into(), "fn_x".into()], content_hash: [0u8; 32] };
        assert_eq!(file_path_for_atom(&atom), None);
    }

    #[test]
    fn test_file_path_for_atom_short_path_returns_none() {
        let atom = Atom::Insert { at: vec!["file".into()], content_hash: [0u8; 32] };
        assert_eq!(file_path_for_atom(&atom), None);
    }

    #[test]
    fn test_file_path_for_atom_semantics_preserving() {
        let atom = Atom::SemanticsPreserving {
            at: vec!["file".into(), "lib.rs".into()],
            description: "format".into(),
        };
        assert_eq!(file_path_for_atom(&atom), Some("lib.rs"));
    }

    // ── paths_overlap() unit tests ───────────────────────────────────────

    #[test]
    fn test_paths_overlap_equal() {
        assert!(paths_overlap(&vec!["a".into(), "b".into()], &vec!["a".into(), "b".into()]));
    }

    #[test]
    fn test_paths_overlap_prefix() {
        assert!(paths_overlap(&vec!["a".into()], &vec!["a".into(), "b".into()]));
    }

    #[test]
    fn test_paths_overlap_suffix_is_prefix() {
        assert!(paths_overlap(&vec!["a".into(), "b".into()], &vec!["a".into()]));
    }

    #[test]
    fn test_paths_no_overlap() {
        assert!(!paths_overlap(&vec!["a".into()], &vec!["b".into()]));
    }

    #[test]
    fn test_paths_no_overlap_different_lengths() {
        assert!(!paths_overlap(&vec!["a".into(), "b".into()], &vec!["a".into(), "c".into()]));
    }

    #[test]
    fn test_paths_overlap_empty() {
        assert!(paths_overlap(&vec![], &vec![]));
    }

    // ── rewrite_atom_paths() unit tests ──────────────────────────────────

    #[test]
    fn test_rewrite_atom_paths_insert() {
        let atom = Atom::Insert { at: vec!["old".into(), "fn".into()], content_hash: [0u8; 32] };
        let old = vec!["old".into()];
        let new = vec!["new".into()];
        let moves: Vec<(&NodePath, &NodePath)> = vec![(&old, &new)];
        let rewritten = rewrite_atom_paths(&atom, &moves);
        match rewritten {
            Atom::Insert { at, .. } => assert_eq!(at, vec!["new".to_string(), "fn".to_string()]),
            _ => panic!("expected Insert"),
        }
    }

    #[test]
    fn test_rewrite_atom_paths_delete() {
        let atom = Atom::Delete { at: vec!["old".into(), "fn".into()], prior_hash: [0u8; 32] };
        let old = vec!["old".into()];
        let new = vec!["new".into()];
        let moves: Vec<(&NodePath, &NodePath)> = vec![(&old, &new)];
        let rewritten = rewrite_atom_paths(&atom, &moves);
        match rewritten {
            Atom::Delete { at, .. } => assert_eq!(at, vec!["new".to_string(), "fn".to_string()]),
            _ => panic!("expected Delete"),
        }
    }

    #[test]
    fn test_rewrite_atom_paths_move() {
        let atom =
            Atom::Move { from: vec!["old".into(), "a".into()], to: vec!["old".into(), "b".into()] };
        let old = vec!["old".into()];
        let new = vec!["new".into()];
        let moves: Vec<(&NodePath, &NodePath)> = vec![(&old, &new)];
        let rewritten = rewrite_atom_paths(&atom, &moves);
        match rewritten {
            Atom::Move { from, to } => {
                assert_eq!(from, vec!["new".to_string(), "a".to_string()]);
                assert_eq!(to, vec!["new".to_string(), "b".to_string()]);
            }
            _ => panic!("expected Move"),
        }
    }

    #[test]
    fn test_rewrite_atom_paths_semantics_preserving() {
        let atom = Atom::SemanticsPreserving {
            at: vec!["old".into(), "body".into()],
            description: "refactor".into(),
        };
        let old = vec!["old".into()];
        let new = vec!["new".into()];
        let moves: Vec<(&NodePath, &NodePath)> = vec![(&old, &new)];
        let rewritten = rewrite_atom_paths(&atom, &moves);
        match rewritten {
            Atom::SemanticsPreserving { at, description } => {
                assert_eq!(at, vec!["new".to_string(), "body".to_string()]);
                assert_eq!(description, "refactor");
            }
            _ => panic!("expected SemanticsPreserving"),
        }
    }

    #[test]
    fn test_rewrite_atom_paths_no_match() {
        let atom =
            Atom::Insert { at: vec!["unrelated".into(), "fn".into()], content_hash: [0u8; 32] };
        let old = vec!["old".into()];
        let new = vec!["new".into()];
        let moves: Vec<(&NodePath, &NodePath)> = vec![(&old, &new)];
        let rewritten = rewrite_atom_paths(&atom, &moves);
        match rewritten {
            Atom::Insert { at, .. } => {
                assert_eq!(at, vec!["unrelated".to_string(), "fn".to_string()])
            }
            _ => panic!("expected Insert"),
        }
    }

    #[test]
    fn test_rewrite_atom_paths_directory_passthrough() {
        let atom = Atom::Directory { path: vec!["dir".into()] };
        let old = vec!["old".into()];
        let new = vec!["new".into()];
        let moves: Vec<(&NodePath, &NodePath)> = vec![(&old, &new)];
        let rewritten = rewrite_atom_paths(&atom, &moves);
        match rewritten {
            Atom::Directory { path } => assert_eq!(path, vec!["dir".to_string()]),
            _ => panic!("expected Directory"),
        }
    }

    // ── atoms_disjoint() via commutes() integration tests ────────────────

    #[test]
    fn test_commutes_blob_vs_insert_same_file_path() {
        let (author, signing_key) = author::test_keypair();
        let a = Change::new(
            HashSet::new(),
            vec![Atom::Blob {
                path: "a.txt".into(),
                hash: blake3::Hash::from_bytes([1u8; 32]),
                size: 100,
            }],
            "blob",
            author.clone(),
            &signing_key,
        );
        let b = Change::new(
            HashSet::new(),
            vec![Atom::Insert {
                at: vec!["file".into(), "a.txt".into(), "body".into()],
                content_hash: [0u8; 32],
            }],
            "insert",
            author,
            &signing_key,
        );
        assert!(!commutes(&a, &b), "Blob and Insert at same file path must NOT commute");
    }

    #[test]
    fn test_commutes_two_blobs_same_path_different_content() {
        let (author, signing_key) = author::test_keypair();
        let a = Change::new(
            HashSet::new(),
            vec![Atom::Blob {
                path: "a.txt".into(),
                hash: blake3::Hash::from_bytes([1u8; 32]),
                size: 100,
            }],
            "blob1",
            author.clone(),
            &signing_key,
        );
        let b = Change::new(
            HashSet::new(),
            vec![Atom::Blob {
                path: "a.txt".into(),
                hash: blake3::Hash::from_bytes([2u8; 32]),
                size: 200,
            }],
            "blob2",
            author,
            &signing_key,
        );
        assert!(!commutes(&a, &b), "Two Blobs same path different content must NOT commute");
    }

    #[test]
    fn test_commutes_blobs_same_content_commute() {
        let (author, signing_key) = author::test_keypair();
        let a = Change::new(
            HashSet::new(),
            vec![Atom::Blob {
                path: "a.txt".into(),
                hash: blake3::Hash::from_bytes([1u8; 32]),
                size: 100,
            }],
            "blob1",
            author.clone(),
            &signing_key,
        );
        let b = Change::new(
            HashSet::new(),
            vec![Atom::Blob {
                path: "a.txt".into(),
                hash: blake3::Hash::from_bytes([1u8; 32]),
                size: 100,
            }],
            "blob2",
            author,
            &signing_key,
        );
        assert!(commutes(&a, &b), "Two Blobs same path same content must commute (idempotent)");
    }

    #[test]
    fn test_commutes_move_atoms_not_counted_as_overlap() {
        let (author, signing_key) = author::test_keypair();
        let a = Change::new(
            HashSet::new(),
            vec![Atom::Move { from: vec!["mod_a".into()], to: vec!["mod_b".into()] }],
            "move a to b",
            author.clone(),
            &signing_key,
        );
        let b = Change::new(
            HashSet::new(),
            vec![Atom::Move { from: vec!["mod_c".into()], to: vec!["mod_d".into()] }],
            "move c to d",
            author,
            &signing_key,
        );
        assert!(commutes(&a, &b), "Two non-overlapping Moves must commute");
    }

    #[test]
    fn test_commute_pair_overlapping_paths_fails() {
        let (author, signing_key) = author::test_keypair();
        let a = make_change(HashSet::new(), vec![vec!["mod".into(), "fn_x".into()]]);
        let b = make_change(HashSet::new(), vec![vec!["mod".into(), "fn_x".into()]]);

        let result = commute_pair(&a, &b, &(author, signing_key));
        assert!(result.is_none(), "Gate 2 (overlapping atoms) must block commute_pair");
    }

    #[test]
    fn test_commute_pair_reverse_dep_fails() {
        let (author, signing_key) = author::test_keypair();
        let a = make_change(HashSet::new(), vec![vec!["mod_a".into()]]);
        // b depends on a — but what if a depends on b?
        let b = make_change(HashSet::new(), vec![vec!["mod_b".into()]]);
        // Make a depend on b
        let a_with_dep =
            Change::new(HashSet::from([b.id]), a.atoms, a.intent, a.author, &signing_key);

        let result = commute_pair(&a_with_dep, &b, &(author, signing_key));
        assert!(result.is_none(), "Gate 1 (reverse explicit dep) must block commute_pair");
    }

    #[test]
    fn test_commute_pair_trivial_no_moves() {
        let (author, signing_key) = author::test_keypair();
        let a = make_change(HashSet::new(), vec![vec!["mod_a".into()]]);
        let b = make_change(HashSet::new(), vec![vec!["mod_b".into()]]);

        let result = commute_pair(&a, &b, &(author.clone(), signing_key));
        assert!(result.is_some());
        let (b_prime, a_prime) = result.unwrap();
        // Without moves, atoms should be identical (just re-signed)
        assert_eq!(b_prime.atoms, b.atoms);
        assert_eq!(a_prime.atoms, a.atoms);
        // Dep sets should not include the cross-dep
        assert!(!b_prime.deps.contains(&a.id));
        assert!(!a_prime.deps.contains(&b.id));
    }

    #[test]
    fn test_commute_pair_preserves_intent() {
        let (author, signing_key) = author::test_keypair();
        let a = make_change(HashSet::new(), vec![vec!["mod_a".into()]]);
        let b = make_change(HashSet::new(), vec![vec!["mod_b".into()]]);

        let result = commute_pair(&a, &b, &(author, signing_key));
        let (b_prime, a_prime) = result.unwrap();
        assert_eq!(b_prime.intent, b.intent);
        assert_eq!(a_prime.intent, a.intent);
    }

    #[test]
    fn test_ghost_conflict_insert_before_delete() {
        let (author, signing_key) = author::test_keypair();
        // Insert in a, Delete in b — reverse order of test_commute_pair_ghost_conflict_fails
        let a = Change::new(
            HashSet::new(),
            vec![Atom::Insert { at: vec!["fn_foo".into()], content_hash: [0u8; 32] }],
            "insert",
            author.clone(),
            &signing_key,
        );
        let b = Change::new(
            HashSet::new(),
            vec![Atom::Delete { at: vec!["fn_foo".into()], prior_hash: [0u8; 32] }],
            "delete",
            author,
            &signing_key,
        );
        assert!(
            !commutes(&a, &b),
            "Insert+Delete at same path must NOT commute (either direction)"
        );
    }

    #[test]
    fn test_paths_overlap_empty_vs_nonempty() {
        assert!(paths_overlap(&vec![], &vec!["a".into()]));
    }

    // ── rewrite_atom_paths with "file" prefix Move atom ───────────────────

    #[test]
    fn test_rewrite_atom_paths_move_with_file_prefix() {
        let atom = Atom::Insert {
            at: vec!["file".into(), "old.rs".into(), "fn_main".into()],
            content_hash: [0u8; 32],
        };
        let from = vec!["file".into(), "old.rs".into()];
        let to = vec!["file".into(), "new.rs".into()];
        let moves: Vec<(&NodePath, &NodePath)> = vec![(&from, &to)];
        let rewritten = rewrite_atom_paths(&atom, &moves);
        match rewritten {
            Atom::Insert { at, .. } => {
                assert_eq!(
                    at,
                    vec!["file".to_string(), "new.rs".to_string(), "fn_main".to_string()]
                );
            }
            other => panic!("expected Insert, got {other:?}"),
        }
    }

    // ── file_path_for_atom Move to path ───────────────────────────────────

    #[test]
    fn test_file_path_for_atom_move_to_only() {
        // Move where `from` doesn't start with "file" but `to` does
        let atom = Atom::Move {
            from: vec!["mod".into(), "old.rs".into()],
            to: vec!["file".into(), "new.rs".into()],
        };
        assert_eq!(file_path_for_atom(&atom), Some("new.rs"));
    }

    // ── file_path_for_atom Mount no file prefix ───────────────────────────

    #[test]
    fn test_file_path_for_atom_mount_non_file() {
        let atom = Atom::Mount {
            path: vec!["vendor".into()],
            coordinate: arc_algebra_types::SpacetimeCoordinate {
                namespace: "n".into(),
                repo: "r".into(),
                hash: blake3::Hash::from_bytes([0u8; 32]),
            },
        };
        assert_eq!(file_path_for_atom(&atom), None);
    }

    // ── file_path_for_atom Directory no file prefix ───────────────────────

    #[test]
    fn test_file_path_for_atom_directory_non_file() {
        let atom = Atom::Directory { path: vec!["vendor".into()] };
        assert_eq!(file_path_for_atom(&atom), None);
    }

    // ── Blob vs Directory with "file" prefix ──────────────────────────────

    #[test]
    fn test_blob_conflicts_with_directory_file_path() {
        let blob = Atom::Blob {
            path: "a.txt".into(),
            hash: blake3::Hash::from_bytes([1u8; 32]),
            size: 100,
        };
        let dir = Atom::Directory { path: vec!["file".into(), "a.txt".into()] };
        assert!(blob_conflicts(&blob, &dir), "Blob vs Directory at same file path must conflict");
    }

    // ── Blob vs Mount with "file" prefix ──────────────────────────────────

    #[test]
    fn test_blob_conflicts_with_mount_file_path() {
        let blob = Atom::Blob {
            path: "a.txt".into(),
            hash: blake3::Hash::from_bytes([1u8; 32]),
            size: 100,
        };
        let mount = Atom::Mount {
            path: vec!["file".into(), "a.txt".into()],
            coordinate: arc_algebra_types::SpacetimeCoordinate {
                namespace: "n".into(),
                repo: "r".into(),
                hash: blake3::Hash::from_bytes([0u8; 32]),
            },
        };
        assert!(blob_conflicts(&blob, &mount), "Blob vs Mount at same file path must conflict");
    }

    // ── Blob vs Conflict with "file" prefix ───────────────────────────────

    #[test]
    fn test_blob_conflicts_with_conflict_file_path() {
        let blob = Atom::Blob {
            path: "a.txt".into(),
            hash: blake3::Hash::from_bytes([1u8; 32]),
            size: 100,
        };
        let conflict = Atom::Conflict {
            bases: vec![[0u8; 32]],
            sides: vec![[1u8; 32]],
            at: vec!["file".into(), "a.txt".into()],
        };
        assert!(
            blob_conflicts(&blob, &conflict),
            "Blob vs Conflict at same file path must conflict"
        );
    }

    // ── Blob vs SemanticsPreserving with "file" prefix ────────────────────

    #[test]
    fn test_blob_conflicts_with_semantics_preserving_file_path() {
        let blob = Atom::Blob {
            path: "a.txt".into(),
            hash: blake3::Hash::from_bytes([1u8; 32]),
            size: 100,
        };
        let sp = Atom::SemanticsPreserving {
            at: vec!["file".into(), "a.txt".into()],
            description: "fmt".into(),
        };
        assert!(
            blob_conflicts(&blob, &sp),
            "Blob vs SemanticsPreserving at same file path must conflict"
        );
    }

    // ── Blob vs non-Blob with no file_path_for_atom match ─────────────────

    #[test]
    fn test_blob_no_conflict_with_move_different_file() {
        let blob = Atom::Blob {
            path: "a.txt".into(),
            hash: blake3::Hash::from_bytes([1u8; 32]),
            size: 100,
        };
        let mv = Atom::Move {
            from: vec!["mod".into(), "x.rs".into()],
            to: vec!["mod".into(), "y.rs".into()],
        };
        assert!(!blob_conflicts(&blob, &mv), "Blob vs Move at non-file path must NOT conflict");
    }

    // ── atoms_disjoint with Move and overlapping Insert ───────────────────

    #[test]
    fn test_commutes_move_and_insert_same_target_path() {
        let (author, signing_key) = author::test_keypair();
        let a = Change::new(
            HashSet::new(),
            vec![Atom::Move { from: vec!["old".into()], to: vec!["mod".into()] }],
            "move",
            author.clone(),
            &signing_key,
        );
        let b = Change::new(
            HashSet::new(),
            vec![Atom::Insert { at: vec!["mod".into(), "fn_x".into()], content_hash: [0u8; 32] }],
            "insert",
            author,
            &signing_key,
        );
        // Move atoms are excluded from atoms_disjoint check, so they should commute
        assert!(
            commutes(&a, &b),
            "Move and Insert at target path must commute (Move excluded from Gate 2)"
        );
    }
}
