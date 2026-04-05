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
use arc_store_types::author::Author;

use crate::BlobStore;

/// Inversion failed.
#[derive(Debug)]
pub enum InvertError {
    /// The blob required to reconstruct the inverse is missing from the CAS.
    CasMissing(Blake3Hash),
    /// This atom variant has no defined inverse (e.g. `SemanticsPreserving`).
    Unsupported,
}

impl std::fmt::Display for InvertError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            InvertError::CasMissing(hash) => {
                write!(
                    f,
                    "blob {hash:?} is missing from the store boundary - cannot invert"
                )
            }
            InvertError::Unsupported => write!(f, "atom type does not support inversion"),
        }
    }
}

impl std::error::Error for InvertError {}

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
            Ok(Atom::Delete {
                at: at.clone(),
                prior_hash: *content_hash,
            })
        }
        Atom::Delete { at, prior_hash } => {
            if !store.contains_blob(prior_hash) {
                return Err(InvertError::CasMissing(*prior_hash));
            }
            Ok(Atom::Insert {
                at: at.clone(),
                content_hash: *prior_hash,
            })
        }
        Atom::Move { from, to } => Ok(Atom::Move {
            from: to.clone(),
            to: from.clone(),
        }),
        _ => Err(InvertError::Unsupported),
    }
}

/// Produce a new [`Change`] that is the semantic inverse of `change`.
///
/// The resulting change:
/// - Contains each atom inverted in **reverse** order.
/// - Depends on `change.id` (ensuring causal ordering: inversion is always
///   applied *after* the original).
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
    Ok(Change::new(
        HashSet::from([change.id]),
        inverted_atoms,
        intent,
        author.clone(),
        signing_key,
    ))
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use arc_store_cas::cas::ObjectStore;
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

        let atom = Atom::Insert {
            at: vec!["fn_main".to_string()],
            content_hash,
        };

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

        let atom = Atom::Delete {
            at: vec!["fn_a".to_string(), "body".to_string()],
            prior_hash,
        };

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

        let original = Atom::Insert {
            at: vec!["node".to_string()],
            content_hash,
        };

        let inv = invert_atom(&original, &store).unwrap();
        let inv_inv = invert_atom(&inv, &store).unwrap();
        assert_eq!(
            original, inv_inv,
            "double inversion must produce the original atom"
        );
    }

    #[test]
    fn test_invert_missing_blob_returns_cas_missing() {
        let (_dir, store) = make_store();
        // Use a hash for a blob that was never written.
        let ghost_hash = [0xde_u8; 32];

        let atom = Atom::Insert {
            at: vec!["fn_foo".to_string()],
            content_hash: ghost_hash,
        };

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
            vec![Atom::Insert {
                at: vec!["fn_x".to_string()],
                content_hash: hash,
            }],
            "add fn_x",
            author.clone(),
            &signing_key,
        );

        let inverted = invert_change(&original, &store, &(author, signing_key)).unwrap();

        assert!(
            inverted.deps.contains(&original.id),
            "inverted change must depend on the original"
        );
        assert!(
            inverted.intent.contains("Revert"),
            "inverted intent must contain 'Revert'"
        );
        assert_eq!(inverted.atoms.len(), original.atoms.len());
    }
}
