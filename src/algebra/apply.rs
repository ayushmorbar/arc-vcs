use std::collections::HashMap;

use crate::algebra::{Atom, NodePath};
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
/// - `Move` and `SemanticsPreserving` are not yet implemented and will
///   return an error.
pub fn apply_change(state: &mut MaterializedState, change: &Change) -> Result<(), String> {
    for atom in &change.atoms {
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
        );

        apply_change(&mut state, &insert_change).unwrap();
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
        );

        apply_change(&mut state, &delete_change).unwrap();
        assert_eq!(state.len(), 1);
        assert!(state
            .get(&vec!["fn_main".into(), "ret".into()])
            .is_none());
    }

    #[test]
    fn test_apply_delete_nonexistent_path_errors() {
        let mut state = MaterializedState::new();

        let bad_delete = Change::new(
            HashSet::new(),
            vec![Atom::Delete {
                at: vec!["ghost".into()],
            }],
        );

        let result = apply_change(&mut state, &bad_delete);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("causality violation"));
    }
}
