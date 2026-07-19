//! Algebraic history mutator engine.
//!
//! This module provides rewrite-safe history operations that preserve
//! materialized state semantics while regenerating content-addressed ids.

use std::collections::{BTreeMap, HashMap, HashSet};

use arc_algebra::commute::commute_pair;
use arc_algebra_types::Blake3Hash;
use arc_change::Change;
use arc_store_graph::ChangeGraph;
use arc_store_types::author::Author;
use arc_store_types::newtypes::ChangeId;
use thiserror::Error;

/// Result of a squash rewrite.
#[derive(Debug, Clone)]
pub struct SquashOutcome {
    /// Newly generated squashed change.
    pub squashed: Change,
    /// Old -> new mapping for every rewritten change id.
    pub rewrite_map: BTreeMap<ChangeId, ChangeId>,
}

/// Result of a reorder rewrite.
#[derive(Debug, Clone)]
pub struct ReorderOutcome {
    /// Rewritten changes in causal application order.
    pub rewritten: Vec<Change>,
    /// Old -> new mapping for every rewritten change id.
    pub rewrite_map: BTreeMap<ChangeId, ChangeId>,
    /// New head after rewriting.
    pub new_head: ChangeId,
}

/// Errors produced by history rewrite operations.
#[derive(Debug, Error)]
pub enum MutatorError {
    /// Target id does not exist in the graph.
    #[error("target change {0} not found")]
    TargetNotFound(ChangeId),
    /// Target id is not reachable from the selected heads.
    #[error("target change {0} is not an ancestor of the view heads")]
    TargetNotAncestor(ChangeId),
    /// Selected ids are not a single contiguous linear chain.
    #[error("selected changes do not form a contiguous linear chain")]
    NonLinearChain,
    /// Reorder input contains duplicates, missing ids, or unknown ids.
    #[error("reorder set must contain unique existing ids")]
    InvalidReorderSet,
    /// Adjacent swap failed commutativity checks.
    #[error("changes {0} and {1} do not commute")]
    NonCommutingPair(ChangeId, ChangeId),
}

/// Squash the contiguous linear chain from `target_id` to `view_heads`.
pub fn squash_into(
    graph: &ChangeGraph,
    view_heads: &HashSet<Blake3Hash>,
    target_id: Blake3Hash,
    signer: &(Author, ed25519_dalek::SigningKey),
) -> Result<SquashOutcome, MutatorError> {
    let target =
        graph.get(&target_id).ok_or(MutatorError::TargetNotFound(ChangeId::from(target_id)))?;

    let spine = collect_linear_spine(graph, view_heads, target_id)?;
    let mut atoms = Vec::new();
    for id in &spine {
        let change = graph.get(id).ok_or(MutatorError::TargetNotFound(ChangeId::from(*id)))?;
        atoms.extend(change.atoms.clone());
    }

    let count = spine.len();
    let intent = format!("Squash: {count} changes into \"{}\"", target.intent);
    let (author, signing_key) = signer;
    let squashed = Change::rewritten_or_resigned(
        target,
        target.deps.clone(),
        atoms,
        intent,
        author.clone(),
        signing_key,
    );

    let mut rewrite_map = BTreeMap::new();
    for id in spine {
        rewrite_map.insert(ChangeId::from(id), ChangeId::from(squashed.id));
    }

    Ok(SquashOutcome { squashed, rewrite_map })
}

