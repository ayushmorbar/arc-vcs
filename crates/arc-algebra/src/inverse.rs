//! Inversion algebra: compute the semantic inverse of a [`Change`].
//!
//! Every `Insert` and `Delete` atom carries enough information to produce its
//! semantic inverse:
//!
//! - `Insert { at, content_hash }` → `Delete { at, prior_hash: content_hash }`
//! - `Delete { at, prior_hash }` → `Insert { at, content_hash: prior_hash }`
//! - `Move { from, to }` → `Move { from: to, to: from }`
//! - All other variants → [`InvertError::Unsupported`]
//!
//! The inverted [`Change`] depends on the original (causal ordering) and is
//! signed with the rebaser's identity so the graph remains cryptographically
//! consistent.

use std::collections::HashSet;

use arc_algebra_types::{Atom, Blake3Hash};
use arc_change::Change;
use arc_store_types::Author;
use thiserror::Error;

use crate::BlobStore;

/// Inversion failed.
#[derive(Debug, Error)]
pub enum InvertError {
    /// The blob required to reconstruct the inverse is missing from the CAS.
    #[error("blob {0:?} is missing from the store boundary - cannot invert")]
    CasMissing(Blake3Hash),
    /// This atom variant has no defined inverse (e.g. `SemanticsPreserving`).
    #[error("atom type does not support inversion")]
    Unsupported,
}

/// Compute the semantic inverse of a single [`Atom`].
///
/// Verifies that the referenced blob exists in `store` before returning.
/// Does **not** write any new objects to the store.
pub fn invert_atom(atom: &Atom, store: &impl BlobStore) -> Result<Atom, InvertError> {
    match atom {
        Atom::Insert { at, content_hash } => {
            if !store.contains_blob(content_hash) {
                return Err(InvertError::CasMissing(*content_hash));
            }
            Ok(Atom::Delete { at: at.clone(), prior_hash: *content_hash })
        }
        Atom::Delete { at, prior_hash } => {
            if !store.contains_blob(prior_hash) {
                return Err(InvertError::CasMissing(*prior_hash));
            }
            Ok(Atom::Insert { at: at.clone(), content_hash: *prior_hash })
        }
        Atom::Move { from, to } => Ok(Atom::Move { from: to.clone(), to: from.clone() }),
        _ => Err(InvertError::Unsupported),
    }
}

