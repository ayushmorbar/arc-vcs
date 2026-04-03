use std::collections::HashMap;

use ignore::gitignore::Gitignore;

use crate::algebra::{Atom, Blake3Hash, NodePath};
use crate::store::author::Author;
use crate::store::cas::ObjectStore;
use crate::store::change::Change;

/// A materialized state is the result of replaying a sequence of changes
/// onto an empty tree. Each key is an AST node path; each value is the
/// serialized content at that path.
pub type MaterializedState = HashMap<NodePath, Vec<u8>>;

/// Maps each live [`NodePath`] to the [`Blake3Hash`] of the [`Change`] that
/// last wrote to it.  Populated incrementally by [`apply_change`] when a
/// `Some(&mut BlameState)` is supplied by the caller.
pub type BlameState = HashMap<NodePath, Blake3Hash>;

/// Apply a single [`Change`] to a materialized state by executing its atoms
/// in order.
///
/// # Replay Law
///
/// - `Insert { at, content_hash }` reads the blob from the CAS and writes it
///   to the state at `at`.
/// - `Delete { at, prior_hash }` removes the path from state (`prior_hash` is
///   not consulted during application — it is used for inversion).
/// - `Directory { path }` records a bare directory existence (empty value).
/// - `Move` and `SemanticsPreserving` are not yet implemented.
///
/// # AI Security Boundary
///
/// When the change's author is [`Author::AI`], every atom's filesystem path
/// is checked against `agent_ignore`. Matches cause an immediate
/// security-violation error. Additionally, no AI author may ever modify
/// `.agentignore` itself — this zero-trust policy is hardcoded and cannot
/// be overridden by the rules inside the file.
pub fn apply_change(
    state: &mut MaterializedState,
    change: &Change,
    store: &ObjectStore,
    agent_ignore: &Gitignore,
    mut blame: Option<&mut BlameState>,
) -> Result<(), String> {
    let is_ai = matches!(change.author, Author::AI { .. });

    for atom in &change.atoms {
        // ------------------------------------------------------------------
        // .agentignore enforcement — AI authors only.
        // ------------------------------------------------------------------
        if is_ai {
            for node_path in atom.paths() {
                let (checked_path, is_dir) = if node_path.len() >= 2 && node_path[0] == "file" {
                    (node_path[1].as_str(), false)
                } else if node_path.len() >= 2 && node_path[0] == "dir" {
                    (node_path[1].as_str(), true)
                } else {
                    continue;
                };
                // Zero-trust: AI can never self-modify the permission boundary.
                let is_sentinel = checked_path == ".agentignore";
                let is_restricted = agent_ignore
                    .matched_path_or_any_parents(checked_path, is_dir)
                    .is_ignore();
                if is_sentinel || is_restricted {
                    return Err(format!(
                        "Security Violation: AI is not permitted to modify \
                         '{checked_path}' via .agentignore"
                    ));
                }
            }
        }

        match atom {
            Atom::Insert { at, content_hash } => {
                let bytes = store
                    .read_blob(content_hash)
                    .map_err(|e| format!("CAS read error for Insert at {at:?}: {e}"))?;
                state.insert(at.clone(), bytes.to_vec());
                if let Some(ref mut b) = blame {
                    b.insert(at.clone(), change.id);
                }
            }
            Atom::Delete { at, .. } => {
                if state.remove(at).is_none() {
                    return Err(format!(
                        "causality violation: Delete targets non-existent path {at:?}"
                    ));
                }
                if let Some(ref mut b) = blame {
                    b.remove(at);
                }
            }
            Atom::Directory { path } => {
                // Record directory existence with an empty value so that
                // write_state_to_working_dir can recreate it.
                state.entry(path.clone()).or_default();
                if let Some(ref mut b) = blame {
                    b.insert(path.clone(), change.id);
                }
            }
            Atom::Move { .. } => {
                return Err("Move atoms are not yet supported".to_string());
            }
            Atom::SemanticsPreserving { .. } => {
                return Err("SemanticsPreserving atoms are not yet supported".to_string());
            }
            Atom::Blob { path, hash } => {
                // Store a magic reference token; write_state_to_working_dir
                // detects this prefix and reads raw bytes from .arc/blobs/.
                let mut token = b"ARC_BLOB_REF:".to_vec();
                token.extend_from_slice(hash);
                state.insert(path.clone(), token);
                if let Some(ref mut b) = blame {
                    b.insert(path.clone(), change.id);
                }
            }
            Atom::Mount { path, url, target } => {
                // Store a magic mount token; write_state_to_working_dir
                // detects this prefix and creates the directory placeholder.
                let token = format!("ARC_MOUNT:{url}|{target}").into_bytes();
                state.insert(path.clone(), token);
                if let Some(ref mut b) = blame {
                    b.insert(path.clone(), change.id);
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;
    use crate::store::cas::ObjectStore;

    /// Helper: create a temporary `ObjectStore` and write a blob, returning its hash.
    fn make_store_and_hash(content: &[u8]) -> (tempfile::TempDir, ObjectStore, Blake3Hash) {
        let dir = tempfile::tempdir().unwrap();
        let store = ObjectStore::new(dir.path());
        let hash = store.write_blob(content).unwrap();
        (dir, store, hash)
    }

    #[test]
    fn test_apply_change() {
        let (dir, store, _) = make_store_and_hash(b"placeholder");
        let body_hash = store.write_blob(b"let x = 1;").unwrap();
        let ret_hash = store.write_blob(b"x + 1").unwrap();
        let del_hash = store.write_blob(b"x + 1").unwrap(); // same content as ret
        let mut state = MaterializedState::new();
        let (author, signing_key) = crate::store::author::test_keypair();

        // Apply a change that inserts two paths.
        let insert_change = Change::new(
            HashSet::new(),
            vec![
                Atom::Insert {
                    at: vec!["fn_main".into(), "body".into()],
                    content_hash: body_hash,
                },
                Atom::Insert {
                    at: vec!["fn_main".into(), "ret".into()],
                    content_hash: ret_hash,
                },
            ],
            "test",
            author.clone(),
            &signing_key,
        );

        apply_change(
            &mut state,
            &insert_change,
            &store,
            &Gitignore::empty(),
            None,
        )
        .unwrap();
        assert_eq!(state.len(), 2);
        assert_eq!(
            state.get(&vec!["fn_main".into(), "body".into()]).unwrap(),
            b"let x = 1;"
        );

        // Apply a change that deletes one of the paths.
        let delete_change = Change::new(
            HashSet::from([insert_change.id]),
            vec![Atom::Delete {
                at: vec!["fn_main".into(), "ret".into()],
                prior_hash: del_hash,
            }],
            "test",
            author.clone(),
            &signing_key,
        );

        apply_change(
            &mut state,
            &delete_change,
            &store,
            &Gitignore::empty(),
            None,
        )
        .unwrap();
        assert_eq!(state.len(), 1);
        assert!(!state.contains_key(&vec!["fn_main".into(), "ret".into()]));
        drop(dir);
    }

    #[test]
    fn test_apply_delete_nonexistent_path_errors() {
        let (dir, store, prior_hash) = make_store_and_hash(b"ghost content");
        let mut state = MaterializedState::new();
        let (author, signing_key) = crate::store::author::test_keypair();

        let bad_delete = Change::new(
            HashSet::new(),
            vec![Atom::Delete {
                at: vec!["ghost".into()],
                prior_hash,
            }],
            "test",
            author,
            &signing_key,
        );

        let result = apply_change(&mut state, &bad_delete, &store, &Gitignore::empty(), None);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("causality violation"));
        drop(dir);
    }

    #[test]
    fn test_agentignore_security() {
        use ignore::gitignore::GitignoreBuilder;
        let mut builder = GitignoreBuilder::new("/");
        builder.add_line(None, "src/crypto.rs").unwrap();
        let agent_ignore = builder.build().unwrap();

        let (dir, store, _) = make_store_and_hash(b"malicious");
        let malicious_hash = store.write_blob(b"malicious").unwrap();
        let erase_hash = store.write_blob(b"remove all rules").unwrap();
        let mut state = MaterializedState::new();
        let (_, signing_key) = crate::store::author::test_keypair();
        let key = signing_key.verifying_key().to_bytes();
        let ai_author = crate::store::author::Author::AI {
            model: "gpt-99".to_string(),
            human_sponsor: key,
        };

        // Restricted path — AI must be blocked.
        let change = Change::new(
            std::collections::HashSet::new(),
            vec![Atom::Insert {
                at: vec!["file".into(), "src/crypto.rs".into(), "fn_evil".into()],
                content_hash: malicious_hash,
            }],
            "inject evil",
            ai_author.clone(),
            &signing_key,
        );
        let result = apply_change(&mut state, &change, &store, &agent_ignore, None);
        assert!(result.is_err());
        let msg = result.unwrap_err();
        assert!(
            msg.contains("Security Violation"),
            "expected security violation, got: {msg}"
        );
        assert!(
            msg.contains("src/crypto.rs"),
            "expected path in error, got: {msg}"
        );

        // Hardcoded sentinel: AI must never modify .agentignore itself,
        // even when the file is NOT listed in the rules.
        let sentinel_change = Change::new(
            std::collections::HashSet::new(),
            vec![Atom::Insert {
                at: vec!["file".into(), ".agentignore".into(), "content".into()],
                content_hash: erase_hash,
            }],
            "erase sandboxing",
            ai_author,
            &signing_key,
        );
        let sentinel_result = apply_change(
            &mut state,
            &sentinel_change,
            &store,
            &Gitignore::empty(),
            None,
        );
        assert!(sentinel_result.is_err());
        assert!(
            sentinel_result.unwrap_err().contains("Security Violation"),
            "sentinel rule must hold even with empty agent_ignore"
        );
        drop(dir);
    }
    /// `Directory` atoms must store an empty value at their path.
    #[test]
    fn test_apply_directory_atom() {
        let (dir, store, _) = make_store_and_hash(b"placeholder");
        let mut state = MaterializedState::new();
        let (author, signing_key) = crate::store::author::test_keypair();

        let change = Change::new(
            HashSet::new(),
            vec![Atom::Directory {
                path: vec!["dir".into(), "src/utils".into()],
            }],
            "create utils dir",
            author,
            &signing_key,
        );

        apply_change(&mut state, &change, &store, &Gitignore::empty(), None).unwrap();
        let key = vec!["dir".into(), "src/utils".into()];
        assert!(
            state.contains_key(&key),
            "Directory atom must insert an entry at its path"
        );
        assert_eq!(
            state[&key].as_slice(),
            b"",
            "Directory atom must store an empty value"
        );
        drop(dir);
    }

    /// `Blob` atoms must store an `ARC_BLOB_REF:` token with the embedded hash.
    #[test]
    fn test_apply_blob_atom() {
        let (dir, store, _) = make_store_and_hash(b"placeholder");
        let mut state = MaterializedState::new();
        let (author, signing_key) = crate::store::author::test_keypair();
        let hash = [0xab_u8; 32];

        let change = Change::new(
            HashSet::new(),
            vec![Atom::Blob {
                path: vec!["file".into(), "logo.png".into()],
                hash,
            }],
            "add binary asset",
            author,
            &signing_key,
        );

        apply_change(&mut state, &change, &store, &Gitignore::empty(), None).unwrap();
        let key = vec!["file".into(), "logo.png".into()];
        let val = state.get(&key).expect("Blob atom must insert an entry");
        assert!(
            val.starts_with(b"ARC_BLOB_REF:"),
            "Blob atom must write ARC_BLOB_REF: prefix, got: {val:?}"
        );
        assert_eq!(
            &val[13..],
            &hash,
            "Blob atom must embed the 32-byte hash after the prefix"
        );
        drop(dir);
    }

    /// `Move` atoms must return an error (unimplemented).
    #[test]
    fn test_apply_move_atom_returns_error() {
        let (dir, store, _) = make_store_and_hash(b"placeholder");
        let mut state = MaterializedState::new();
        let (author, signing_key) = crate::store::author::test_keypair();

        let change = Change::new(
            HashSet::new(),
            vec![Atom::Move {
                from: vec!["fn_old".into()],
                to: vec!["fn_new".into()],
            }],
            "rename fn",
            author,
            &signing_key,
        );

        let result = apply_change(&mut state, &change, &store, &Gitignore::empty(), None);
        assert!(
            result.is_err(),
            "Move atom must return an error (not yet implemented)"
        );
        assert!(
            result.unwrap_err().contains("Move"),
            "error message must mention Move"
        );
        drop(dir);
    }

    /// Blame state must be populated for every inserted path.
    #[test]
    fn test_blame_state_population() {
        let (dir, store, _) = make_store_and_hash(b"placeholder");
        let fn_a_hash = store.write_blob(b"fn a() {}").unwrap();
        let fn_b_hash = store.write_blob(b"fn b() {}").unwrap();
        let fn_a_prior = store.write_blob(b"fn a() {}").unwrap();
        let mut state = MaterializedState::new();
        let mut blame = BlameState::new();
        let (author, signing_key) = crate::store::author::test_keypair();

        let change = Change::new(
            HashSet::new(),
            vec![
                Atom::Insert {
                    at: vec!["fn_a".into()],
                    content_hash: fn_a_hash,
                },
                Atom::Insert {
                    at: vec!["fn_b".into()],
                    content_hash: fn_b_hash,
                },
            ],
            "add a and b",
            author,
            &signing_key,
        );

        apply_change(
            &mut state,
            &change,
            &store,
            &Gitignore::empty(),
            Some(&mut blame),
        )
        .unwrap();

        assert_eq!(
            blame.get(&vec!["fn_a".into()]),
            Some(&change.id),
            "blame must attribute fn_a to its change"
        );
        assert_eq!(
            blame.get(&vec!["fn_b".into()]),
            Some(&change.id),
            "blame must attribute fn_b to its change"
        );

        // Delete fn_a — blame entry must be removed.
        let (author2, signing_key2) = crate::store::author::test_keypair();
        let del = Change::new(
            HashSet::from([change.id]),
            vec![Atom::Delete {
                at: vec!["fn_a".into()],
                prior_hash: fn_a_prior,
            }],
            "remove a",
            author2,
            &signing_key2,
        );
        apply_change(
            &mut state,
            &del,
            &store,
            &Gitignore::empty(),
            Some(&mut blame),
        )
        .unwrap();
        assert!(
            !blame.contains_key(&vec!["fn_a".into()]),
            "blame must remove fn_a after Delete"
        );
        drop(dir);
    }
}