/// Reorder a contiguous linear chain into `desired_order`.
///
/// `desired_order` must contain the same ids as the current chain exactly once.
pub fn reorder(
    graph: &ChangeGraph,
    desired_order: &[Blake3Hash],
    signer: &(Author, ed25519_dalek::SigningKey),
) -> Result<ReorderOutcome, MutatorError> {
    if desired_order.len() < 2 {
        return Err(MutatorError::InvalidReorderSet);
    }

    let mut desired_set = HashSet::new();
    for id in desired_order {
        if !desired_set.insert(*id) || graph.get(id).is_none() {
            return Err(MutatorError::InvalidReorderSet);
        }
    }

    let current_order = resolve_linear_chain(graph, &desired_set)?;
    let current_set: HashSet<Blake3Hash> = current_order.iter().copied().collect();
    if current_set != desired_set {
        return Err(MutatorError::InvalidReorderSet);
    }

    let mut desired_pos = HashMap::new();
    for (idx, id) in desired_order.iter().copied().enumerate() {
        desired_pos.insert(id, idx);
    }

    #[derive(Clone)]
    struct WorkingChange {
        origin: Blake3Hash,
        change: Change,
    }

    let mut working: Vec<WorkingChange> = current_order
        .iter()
        .map(|id| WorkingChange {
            origin: *id,
            change: graph.get(id).expect("validated existing id").clone(),
        })
        .collect();

    let len = working.len();
    for _ in 0..len {
        let mut swapped = false;
        for i in 0..(len - 1) {
            let left_pos =
                *desired_pos.get(&working[i].origin).ok_or(MutatorError::InvalidReorderSet)?;
            let right_pos =
                *desired_pos.get(&working[i + 1].origin).ok_or(MutatorError::InvalidReorderSet)?;

            if left_pos <= right_pos {
                continue;
            }

            let (new_left, new_right) =
                commute_linear_pair(&working[i].change, &working[i + 1].change, signer).ok_or(
                    MutatorError::NonCommutingPair(
                        ChangeId::from(working[i].origin),
                        ChangeId::from(working[i + 1].origin),
                    ),
                )?;

            let left_origin = working[i].origin;
            working[i] = WorkingChange { origin: working[i + 1].origin, change: new_left };
            working[i + 1] = WorkingChange { origin: left_origin, change: new_right };
            swapped = true;
        }
        if !swapped {
            break;
        }
    }

    let final_origins: Vec<Blake3Hash> = working.iter().map(|w| w.origin).collect();
    if final_origins != desired_order {
        return Err(MutatorError::NonLinearChain);
    }

    let selected_set: HashSet<Blake3Hash> = desired_set;
    let (author, signing_key) = signer;
    let mut rewrite_map_raw: HashMap<Blake3Hash, Blake3Hash> = HashMap::new();
    let mut rewritten = Vec::with_capacity(working.len());
    let mut prev_new_id: Option<Blake3Hash> = None;

    for work in &working {
        let mut deps = HashSet::new();
        for dep in &work.change.deps {
            if selected_set.contains(dep) {
                if let Some(mapped) = rewrite_map_raw.get(dep) {
                    deps.insert(*mapped);
                }
            } else {
                deps.insert(*dep);
            }
        }
        if let Some(prev) = prev_new_id {
            deps.insert(prev);
        }

        let rewritten_change = Change::rewritten_or_resigned(
            &work.change,
            deps,
            work.change.atoms.clone(),
            work.change.intent.clone(),
            author.clone(),
            signing_key,
        );
        prev_new_id = Some(rewritten_change.id);
        rewrite_map_raw.insert(work.origin, rewritten_change.id);
        rewritten.push(rewritten_change);
    }

    let mut rewrite_map = BTreeMap::new();
    for (old, new) in rewrite_map_raw {
        rewrite_map.insert(ChangeId::from(old), ChangeId::from(new));
    }

    let new_head =
        rewritten.last().map(|c| ChangeId::from(c.id)).ok_or(MutatorError::InvalidReorderSet)?;

    Ok(ReorderOutcome { rewritten, rewrite_map, new_head })
}

fn commute_linear_pair(
    left: &Change,
    right: &Change,
    signer: &(Author, ed25519_dalek::SigningKey),
) -> Option<(Change, Change)> {
    if right.deps.contains(&left.id) {
        let mut relaxed_right = right.clone();
        let _ = relaxed_right.deps.remove(&left.id);
        commute_pair(left, &relaxed_right, signer)
    } else {
        commute_pair(left, right, signer)
    }
}

fn collect_linear_spine(
    graph: &ChangeGraph,
    view_heads: &HashSet<Blake3Hash>,
    target_id: Blake3Hash,
) -> Result<Vec<Blake3Hash>, MutatorError> {
    let ancestors = graph.ancestors(view_heads);
    if !ancestors.contains(&target_id) && !view_heads.contains(&target_id) {
        return Err(MutatorError::TargetNotAncestor(ChangeId::from(target_id)));
    }

    let topo = graph.topological_sort(view_heads);
    let start_idx = topo
        .iter()
        .position(|id| *id == target_id)
        .ok_or(MutatorError::TargetNotFound(ChangeId::from(target_id)))?;
    let spine: Vec<Blake3Hash> = topo[start_idx..].to_vec();
    let spine_set: HashSet<Blake3Hash> = spine.iter().copied().collect();

    if spine.len() < 2 {
        return Ok(spine);
    }

    for id in &spine {
        let child_count = graph
            .child_ids(ChangeId::from(*id))
            .into_iter()
            .filter(|candidate| spine_set.contains(&Blake3Hash::from(*candidate)))
            .count();
        if id == spine.last().expect("non-empty spine") {
            if child_count > 0 {
                return Err(MutatorError::NonLinearChain);
            }
        } else if child_count != 1 {
            return Err(MutatorError::NonLinearChain);
        }
    }

    Ok(spine)
}

