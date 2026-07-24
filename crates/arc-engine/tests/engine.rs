use std::{
    collections::HashSet,
    sync::atomic::{AtomicU32, Ordering},
};

use arc_algebra_types::{Atom, Blake3Hash};
use arc_change::Change;
use arc_engine::{
    mutator::{self, MutatorError, ReorderOutcome, SquashOutcome},
    spacetime::{self, SpacetimeError},
    task_harness::{EngineTask, TaskRegistry},
};
use arc_store_cas::ObjectStore;
use arc_store_graph::ChangeGraph;
use arc_store_types::{
    author::{Author, test_keypair},
    newtypes::ChangeId,
};
use ed25519_dalek::SigningKey;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn make_store() -> (tempfile::TempDir, ObjectStore) {
    let dir = tempfile::tempdir().unwrap();
    let store = ObjectStore::new(dir.path());
    (dir, store)
}

fn signer() -> (Author, SigningKey) {
    test_keypair()
}

fn make_atom(path: &str) -> Atom {
    Atom::Insert {
        at: vec!["root".to_string(), format!("{path}.rs")],
        content_hash: *blake3::hash(path.as_bytes()).as_bytes(),
    }
}

fn make_change(deps: HashSet<Blake3Hash>, path: &str) -> Change {
    let (author, key) = signer();
    Change::new(deps, vec![make_atom(path)], path, author, &key)
}

fn chain3() -> (ChangeGraph, Change, Change, Change) {
    let a = make_change(HashSet::new(), "alpha");
    let b = make_change(HashSet::from([a.id]), "beta");
    let c = make_change(HashSet::from([b.id]), "gamma");

    let mut graph = ChangeGraph::new();
    graph.add_change(a.clone());
    graph.add_change(b.clone());
    graph.add_change(c.clone());
    (graph, a, b, c)
}

struct CountingTask {
    id: &'static str,
    count: AtomicU32,
}

impl EngineTask for CountingTask {
    fn id(&self) -> &'static str {
        self.id
    }

    fn run(&self) -> anyhow::Result<()> {
        self.count.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }
}

struct FailTask;

impl EngineTask for FailTask {
    fn id(&self) -> &'static str {
        "fail"
    }

    fn run(&self) -> anyhow::Result<()> {
        Err(anyhow::anyhow!("deliberate failure"))
    }
}

struct SlowTask {
    id: &'static str,
}

impl EngineTask for SlowTask {
    fn id(&self) -> &'static str {
        self.id
    }

    fn run(&self) -> anyhow::Result<()> {
        std::thread::sleep(std::time::Duration::from_millis(5));
        Ok(())
    }
}

// ===========================================================================
// spacetime::squash_into tests
// ===========================================================================

#[test]
fn spacetime_squash_linear_chain_fuses_atoms_and_inherits_deps() {
    let (_dir, store) = make_store();
    let a = make_change(HashSet::new(), "root.a");
    let b = make_change(HashSet::from([a.id]), "root.b");
    let c = make_change(HashSet::from([b.id]), "root.c");

    let mut graph = ChangeGraph::new();
    graph.add_change(a.clone());
    graph.add_change(b.clone());
    graph.add_change(c.clone());

    let (author, key) = signer();
    let view_heads = HashSet::from([c.id]);
    let result = spacetime::squash_into(&graph, &store, &view_heads, a.id, &(author, key))
        .expect("must squash linear chain");

    assert_eq!(result.atoms.len(), 3, "all atoms from spine must be fused");
    assert!(result.deps.is_empty(), "squashed change inherits target deps (a has empty deps)");
    assert!(result.verify_signature(), "squashed change must be signed");
    assert!(result.intent.contains("Squash"), "intent must mention Squash");
    assert!(result.intent.contains("3"), "intent must contain the count of squashed changes");
}

