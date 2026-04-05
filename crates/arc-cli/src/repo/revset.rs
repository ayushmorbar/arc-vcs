use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Instant;

use arc_change::Change;
use arc_store_types::newtypes::ChangeId;

use super::core::*;

impl Repository {
    /// Return all changes selected by `revset`, newest-first.
    pub fn log_revset(&mut self, revset: &str) -> anyhow::Result<Vec<Change>> {
        let expr = arc_revset::parse(revset)
            .map_err(|e| anyhow::anyhow!("invalid revset '{}': {e}", revset))?;
        let expr = constrain_touched_to_current_view(&expr);
        self.prepare_revset(&expr)?;

        let graph = self.graph_snapshot();
        let mut resolver = |symbol: &str| self.resolve_revset_symbol_typed(symbol);
        let mut refs_resolver =
            |function_name: &str| self.resolve_revset_reference_heads(function_name);
        let selected: BTreeSet<ChangeId> = arc_revset::compile_change_ids_with_refs(
            &expr,
            Arc::clone(&graph),
            &mut resolver,
            &mut refs_resolver,
        )?
        .collect();

        let mut ordered_ids = graph.topological_sort_ids(&selected);
        ordered_ids.reverse();
        ordered_ids
            .into_iter()
            .map(|id| self.read_change(&arc_algebra_types::Blake3Hash::from(id)))
            .collect()
    }

    /// Benchmark revset evaluation by counting selected revisions.
    pub fn bench_revset(&mut self, revset: &str, iterations: u32) -> anyhow::Result<(u128, usize)> {
        let mut total_nanos = 0u128;
        let mut last_count = 0usize;
        for _ in 0..iterations.max(1) {
            let start = Instant::now();
            let selected = self.resolve_revset_ids(revset)?;
            total_nanos += start.elapsed().as_nanos();
            last_count = selected.len();
        }
        Ok((total_nanos, last_count))
    }

    #[tracing::instrument(skip_all, fields(revset = %revset))]
    pub(super) fn resolve_revset_ids(
        &mut self,
        revset: &str,
    ) -> anyhow::Result<BTreeSet<ChangeId>> {
        let expr = arc_revset::parse(revset)
            .map_err(|e| anyhow::anyhow!("invalid revset '{}': {e}", revset))?;
        let expr = constrain_touched_to_current_view(&expr);
        self.prepare_revset(&expr)?;

        let graph = self.graph_snapshot();
        let mut resolver = |symbol: &str| self.resolve_revset_symbol_typed(symbol);
        let mut refs_resolver =
            |function_name: &str| self.resolve_revset_reference_heads(function_name);
        let selected: BTreeSet<ChangeId> = arc_revset::compile_change_ids_with_refs(
            &expr,
            Arc::clone(&graph),
            &mut resolver,
            &mut refs_resolver,
        )?
        .collect();
        Ok(selected)
    }
}
