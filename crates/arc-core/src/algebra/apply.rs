use std::collections::HashMap;

use ignore::gitignore::Gitignore;

use crate::algebra::{Atom, Blake3Hash, NodePath};
use crate::store::author::Author;
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
/// - `Insert { at, content }` adds the path/content pair to the state.
/// - `Delete { at }` removes it (returns an error if the path is absent,
///   since that violates causal ordering).
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
            Atom::Insert { at, content } => {
                state.insert(at.clone(), content.clone());
                if let Some(ref mut b) = blame {
                    b.insert(at.clone(), change.id);
                }
            }
            Atom::Delete { at } => {
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

    #[test]
    fn test_apply_change() {
        let mut state = MaterializedState::new();
        let (author, signing_key) = crate::store::author::test_keypair();

        // Apply a change that inserts two paths.
        let insert_change = Change::new(
            HashSet::new(),
            vec![
                Atom::Insert {
                    at: vec!["fn_main".into(), "body".into()],
                    content: b"let x = 1;".to_vec(),
                },
                Atom::Insert {
                    at: vec!["fn_main".into(), "ret".into()],
                    content: b"x + 1".to_vec(),
                },
            ],
            "test",
            author.clone(),
            &signing_key,
        );

        apply_change(&mut state, &insert_change, &Gitignore::empty(), None).unwrap();
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
            }],
            "test",
            author.clone(),
            &signing_key,
        );

        apply_change(&mut state, &delete_change, &Gitignore::empty(), None).unwrap();
        assert_eq!(state.len(), 1);
        assert!(!state.contains_key(&vec!["fn_main".into(), "ret".into()]));
    }

    #[test]
    fn test_apply_delete_nonexistent_path_errors() {
        let mut state = MaterializedState::new();
        let (author, signing_key) = crate::store::author::test_keypair();

        let bad_delete = Change::new(
            HashSet::new(),
            vec![Atom::Delete {
                at: vec!["ghost".into()],
            }],
            "test",
            author,
            &signing_key,
        );

        let result = apply_change(&mut state, &bad_delete, &Gitignore::empty(), None);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("causality violation"));
    }

    #[test]
    fn test_agentignore_security() {
        use ignore::gitignore::GitignoreBuilder;
        let mut builder = GitignoreBuilder::new("/");
        builder.add_line(None, "src/crypto.rs").unwrap();
        let agent_ignore = builder.build().unwrap();

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
                content: b"malicious".to_vec(),
            }],
            "inject evil",
            ai_author.clone(),
            &signing_key,
        );
        let result = apply_change(&mut state, &change, &agent_ignore, None);
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
                content: b"remove all rules".to_vec(),
            }],
            "erase sandboxing",
            ai_author,
            &signing_key,
        );
        let sentinel_result = apply_change(&mut state, &sentinel_change, &Gitignore::empty(), None);
        assert!(sentinel_result.is_err());
        assert!(
            sentinel_result.unwrap_err().contains("Security Violation"),
            "sentinel rule must hold even with empty agent_ignore"
        );
    }
    /// `Directory` atoms must store an empty value at their path.
    #[test]
    fn test_apply_directory_atom() {
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

        apply_change(&mut state, &change, &Gitignore::empty(), None).unwrap();
        let key = vec!["dir".into(), "src/utils".into()];
        assert!(state.contains_key(&key), "Directory atom must insert an entry at its path");
        assert_eq!(
            state[&key].as_slice(),
            b"",
            "Directory atom must store an empty value"
        );
    }

    /// `Blob` atoms must store an `ARC_BLOB_REF:` token with the embedded hash.
    #[test]
    fn test_apply_blob_atom() {
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

        apply_change(&mut state, &change, &Gitignore::empty(), None).unwrap();
        let key = vec!["file".into(), "logo.png".into()];
        let val = state.get(&key).expect("Blob atom must insert an entry");
        assert!(
            val.starts_with(b"ARC_BLOB_REF:"),
            "Blob atom must write ARC_BLOB_REF: prefix, got: {val:?}"
        );
        assert_eq!(&val[13..], &hash, "Blob atom must embed the 32-byte hash after the prefix");
    }

    /// `Move` atoms must return an error (unimplemented).
    #[test]
    fn test_apply_move_atom_returns_error() {
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

        let result = apply_change(&mut state, &change, &Gitignore::empty(), None);
        assert!(result.is_err(), "Move atom must return an error (not yet implemented)");
        assert!(
            result.unwrap_err().contains("Move"),
            "error message must mention Move"
        );
    }

    /// Blame state must be populated for every inserted path.
    #[test]
    fn test_blame_state_population() {
        let mut state = MaterializedState::new();
        let mut blame = BlameState::new();
        let (author, signing_key) = crate::store::author::test_keypair();

        let change = Change::new(
            HashSet::new(),
            vec![
                Atom::Insert {
                    at: vec!["fn_a".into()],
                    content: b"fn a() {}".to_vec(),
                },
                Atom::Insert {
                    at: vec!["fn_b".into()],
                    content: b"fn b() {}".to_vec(),
                },
            ],
            "add a and b",
            author,
            &signing_key,
        );

        apply_change(&mut state, &change, &Gitignore::empty(), Some(&mut blame)).unwrap();

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
            vec![Atom::Delete { at: vec!["fn_a".into()] }],
            "remove a",
            author2,
            &signing_key2,
        );
        apply_change(&mut state, &del, &Gitignore::empty(), Some(&mut blame)).unwrap();
        assert!(
            blame.get(&vec!["fn_a".into()]).is_none(),
            "blame must remove fn_a after Delete"
        );
    }}