#[test]
fn spacetime_squash_preserves_target_deps_not_head_deps() {
    let (_dir, store) = make_store();

    // A → B → C chain. A depends on an external change X.
    let x = make_change(HashSet::new(), "external.x");
    let a = make_change(HashSet::from([x.id]), "root.a");
    let b = make_change(HashSet::from([a.id]), "root.b");

    let mut graph = ChangeGraph::new();
    graph.add_change(x.clone());
    graph.add_change(a.clone());
    graph.add_change(b.clone());

    let (author, key) = signer();
    let view_heads = HashSet::from([b.id]);
    let result = spacetime::squash_into(&graph, &store, &view_heads, a.id, &(author, key))
        .expect("must squash");

    assert_eq!(result.deps, HashSet::from([x.id]), "must inherit target's deps, not head's");
}

#[test]
fn spacetime_squash_empty_spine_is_valid() {
    let (_dir, store) = make_store();
    let a = make_change(HashSet::new(), "solo");

    let mut graph = ChangeGraph::new();
    graph.add_change(a.clone());

    let (author, key) = signer();
    let view_heads = HashSet::from([a.id]);
    let result = spacetime::squash_into(&graph, &store, &view_heads, a.id, &(author, key))
        .expect("squashing a single change to itself should work");

    assert_eq!(result.atoms.len(), 1, "single change squash preserves its atom");
    assert!(result.intent.contains("1"), "intent says 1 change squashed");
}

#[test]
fn spacetime_squash_target_not_found() {
    let (_dir, store) = make_store();
    let graph = ChangeGraph::new();
    let (author, key) = signer();
    let heads: HashSet<Blake3Hash> = HashSet::new();
    let missing: Blake3Hash = [0xDE; 32];

    let err = spacetime::squash_into(&graph, &store, &heads, missing, &(author, key)).unwrap_err();
    assert!(matches!(err, SpacetimeError::TargetNotFound(_)));
}

#[test]
fn spacetime_squash_target_not_ancestor_of_heads() {
    let (_dir, store) = make_store();

    // Two unrelated chains: A→B and X→Y. Target is X but heads contain B.
    let a = make_change(HashSet::new(), "chain1.a");
    let b = make_change(HashSet::from([a.id]), "chain1.b");
    let x = make_change(HashSet::new(), "chain2.x");
    let y = make_change(HashSet::from([x.id]), "chain2.y");

    let mut graph = ChangeGraph::new();
    graph.add_change(a.clone());
    graph.add_change(b.clone());
    graph.add_change(x.clone());
    graph.add_change(y.clone());

    let (author, key) = signer();
    let heads = HashSet::from([b.id]);
    let err = spacetime::squash_into(&graph, &store, &heads, x.id, &(author, key)).unwrap_err();
    assert!(
        matches!(err, SpacetimeError::TargetNotAncestor(_)),
        "target from different chain must fail with TargetNotAncestor"
    );
}

#[test]
fn spacetime_squash_forked_chain_returns_non_linear_spine() {
    let (_dir, store) = make_store();

    // A → B, A → C, heads = {B, C}. Fork at A.
    let a = make_change(HashSet::new(), "fork.root");
    let b = make_change(HashSet::from([a.id]), "fork.left");
    let c = make_change(HashSet::from([a.id]), "fork.right");

    let mut graph = ChangeGraph::new();
    graph.add_change(a.clone());
    graph.add_change(b.clone());
    graph.add_change(c.clone());

    let (author, key) = signer();
    let heads = HashSet::from([b.id, c.id]);
    let err = spacetime::squash_into(&graph, &store, &heads, a.id, &(author, key)).unwrap_err();
    assert!(matches!(err, SpacetimeError::NonLinearSpine(_)));
}

#[test]
fn spacetime_squash_two_change_chain() {
    let (_dir, store) = make_store();
    let a = make_change(HashSet::new(), "pair.first");
    let b = make_change(HashSet::from([a.id]), "pair.second");

    let mut graph = ChangeGraph::new();
    graph.add_change(a.clone());
    graph.add_change(b.clone());

    let (author, key) = signer();
    let heads = HashSet::from([b.id]);
    let result = spacetime::squash_into(&graph, &store, &heads, a.id, &(author, key))
        .expect("two-change squash must succeed");

    assert_eq!(result.atoms.len(), 2);
    assert!(result.deps.is_empty());
    assert!(result.verify_signature());
}

