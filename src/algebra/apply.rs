use std::collections::HashMap;

use ignore::gitignore::Gitignore;

use crate::algebra::{Atom, NodePath};
use crate::store::author::Author;
use crate::store::change::Change;

/// A materialized state is the result of replaying a sequence of changes
/// onto an empty tree. Each key is an AST node path; each value is the
/// serialized content at that path.
pub type MaterializedState = HashMap<NodePath, Vec<u8>>;

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
            }
            Atom::Delete { at } => {
                if state.remove(at).is_none() {
                    return Err(format!(
                        "causality violation: Delete targets non-existent path {at:?}"
                    ));
                }
            }
            Atom::Directory { path } => {
                // Record directory existence with an empty value so that
                // write_state_to_working_dir can recreate it.
                state.entry(path.clone()).or_default();
            }
            Atom::Move { .. } => {
                return Err("Move atoms are not yet supported".to_string());
            }
            Atom::SemanticsPreserving { .. } => {
                return Err("SemanticsPreserving atoms are not yet supported".to_string());
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

        apply_change(&mut state, &insert_change, &Gitignore::empty()).unwrap();
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

        apply_change(&mut state, &delete_change, &Gitignore::empty()).unwrap();
        assert_eq!(state.len(), 1);
        assert!(state
            .get(&vec!["fn_main".into(), "ret".into()])
            .is_none());
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

        let result = apply_change(&mut state, &bad_delete, &Gitignore::empty());
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
        let result = apply_change(&mut state, &change, &agent_ignore);
        assert!(result.is_err());
        let msg = result.unwrap_err();
        assert!(msg.contains("Security Violation"), "expected security violation, got: {msg}");
        assert!(msg.contains("src/crypto.rs"), "expected path in error, got: {msg}");

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
        let sentinel_result = apply_change(&mut state, &sentinel_change, &Gitignore::empty());
        assert!(sentinel_result.is_err());
        assert!(
            sentinel_result.unwrap_err().contains("Security Violation"),
            "sentinel rule must hold even with empty agent_ignore"
        );
    }
}