/// Produce a new [`Change`] that is the semantic inverse of `change`.
///
/// The resulting change:
/// - Contains each atom inverted in **reverse** order.
/// - Depends on `change.id` (ensuring causal ordering: inversion is always applied *after* the
///   original).
/// - Is signed with `(author, signing_key)` (the rebaser's identity).
/// - Sets `intent` to `"Revert: {original_intent}"`.
pub fn invert_change(
    change: &Change,
    store: &impl BlobStore,
    signer: &(Author, ed25519_dalek::SigningKey),
) -> Result<Change, InvertError> {
    let mut inverted_atoms = Vec::with_capacity(change.atoms.len());
    for atom in change.atoms.iter().rev() {
        inverted_atoms.push(invert_atom(atom, store)?);
    }

    let (author, signing_key) = signer;
    let intent = format!("Revert: {}", change.intent);
    Ok(Change::new(HashSet::from([change.id]), inverted_atoms, intent, author.clone(), signing_key))
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use arc_store_cas::ObjectStore;
    use arc_store_types::author;

    use super::*;

    fn make_store() -> (tempfile::TempDir, ObjectStore) {
        let dir = tempfile::tempdir().unwrap();
        let store = ObjectStore::new(dir.path());
        (dir, store)
    }

    #[test]
    fn test_invert_insert_produces_delete() {
        let (_dir, store) = make_store();
        let content = b"fn main() {}";
        let content_hash = store.write_blob(content).unwrap();

        let atom = Atom::Insert { at: vec!["fn_main".to_string()], content_hash };

        let inv = invert_atom(&atom, &store).unwrap();
        match inv {
            Atom::Delete { at, prior_hash } => {
                assert_eq!(at, vec!["fn_main".to_string()]);
                assert_eq!(prior_hash, content_hash);
            }
            other => panic!("expected Delete, got {other:?}"),
        }
    }

    #[test]
    fn test_invert_delete_produces_insert() {
        let (_dir, store) = make_store();
        let content = b"let x = 42;";
        let prior_hash = store.write_blob(content).unwrap();

        let atom = Atom::Delete { at: vec!["fn_a".to_string(), "body".to_string()], prior_hash };

        let inv = invert_atom(&atom, &store).unwrap();
        match inv {
            Atom::Insert { at, content_hash } => {
                assert_eq!(at, vec!["fn_a".to_string(), "body".to_string()]);
                assert_eq!(content_hash, prior_hash);
            }
            other => panic!("expected Insert, got {other:?}"),
        }
    }

    #[test]
    fn test_invert_insert_delete_roundtrip() {
        let (_dir, store) = make_store();
        let content = b"hello world";
        let content_hash = store.write_blob(content).unwrap();

        let original = Atom::Insert { at: vec!["node".to_string()], content_hash };

        let inv = invert_atom(&original, &store).unwrap();
        let inv_inv = invert_atom(&inv, &store).unwrap();
        assert_eq!(original, inv_inv, "double inversion must produce the original atom");
    }

    #[test]
    fn test_invert_missing_blob_returns_cas_missing() {
        let (_dir, store) = make_store();
        // Use a hash for a blob that was never written.
        let ghost_hash = [0xDE_u8; 32];

        let atom = Atom::Insert { at: vec!["fn_foo".to_string()], content_hash: ghost_hash };

        let result = invert_atom(&atom, &store);
        assert!(
            matches!(result, Err(InvertError::CasMissing(_))),
            "missing blob must yield CasMissing, got: {result:?}"
        );
    }

    #[test]
    fn test_invert_change_depends_on_original() {
        let (_dir, store) = make_store();
        let hash = store.write_blob(b"content").unwrap();
        let (author, signing_key) = author::test_keypair();

        let original = Change::new(
            HashSet::new(),
            vec![Atom::Insert { at: vec!["fn_x".to_string()], content_hash: hash }],
            "add fn_x",
            author.clone(),
            &signing_key,
        );

        let inverted = invert_change(&original, &store, &(author, signing_key)).unwrap();

        assert!(
            inverted.deps.contains(&original.id),
            "inverted change must depend on the original"
        );
        assert!(inverted.intent.contains("Revert"), "inverted intent must contain 'Revert'");
        assert_eq!(inverted.atoms.len(), original.atoms.len());
    }

    // ── Move atom inversion ───────────────────────────────────────────────

    #[test]
    fn test_invert_atom_move() {
        let (_dir, store) = make_store();
        let atom = Atom::Move {
            from: vec!["old".into(), "a.rs".into()],
            to: vec!["new".into(), "a.rs".into()],
        };
        let inv = invert_atom(&atom, &store).unwrap();
        match inv {
            Atom::Move { from, to } => {
                assert_eq!(from, vec!["new".to_string(), "a.rs".to_string()]);
                assert_eq!(to, vec!["old".to_string(), "a.rs".to_string()]);
            }
            other => panic!("expected Move, got {other:?}"),
        }
    }

    // ── Delete with missing blob returns CasMissing ───────────────────────

    #[test]
    fn test_invert_delete_cas_missing() {
        let (_dir, store) = make_store();
        let ghost_hash = [0xBE_u8; 32];
        let atom = Atom::Delete { at: vec!["fn_x".into()], prior_hash: ghost_hash };
        let result = invert_atom(&atom, &store);
        assert!(
            matches!(result, Err(InvertError::CasMissing(h)) if h == ghost_hash),
            "missing blob on Delete must yield CasMissing, got: {result:?}"
        );
    }

    // ── Unsupported atom variants ─────────────────────────────────────────

    #[test]
    fn test_invert_atom_unsupported_directory() {
        let (_dir, store) = make_store();
        let atom = Atom::Directory { path: vec!["dir".into()] };
        let result = invert_atom(&atom, &store);
        assert!(matches!(result, Err(InvertError::Unsupported)));
    }

    #[test]
    fn test_invert_atom_unsupported_blob() {
        let (_dir, store) = make_store();
        let atom = Atom::Blob {
            path: "x.bin".into(),
            hash: blake3::Hash::from_bytes([0u8; 32]),
            size: 100,
        };
        let result = invert_atom(&atom, &store);
        assert!(matches!(result, Err(InvertError::Unsupported)));
    }

    #[test]
    fn test_invert_atom_unsupported_conflict() {
        let (_dir, store) = make_store();
        let atom = Atom::Conflict {
            bases: vec![[0u8; 32]],
            sides: vec![[1u8; 32]],
            at: vec!["file".into()],
        };
        let result = invert_atom(&atom, &store);
        assert!(matches!(result, Err(InvertError::Unsupported)));
    }

    #[test]
    fn test_invert_atom_unsupported_mount() {
        let (_dir, store) = make_store();
        let atom = Atom::Mount {
            path: vec!["lib".into()],
            coordinate: arc_algebra_types::SpacetimeCoordinate {
                namespace: "n".into(),
                repo: "r".into(),
                hash: blake3::Hash::from_bytes([0u8; 32]),
            },
        };
        let result = invert_atom(&atom, &store);
        assert!(matches!(result, Err(InvertError::Unsupported)));
    }

    #[test]
    fn test_invert_atom_unsupported_semantics_preserving() {
        let (_dir, store) = make_store();
        let atom = Atom::SemanticsPreserving { at: vec!["f".into()], description: "fmt".into() };
        let result = invert_atom(&atom, &store);
        assert!(matches!(result, Err(InvertError::Unsupported)));
    }

    // ── invert_change with multiple atoms reversed ────────────────────────

    #[test]
    fn test_invert_change_reverses_atom_order() {
        let (_dir, store) = make_store();
        let h1 = store.write_blob(b"a").unwrap();
        let h2 = store.write_blob(b"b").unwrap();
        let (author, signing_key) = author::test_keypair();

        let original = Change::new(
            HashSet::new(),
            vec![
                Atom::Insert { at: vec!["first".into()], content_hash: h1 },
                Atom::Insert { at: vec!["second".into()], content_hash: h2 },
            ],
            "add two",
            author.clone(),
            &signing_key,
        );

        let inverted = invert_change(&original, &store, &(author, signing_key)).unwrap();
        assert_eq!(inverted.atoms.len(), 2);
        // Atoms are reversed: original [Insert(first), Insert(second)] → inverted [Delete(second),
        // Delete(first)]
        match &inverted.atoms[0] {
            Atom::Delete { at, .. } => assert_eq!(at, &vec!["second".to_string()]),
            other => panic!("expected Delete for second, got {other:?}"),
        }
        match &inverted.atoms[1] {
            Atom::Delete { at, .. } => assert_eq!(at, &vec!["first".to_string()]),
            other => panic!("expected Delete for first, got {other:?}"),
        }
    }

    // ── CasMissing error message ──────────────────────────────────────────

    #[test]
    fn cas_missing_error_display() {
        let hash = [0xAB_u8; 32];
        let err = InvertError::CasMissing(hash);
        let msg = format!("{err}");
        assert!(msg.contains("missing"));
        assert!(msg.contains("cannot invert"));
    }
}