// ===========================================================================
// mutator::squash_into tests
// ===========================================================================

#[test]
fn mutator_squash_collects_rewrite_map_for_all_spine_changes() {
    let (graph, a, b, c) = chain3();
    let (author, key) = signer();
    let heads = HashSet::from([c.id]);

    let outcome: SquashOutcome =
        mutator::squash_into(&graph, &heads, a.id, &(author, key)).expect("must squash");

    assert_eq!(outcome.squashed.atoms.len(), 3);
    assert_eq!(outcome.rewrite_map.len(), 3);
    assert_eq!(
        outcome.rewrite_map[&arc_store_types::newtypes::ChangeId::from(a.id)],
        arc_store_types::newtypes::ChangeId::from(outcome.squashed.id)
    );
    assert_eq!(
        outcome.rewrite_map[&arc_store_types::newtypes::ChangeId::from(b.id)],
        arc_store_types::newtypes::ChangeId::from(outcome.squashed.id)
    );
    assert_eq!(
        outcome.rewrite_map[&arc_store_types::newtypes::ChangeId::from(c.id)],
        arc_store_types::newtypes::ChangeId::from(outcome.squashed.id)
    );
}

#[test]
fn mutator_squash_inherits_target_deps() {
    let x = make_change(HashSet::new(), "dep.x");
    let a = make_change(HashSet::from([x.id]), "squash.a");
    let b = make_change(HashSet::from([a.id]), "squash.b");

    let mut graph = ChangeGraph::new();
    graph.add_change(x.clone());
    graph.add_change(a.clone());
    graph.add_change(b.clone());

    let (author, key) = signer();
    let heads = HashSet::from([b.id]);
    let outcome = mutator::squash_into(&graph, &heads, a.id, &(author, key)).expect("must squash");

    assert_eq!(outcome.squashed.deps, HashSet::from([x.id]), "squashed must inherit target deps");
}

#[test]
fn mutator_squash_single_change_to_itself() {
    let a = make_change(HashSet::new(), "solo.sq");
    let mut graph = ChangeGraph::new();
    graph.add_change(a.clone());

    let (author, key) = signer();
    let heads = HashSet::from([a.id]);
    let outcome = mutator::squash_into(&graph, &heads, a.id, &(author, key))
        .expect("single change squash must succeed");

    assert_eq!(outcome.squashed.atoms.len(), 1);
    assert_eq!(outcome.rewrite_map.len(), 1);
}

#[test]
fn mutator_squash_target_not_found() {
    let graph = ChangeGraph::new();
    let (author, key) = signer();
    let heads: HashSet<Blake3Hash> = HashSet::new();
    let missing: Blake3Hash = [0xAB; 32];

    let err = mutator::squash_into(&graph, &heads, missing, &(author, key)).unwrap_err();
    assert!(matches!(err, MutatorError::TargetNotFound(_)));
}

#[test]
fn mutator_squash_target_not_ancestor() {
    let a = make_change(HashSet::new(), "chain1.a");
    let b = make_change(HashSet::from([a.id]), "chain1.b");
    let x = make_change(HashSet::new(), "chain2.x");
    let y = make_change(HashSet::from([x.id]), "chain2.y");

    let mut graph = ChangeGraph::new();
    graph.add_change(a.clone());
    graph.add_change(b.clone());
    graph.add_change(x.clone());
    graph.add_change(y.clone());

    let (author, key) = signer();
    let heads = HashSet::from([b.id]);
    let err = mutator::squash_into(&graph, &heads, x.id, &(author, key)).unwrap_err();
    assert!(matches!(err, MutatorError::TargetNotAncestor(_)));
}

#[test]
fn mutator_squash_forked_chain_returns_non_linear_chain() {
    let a = make_change(HashSet::new(), "fork.mr");
    let b = make_change(HashSet::from([a.id]), "fork.left");
    let c = make_change(HashSet::from([a.id]), "fork.right");

    let mut graph = ChangeGraph::new();
    graph.add_change(a.clone());
    graph.add_change(b.clone());
    graph.add_change(c.clone());

    let (author, key) = signer();
    let heads = HashSet::from([b.id, c.id]);
    let err = mutator::squash_into(&graph, &heads, a.id, &(author, key)).unwrap_err();
    assert!(matches!(err, MutatorError::NonLinearChain));
}