fn resolve_linear_chain(
    graph: &ChangeGraph,
    selected: &HashSet<Blake3Hash>,
) -> Result<Vec<Blake3Hash>, MutatorError> {
    let mut roots = Vec::new();
    let mut children_by_id: HashMap<Blake3Hash, Vec<Blake3Hash>> = HashMap::new();

    for id in selected {
        let parents_in_selected: Vec<Blake3Hash> = graph
            .parent_ids(ChangeId::from(*id))
            .into_iter()
            .map(Blake3Hash::from)
            .filter(|dep| selected.contains(dep))
            .collect();
        if parents_in_selected.is_empty() {
            roots.push(*id);
        }
        if parents_in_selected.len() > 1 {
            return Err(MutatorError::NonLinearChain);
        }

        let children_in_selected: Vec<Blake3Hash> = graph
            .child_ids(ChangeId::from(*id))
            .into_iter()
            .map(Blake3Hash::from)
            .filter(|child| selected.contains(child))
            .collect();
        if children_in_selected.len() > 1 {
            return Err(MutatorError::NonLinearChain);
        }
        children_by_id.insert(*id, children_in_selected);
    }

    if roots.len() != 1 {
        return Err(MutatorError::NonLinearChain);
    }

    let mut ordered = Vec::with_capacity(selected.len());
    let mut current = roots[0];
    let mut seen = HashSet::new();

    loop {
        if !seen.insert(current) {
            return Err(MutatorError::NonLinearChain);
        }
        ordered.push(current);

        let children = children_by_id.get(&current).ok_or(MutatorError::NonLinearChain)?;
        if children.is_empty() {
            break;
        }
        current = children[0];
    }

    if ordered.len() != selected.len() {
        return Err(MutatorError::NonLinearChain);
    }

    Ok(ordered)
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use arc_algebra_types::Atom;
    use arc_store_types::author::test_keypair;

    use super::*;

    fn change_with_path(deps: HashSet<Blake3Hash>, path: &str) -> Change {
        let (author, signing_key) = test_keypair();
        Change::new(
            deps,
            vec![Atom::Insert {
                at: vec!["file".to_string(), format!("{path}.rs")],
                content_hash: *blake3::hash(path.as_bytes()).as_bytes(),
            }],
            format!("change-{path}"),
            author,
            &signing_key,
        )
    }

    #[test]
    fn squash_generates_rewrite_map_for_entire_spine() {
        let a = change_with_path(HashSet::new(), "a");
        let b = change_with_path(HashSet::from([a.id]), "b");
        let c = change_with_path(HashSet::from([b.id]), "c");

        let mut graph = ChangeGraph::new();
        graph.add_change(a.clone());
        graph.add_change(b.clone());
        graph.add_change(c.clone());

        let (author, signing_key) = test_keypair();
        let out = squash_into(&graph, &HashSet::from([c.id]), a.id, &(author, signing_key))
            .expect("squash should succeed");

        assert_eq!(out.squashed.atoms.len(), 3);
        assert_eq!(out.rewrite_map.len(), 3);
        assert_eq!(out.rewrite_map[&ChangeId::from(a.id)], ChangeId::from(out.squashed.id));
        assert_eq!(out.rewrite_map[&ChangeId::from(b.id)], ChangeId::from(out.squashed.id));
        assert_eq!(out.rewrite_map[&ChangeId::from(c.id)], ChangeId::from(out.squashed.id));
    }

    #[test]
    fn reorder_linear_chain_with_commuting_atoms() {
        let a = change_with_path(HashSet::new(), "a");
        let b = change_with_path(HashSet::from([a.id]), "b");
        let c = change_with_path(HashSet::from([b.id]), "c");

        let mut graph = ChangeGraph::new();
        graph.add_change(a.clone());
        graph.add_change(b.clone());
        graph.add_change(c.clone());

        let (author, signing_key) = test_keypair();
        let out = reorder(&graph, &[a.id, c.id, b.id], &(author, signing_key))
            .expect("reorder should succeed");

        assert_eq!(out.rewritten.len(), 3);
        assert_eq!(out.rewrite_map.len(), 3);
        assert_eq!(out.new_head, out.rewrite_map[&ChangeId::from(b.id)]);
        let rewritten_ids: HashSet<ChangeId> = out.rewrite_map.values().copied().collect();
        assert!(rewritten_ids.len() >= 2, "reorder should produce at least one rewritten id");
    }

    #[test]
    fn reorder_rejects_non_commuting_atoms() {
        let (author, signing_key) = test_keypair();
        let a = Change::new(
            HashSet::new(),
            vec![Atom::Insert {
                at: vec!["file".to_string(), "same.rs".to_string()],
                content_hash: [1u8; 32],
            }],
            "a",
            author.clone(),
            &signing_key,
        );
        let b = Change::new(
            HashSet::from([a.id]),
            vec![Atom::Delete {
                at: vec!["file".to_string(), "same.rs".to_string()],
                prior_hash: [1u8; 32],
            }],
            "b",
            author,
            &signing_key,
        );

        let mut graph = ChangeGraph::new();
        graph.add_change(a.clone());
        graph.add_change(b.clone());

        let (author2, signing_key2) = test_keypair();
        let err = reorder(&graph, &[b.id, a.id], &(author2, signing_key2))
            .expect_err("non-commuting reorder must fail");
        assert!(matches!(err, MutatorError::NonCommutingPair(_, _)));
    }
}
