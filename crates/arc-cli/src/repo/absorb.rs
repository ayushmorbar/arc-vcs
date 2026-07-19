use std::collections::HashSet;

use arc_algebra::commute::commutes;
use arc_algebra_types::Blake3Hash;
use arc_change::Change;
use arc_diagnostics::ResultExt;
use arc_store_view::View;

use super::core::*;
use crate::store_compat::ObjectStoreChangeExt;

#[derive(Debug, Clone)]
pub struct AbsorbAstResult {
    pub selected_target: Blake3Hash,
    pub new_head: Option<Blake3Hash>,
    pub absorbed_atoms: usize,
}

impl Repository {
    /// Absorb working-directory AST edits into history using a conservative
    /// commutativity selector.
    ///
    /// This initial scaffold computes the AST delta and picks the deepest
    /// first-parent ancestor where the working delta commutes with all newer
    /// changes on that linear spine. For safety, rewrite application currently
    /// supports only `HEAD` targets; older targets report a clear next-step
    /// message while preserving the working tree.
    pub fn absorb_ast(&mut self) -> anyhow::Result<AbsorbAstResult> {
        let view_name = self.current_view_name()?;
        self.hydrate(&view_name)?;
        let view = View::load(&self.shared_root, &view_name)
            .map_err(|e| anyhow::anyhow!("failed to load view '{view_name}': {e}"))?;

        if view.heads.len() != 1 {
            anyhow::bail!(
                "absorb requires exactly one head; current view '{}' has {} heads",
                view_name,
                view.heads.len()
            );
        }

        let head = *view.heads.iter().next().unwrap();
        let state = self.materialize(&view_name)?;
        let working_delta = self.compute_working_directory_delta(&state)?;
        if working_delta.is_empty() {
            return Ok(AbsorbAstResult {
                selected_target: head,
                new_head: None,
                absorbed_atoms: 0,
            });
        }

        let (author, signing_key) = self.signing_identity()?;
        let synthetic_working = Change::new(
            HashSet::new(),
            working_delta.clone(),
            "absorb(ast): working delta",
            author.clone(),
            signing_key,
        );

        let spine_ids = self.first_parent_spine(head)?;
        let mut spine_changes = Vec::with_capacity(spine_ids.len());
        for change_id in &spine_ids {
            let change = self
                .store
                .read_change(change_id)
                .map_err(|_| anyhow::anyhow!("change {} not found in CAS", _hex(change_id)))?;
            spine_changes.push(change);
        }

        let selected_idx = deepest_commuting_index(&synthetic_working, &spine_changes);
        let selected_target = spine_ids[selected_idx];

        if selected_target != head {
            return Err(anyhow::anyhow!(
                "absorb selected non-HEAD target {} (HEAD {})",
                &_hex(&selected_target)[..12],
                &_hex(&head)[..12]
            ))
            .with_hint_command(
                "Absorb currently requires the target to be HEAD. Try restacking this commit to the top of your stack first.",
                "arc restack",
            );
        }

        let new_head = self.amend(None)?;
        Ok(AbsorbAstResult {
            selected_target,
            new_head: Some(new_head),
            absorbed_atoms: working_delta.len(),
        })
    }

    fn first_parent_spine(&self, start: Blake3Hash) -> anyhow::Result<Vec<Blake3Hash>> {
        let mut out = vec![start];
        let mut cursor = start;

        loop {
            let change = self
                .store
                .read_change(&cursor)
                .map_err(|_| anyhow::anyhow!("change {} not found in CAS", _hex(&cursor)))?;

            if change.deps.is_empty() {
                break;
            }

            if change.deps.len() > 1 {
                anyhow::bail!(
                    "absorb(ast) scaffold currently supports linear history only; encountered merge change {}",
                    &_hex(&cursor)[..12]
                );
            }

            let mut deps: Vec<Blake3Hash> = change.deps.into_iter().collect();
            deps.sort_by_key(_hex);
            cursor = deps[0];
            out.push(cursor);
        }

        Ok(out)
    }
}

fn deepest_commuting_index(synthetic_working: &Change, spine: &[Change]) -> usize {
    let mut selected = 0usize;

    for idx in 0..spine.len() {
        if !spine[..idx].iter().all(|newer| commutes(synthetic_working, newer)) {
            continue;
        }
        selected = idx;
    }

    selected
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use arc_algebra_types::Atom;
    use arc_change::Change;
    use arc_store_types::author::test_keypair;

    use super::deepest_commuting_index;

    fn mk_change(path: &[&str]) -> Change {
        let (author, key) = test_keypair();
        Change::new(
            HashSet::new(),
            vec![Atom::SemanticsPreserving {
                at: path.iter().map(|s| s.to_string()).collect(),
                description: "x".to_string(),
            }],
            "t",
            author,
            &key,
        )
    }

    #[test]
    fn selects_deeper_target_when_working_delta_commutes_with_newer_changes() {
        let synthetic = mk_change(&["file", "src/lib.rs", "fn", "new_fn"]);
        let newer = mk_change(&["file", "src/lib.rs", "fn", "other_fn"]);
        let older = mk_change(&["file", "src/lib.rs", "fn", "base_fn"]);
        let spine = vec![newer, older];

        let idx = deepest_commuting_index(&synthetic, &spine);
        assert_eq!(idx, 1);
    }

    #[test]
    fn keeps_head_target_when_working_delta_conflicts_with_head() {
        let synthetic = mk_change(&["file", "src/lib.rs", "fn", "same_fn"]);
        let head = mk_change(&["file", "src/lib.rs", "fn", "same_fn"]);
        let older = mk_change(&["file", "src/lib.rs", "fn", "base_fn"]);
        let spine = vec![head, older];

        let idx = deepest_commuting_index(&synthetic, &spine);
        assert_eq!(idx, 0);
    }
}
