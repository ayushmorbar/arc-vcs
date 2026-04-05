use std::collections::HashSet;
use std::fs;

use arc_algebra_types::{Atom, Blake3Hash, NodePath};
use arc_change::Change;
use arc_store_view::View;

use super::core::*;
use crate::store_compat::ObjectStoreChangeExt;

impl Repository {
    /// Merge `target_name` view into the current view using the algebraic
    /// merge law.
    ///
    /// If all exclusive changes commute, the merge is a simple head-union
    /// with no merge commit. If any pair conflicts, aborts with an error.
    pub fn merge_view(&mut self, target_name: &str) -> anyhow::Result<()> {
        self.hydrate(target_name)?;
        let target_view = View::load(&self.shared_root, target_name)
            .map_err(|e| anyhow::anyhow!("failed to load view '{target_name}': {e}"))?;
        self.merge_heads(&target_view.heads)
    }

    /// Merge an arbitrary set of heads into the current view.
    ///
    /// This is the head-based primitive underlying `merge_view`. It performs
    /// a dirty-check on the working directory before mutating any state,
    /// then runs the algebraic commutativity check on the exclusive deltas.
    pub fn merge_heads(&mut self, target_heads: &HashSet<Blake3Hash>) -> anyhow::Result<()> {
        self.acquire_lock()?;
        let current_name = self.current_view_name()?;
        tracing::info!(view = %current_name, "merge_heads started");

        // Hydrate both sides (idempotent - already-present nodes are skipped).
        self.hydrate(&current_name)?;
        self.hydrate_heads(target_heads)?;

        let current_view = View::load(&self.shared_root, &current_name)
            .map_err(|e| anyhow::anyhow!("failed to load view '{current_name}': {e}"))?;

        // --- Dirty working-directory check ---
        let current_state = self.materialize_heads(&current_view.heads)?;
        check_working_dir_clean(&self.work_root, &current_state, &self.store, "merging")?;

        // Find LCA.
        let lca_heads = self
            .graph
            .load()
            .merge_base(&current_view.heads, target_heads);

        // Compute ancestors from each side and from the LCA.
        let ancestors_current = self.graph.load().ancestors(&current_view.heads);
        let ancestors_target = self.graph.load().ancestors(target_heads);
        let ancestors_lca = if lca_heads.is_empty() {
            HashSet::new()
        } else {
            self.graph.load().ancestors(&lca_heads)
        };

        // Delta A = changes in current but not in LCA ancestry.
        let delta_a: Vec<Blake3Hash> = ancestors_current
            .difference(&ancestors_lca)
            .copied()
            .collect();

        // Delta B = changes in target but not in LCA ancestry.
        let delta_b: Vec<Blake3Hash> = ancestors_target
            .difference(&ancestors_lca)
            .copied()
            .collect();

        let mut delta_a = delta_a;
        let mut delta_b = delta_b;
        delta_a.sort();
        delta_b.sort();

        // Cross-product commutativity check.
        let mut conflicting_pairs = Vec::new();
        let g = self.graph.load_full();
        for &id_a in &delta_a {
            let change_a = g
                .get(&id_a)
                .ok_or_else(|| anyhow::anyhow!("change missing from graph"))?;
            for &id_b in &delta_b {
                let change_b = g
                    .get(&id_b)
                    .ok_or_else(|| anyhow::anyhow!("change missing from graph"))?;
                if !arc_algebra::commute::commutes(change_a, change_b) {
                    conflicting_pairs.push((id_a, id_b));
                }
            }
        }

        if !conflicting_pairs.is_empty() {
            // Preserve legacy metadata for Ghost Node / resolve workflow.
            let conflict = PendingConflict {
                current_view: current_name.clone(),
                target_heads: target_heads.clone(),
                conflicting_pairs: conflicting_pairs.clone(),
            };
            let conflict_path = self.shared_root.join(".arc").join("conflict");
            let bytes = bincode::serialize(&conflict)
                .map_err(|e| anyhow::anyhow!("failed to serialize conflict: {e}"))?;
            fs::write(&conflict_path, bytes)?;

            // Materialize the three states used to build first-class conflict atoms.
            let target_state = self.materialize_heads(target_heads)?;
            let lca_state = if lca_heads.is_empty() {
                arc_algebra::apply::MaterializedState::new()
            } else {
                self.materialize_heads(&lca_heads)?
            };

            let mut conflict_atoms = Vec::new();
            let mut seen_paths: HashSet<NodePath> = HashSet::new();

            for (id_a, id_b) in &conflicting_pairs {
                let change_a = g
                    .get(id_a)
                    .ok_or_else(|| anyhow::anyhow!("conflicting change missing from graph"))?;
                let change_b = g
                    .get(id_b)
                    .ok_or_else(|| anyhow::anyhow!("conflicting change missing from graph"))?;

                let overlap = find_overlapping_path(&change_a.atoms, &change_b.atoms)
                    .ok_or_else(|| anyhow::anyhow!("no overlapping path found for conflict"))?;

                if !seen_paths.insert(overlap.clone()) {
                    continue;
                }

                let base_bytes = extract_content_at_path(&lca_state, &overlap);
                let ours_bytes = extract_content_at_path(&current_state, &overlap);
                let theirs_bytes = extract_content_at_path(&target_state, &overlap);

                let base_hash = self
                    .store
                    .write_blob(&base_bytes)
                    .map_err(|e| anyhow::anyhow!("failed to write conflict base blob: {e}"))?;
                let ours_hash = self
                    .store
                    .write_blob(&ours_bytes)
                    .map_err(|e| anyhow::anyhow!("failed to write conflict side blob: {e}"))?;
                let theirs_hash = self
                    .store
                    .write_blob(&theirs_bytes)
                    .map_err(|e| anyhow::anyhow!("failed to write conflict side blob: {e}"))?;

                let mut bases = vec![base_hash];
                let mut sides = vec![ours_hash, theirs_hash];
                bases.sort();
                sides.sort();

                conflict_atoms.push(Atom::Conflict {
                    bases,
                    sides,
                    at: overlap,
                });
            }

            conflict_atoms.sort_by(|a, b| {
                let a_path = match a {
                    Atom::Conflict { at, .. } => at,
                    _ => unreachable!("conflict_atoms only contains Atom::Conflict"),
                };
                let b_path = match b {
                    Atom::Conflict { at, .. } => at,
                    _ => unreachable!("conflict_atoms only contains Atom::Conflict"),
                };
                a_path.cmp(b_path)
            });

            if conflict_atoms.is_empty() {
                anyhow::bail!("detected semantic conflict but failed to construct conflict atoms");
            }

            let (author, signing_key) = self.signing_identity()?;
            let mut merge_deps = current_view.heads.clone();
            merge_deps.extend(target_heads);
            let conflict_change = Change::new(
                merge_deps,
                conflict_atoms,
                format!("merge conflict: {} pair(s)", conflicting_pairs.len()),
                author.clone(),
                signing_key,
            );

            self.store
                .write_change(&conflict_change)
                .map_err(|e| anyhow::anyhow!("failed to persist conflict change: {e}"))?;
            self.graph_add_change(conflict_change.clone());

            let prev_heads = current_view.heads.clone();
            let merged_heads = HashSet::from([conflict_change.id]);

            let updated_view = View::new(&current_name, merged_heads.clone());
            updated_view
                .save(&self.shared_root)
                .map_err(|e| anyhow::anyhow!("failed to save merged view: {e}"))?;

            self.log_operation("merge", &current_name, prev_heads, merged_heads.clone())?;

            let merged_state = self.materialize_heads(&merged_heads)?;
            write_state_to_working_dir(&self.work_root, &self.shared_root, &merged_state)?;
            self.run_hook("post-merge")?;
            tracing::info!("merge_heads complete (conflict change)");
            return Ok(());
        }

        // All commute - union the heads.
        let prev_heads = current_view.heads.clone();
        let mut merged_heads = current_view.heads;
        merged_heads.extend(target_heads);

        let updated_view = View::new(&current_name, merged_heads.clone());
        updated_view
            .save(&self.shared_root)
            .map_err(|e| anyhow::anyhow!("failed to save merged view: {e}"))?;

        // Record the completed merge in the spacetime log.
        self.log_operation("merge", &current_name, prev_heads, merged_heads.clone())?;

        // Re-materialize and write to working directory.
        let merged_state = self.materialize_heads(&merged_heads)?;
        write_state_to_working_dir(&self.work_root, &self.shared_root, &merged_state)?;
        self.run_hook("post-merge")?;
        tracing::info!("merge_heads complete");
        Ok(())
    }
}