// ===========================================================================
// mutator::reorder tests
// ===========================================================================

#[test]
fn reorder_identity_keeps_same_atoms_and_valid_signature() {
    let (graph, a, b, c) = chain3();
    let (author, key) = signer();

    let outcome: ReorderOutcome = mutator::reorder(&graph, &[a.id, b.id, c.id], &(author, key))
        .expect("identity reorder must succeed");

    assert_eq!(outcome.rewritten.len(), 3);
    assert!(outcome.rewritten.iter().all(|c| c.verify_signature()));
    // new_head should correspond to last element in desired order
    assert_eq!(
        outcome.new_head,
        arc_store_types::newtypes::ChangeId::from(c.id),
        "new_head must be the last in the desired order"
    );
}

#[test]
fn reorder_swap_adjacent_commuting_changes() {
    let (graph, a, b, c) = chain3();
    let (author, key) = signer();

    // Try a different order: a, c, b
    let outcome = mutator::reorder(&graph, &[a.id, c.id, b.id], &(author, key))
        .expect("reorder with commuting atoms must succeed");

    assert_eq!(outcome.rewritten.len(), 3);
    assert_eq!(outcome.rewrite_map.len(), 3);
    // All rewritten changes must be cryptographically valid
    assert!(outcome.rewritten.iter().all(|c| c.verify_signature()));
}

#[test]
fn reorder_rejects_non_commuting_insert_delete_pair() {
    let (author, key) = signer();

    // Insert at same path, then Delete at same path — cannot be reordered
    let a = Change::new(
        HashSet::new(),
        vec![Atom::Insert {
            at: vec!["file".to_string(), "same.txt".to_string()],
            content_hash: [0x01; 32],
        }],
        "insert-same",
        author.clone(),
        &key,
    );
    let b = Change::new(
        HashSet::from([a.id]),
        vec![Atom::Delete {
            at: vec!["file".to_string(), "same.txt".to_string()],
            prior_hash: [0x01; 32],
        }],
        "delete-same",
        author,
        &key,
    );

    let mut graph = ChangeGraph::new();
    graph.add_change(a.clone());
    graph.add_change(b.clone());

    let (author2, key2) = signer();
    let err = mutator::reorder(&graph, &[b.id, a.id], &(author2, key2)).unwrap_err();
    assert!(matches!(err, MutatorError::NonCommutingPair(_, _)));
}

#[test]
fn reorder_rejects_single_element() {
    let a = make_change(HashSet::new(), "single");
    let mut graph = ChangeGraph::new();
    graph.add_change(a.clone());

    let (author, key) = signer();
    let err = mutator::reorder(&graph, &[a.id], &(author, key)).unwrap_err();
    assert!(matches!(err, MutatorError::InvalidReorderSet));
}

#[test]
fn reorder_rejects_empty_desired_order() {
    let a = make_change(HashSet::new(), "irrelevant");
    let mut graph = ChangeGraph::new();
    graph.add_change(a);

    let (author, key) = signer();
    let err = mutator::reorder(&graph, &[], &(author, key)).unwrap_err();
    assert!(matches!(err, MutatorError::InvalidReorderSet));
}

#[test]
fn reorder_rejects_duplicate_in_desired_order() {
    let a = make_change(HashSet::new(), "dup.a");
    let b = make_change(HashSet::from([a.id]), "dup.b");

    let mut graph = ChangeGraph::new();
    graph.add_change(a.clone());
    graph.add_change(b.clone());

    let (author, key) = signer();
    let err = mutator::reorder(&graph, &[a.id, a.id, b.id], &(author, key)).unwrap_err();
    assert!(matches!(err, MutatorError::InvalidReorderSet));
}

#[test]
fn reorder_rejects_nonexistent_id_in_desired_order() {
    let a = make_change(HashSet::new(), "exists");
    let mut graph = ChangeGraph::new();
    graph.add_change(a.clone());

    let (author, key) = signer();
    let phantom: Blake3Hash = [0xFF; 32];
    let err = mutator::reorder(&graph, &[a.id, phantom], &(author, key)).unwrap_err();
    assert!(matches!(err, MutatorError::InvalidReorderSet));
}

#[test]
fn reorder_rejects_non_contiguous_set() {
    let a = make_change(HashSet::new(), "n.a");
    let b = make_change(HashSet::from([a.id]), "n.b");
    let c = make_change(HashSet::from([b.id]), "n.c");
    let x = make_change(HashSet::new(), "x");

    let mut graph = ChangeGraph::new();
    graph.add_change(a.clone());
    graph.add_change(b.clone());
    graph.add_change(c.clone());
    graph.add_change(x.clone());

    let (author, key) = signer();
    // a, c, x are not a contiguous linear chain
    let err = mutator::reorder(&graph, &[a.id, c.id, x.id], &(author, key)).unwrap_err();
    assert!(
        matches!(err, MutatorError::InvalidReorderSet | MutatorError::NonLinearChain),
        "non-contiguous set must fail"
    );
}

#[test]
fn reorder_rewrites_deps_of_swapped_changes() {
    let (graph, a, b, c) = chain3();
    let (author, key) = signer();

    // Desired order: a, c, b (swap b and c)
    let outcome = mutator::reorder(&graph, &[a.id, c.id, b.id], &(author, key))
        .expect("reorder must succeed");

    // Find the rewritten change that corresponds to original b
    let old_b = ChangeId::from(b.id);
    let new_b_id = outcome.rewrite_map[&old_b];
    let new_b = outcome.rewritten.iter().find(|c| c.id == new_b_id.0).unwrap();

    // new_b's deps should include new_c (the change that comes before it in the new order)
    let old_c = ChangeId::from(c.id);
    let new_c_id = outcome.rewrite_map[&old_c];
    assert!(
        new_b.deps.contains(&new_c_id.0),
        "rewritten b must depend on rewritten c after reorder"
    );
}

#[test]
fn reorder_deterministic_for_same_input() {
    let (graph, a, b, c) = chain3();

    let (author1, key1) = signer();
    let out1 = mutator::reorder(&graph, &[a.id, c.id, b.id], &(author1, key1)).unwrap();

    let (author2, key2) = signer();
    let out2 = mutator::reorder(&graph, &[a.id, c.id, b.id], &(author2, key2)).unwrap();

    // Different signers produce different ids, but atoms and rewrite_map structure should match
    assert_eq!(out1.rewritten.len(), out2.rewritten.len());
    assert_eq!(out1.rewrite_map.len(), out2.rewrite_map.len());
    // All atoms should be identical
    for (orig, r) in out1.rewritten.iter().zip(out2.rewritten.iter()) {
        assert_eq!(orig.atoms, r.atoms);
    }
}

// ===========================================================================
// task_harness tests
// ===========================================================================

#[test]
fn task_registry_new_is_empty() {
    let registry = TaskRegistry::new();
    assert!(registry.ids().is_empty());
}

#[test]
fn task_registry_register_and_list() {
    let mut registry = TaskRegistry::new();
    registry
        .register(Box::new(CountingTask {
            id: "alpha",
            count: std::sync::atomic::AtomicU32::new(0),
        }))
        .unwrap();
    registry
        .register(Box::new(CountingTask {
            id: "beta",
            count: std::sync::atomic::AtomicU32::new(0),
        }))
        .unwrap();

    let ids = registry.ids();
    assert_eq!(ids.len(), 2);
    assert!(ids.contains(&"alpha"));
    assert!(ids.contains(&"beta"));
}

#[test]
fn task_registry_duplicate_registration_fails() {
    let mut registry = TaskRegistry::new();
    registry
        .register(Box::new(CountingTask { id: "x", count: std::sync::atomic::AtomicU32::new(0) }))
        .unwrap();
    let dup = registry
        .register(Box::new(CountingTask { id: "x", count: std::sync::atomic::AtomicU32::new(0) }));
    assert!(dup.is_err(), "duplicate registration must fail");
}

#[test]
fn task_registry_run_one_returns_correct_result() {
    let mut registry = TaskRegistry::new();
    registry.register(Box::new(SlowTask { id: "slow" })).unwrap();

    let result = registry.run_one("slow").expect("slow task must succeed");
    assert_eq!(result.id, "slow");
    assert!(result.duration >= std::time::Duration::ZERO);
}

#[test]
fn task_registry_run_one_not_found_fails() {
    let registry = TaskRegistry::new();
    let err = registry.run_one("nonexistent").unwrap_err();
    assert!(err.to_string().contains("not found"));
}

#[test]
fn task_registry_run_all_executes_all_tasks() {
    let mut registry = TaskRegistry::new();
    registry
        .register(Box::new(CountingTask { id: "a", count: std::sync::atomic::AtomicU32::new(0) }))
        .unwrap();
    registry
        .register(Box::new(CountingTask { id: "b", count: std::sync::atomic::AtomicU32::new(0) }))
        .unwrap();

    let results = registry.run_all().expect("all tasks must succeed");
    assert_eq!(results.len(), 2);
    assert!(results.iter().all(|r| r.duration >= std::time::Duration::ZERO));
}

#[test]
fn task_registry_run_all_stops_on_first_failure() {
    let mut registry = TaskRegistry::new();
    registry
        .register(Box::new(CountingTask { id: "ok", count: std::sync::atomic::AtomicU32::new(0) }))
        .unwrap();
    registry.register(Box::new(FailTask)).unwrap();

    let err = registry.run_all().unwrap_err();
    assert!(err.to_string().contains("deliberate failure"));
}

#[test]
fn task_registry_ids_are_deterministic() {
    let mut registry = TaskRegistry::new();
    registry
        .register(Box::new(CountingTask { id: "z", count: std::sync::atomic::AtomicU32::new(0) }))
        .unwrap();
    registry
        .register(Box::new(CountingTask { id: "a", count: std::sync::atomic::AtomicU32::new(0) }))
        .unwrap();
    registry
        .register(Box::new(CountingTask { id: "m", count: std::sync::atomic::AtomicU32::new(0) }))
        .unwrap();

    let ids = registry.ids();
    // BTreeMap guarantees sorted order
    assert_eq!(ids, vec!["a", "m", "z"]);
}

#[test]
fn task_registry_run_all_deterministic_order() {
    let mut registry = TaskRegistry::new();
    registry
        .register(Box::new(CountingTask { id: "z", count: std::sync::atomic::AtomicU32::new(0) }))
        .unwrap();
    registry
        .register(Box::new(CountingTask { id: "a", count: std::sync::atomic::AtomicU32::new(0) }))
        .unwrap();

    let results = registry.run_all().unwrap();
    let ids: Vec<&str> = results.iter().map(|r| r.id.as_str()).collect();
    assert_eq!(ids, vec!["a", "z"], "run_all must execute in BTreeMap order");
}

// ===========================================================================
// Cross-module: mutator reorder preserves atom content
// ===========================================================================

#[test]
fn reorder_preserves_atom_content_after_swap() {
    let (graph, a, b, c) = chain3();
    let (author, key) = signer();

    let outcome = mutator::reorder(&graph, &[a.id, c.id, b.id], &(author, key))
        .expect("reorder must succeed");

    // Collect all atoms from the original chain
    let mut original_atoms: Vec<Atom> = Vec::new();
    original_atoms.extend(a.atoms.clone());
    original_atoms.extend(b.atoms.clone());
    original_atoms.extend(c.atoms.clone());

    // Collect all atoms from rewritten chain
    let mut rewritten_atoms: Vec<Atom> = Vec::new();
    for c in &outcome.rewritten {
        rewritten_atoms.extend(c.atoms.clone());
    }

    // Same multiset of atoms must be preserved
    original_atoms.sort_by(|x, y| format!("{x:?}").cmp(&format!("{y:?}")));
    rewritten_atoms.sort_by(|x, y| format!("{x:?}").cmp(&format!("{y:?}")));
    assert_eq!(original_atoms, rewritten_atoms, "atom multiset must be preserved after reorder");
}
