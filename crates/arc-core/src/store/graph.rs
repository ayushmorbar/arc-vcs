use std::collections::{BTreeSet, HashSet, VecDeque};

use crate::algebra::Blake3Hash;
use crate::store::blake3_hasher::Blake3HashMap;
use crate::store::change::Change;
use crate::store::newtypes::ChangeId;

/// In-memory DAG of changes loaded from the CAS.
///
/// Maintains forward edges (`change → its dependencies`) and reverse edges
/// (`dependency → changes that depend on it`) for efficient traversal in
/// both directions.
///
/// # Ghost Nodes
///
/// The graph supports "ghost" dependencies — changes referenced by a
/// `Change.deps` entry that have not yet been inserted via [`ChangeGraph::add_change`].
/// This occurs naturally in a distributed CRDT network where `Change B`
/// (depending on `Change A`) may arrive before `Change A`. All traversals
/// safely halt at graph boundaries by checking `edges.get()` / `nodes.get()`
/// and skipping missing entries.
#[derive(Clone)]
pub struct ChangeGraph {
    /// Every change in the graph, keyed by its content-addressed id.
    /// Uses [`Blake3HashMap`] — the identity hasher extracts the first 8 bytes
    /// of each BLAKE3 digest directly as the bucket index, eliminating
    /// SipHash mixing on every DAG lookup.
    nodes: Blake3HashMap<Change>,
    /// Forward edges: `child → set of parents (dependencies)`.
    edges: Blake3HashMap<HashSet<Blake3Hash>>,
    /// Reverse edges: `parent → set of children (dependents)`.
    reverse_edges: Blake3HashMap<HashSet<Blake3Hash>>,
}

impl ChangeGraph {
    /// Create an empty change graph.
    pub fn new() -> Self {
        Self {
            nodes: Blake3HashMap::default(),
            edges: Blake3HashMap::default(),
            reverse_edges: Blake3HashMap::default(),
        }
    }

    /// Insert a change into the graph, wiring up both forward and reverse edges.
    pub fn add_change(&mut self, change: Change) {
        let id = change.id;
        self.edges.insert(id, change.deps.clone());

        for &dep in &change.deps {
            self.reverse_edges.entry(dep).or_default().insert(id);
        }
        // Ensure the node itself has a reverse-edges entry even if nothing
        // depends on it yet.
        self.reverse_edges.entry(id).or_default();

        self.nodes.insert(id, change);
    }

    /// Look up a change by its hash.
    pub fn get(&self, id: &Blake3Hash) -> Option<&Change> {
        self.nodes.get(id)
    }

    /// Returns the number of changes in the graph.
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    /// Returns `true` if the graph contains no changes.
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// Iterate over all changes in the graph (order is unspecified).
    pub fn iter(&self) -> impl Iterator<Item = &Change> {
        self.nodes.values()
    }

    // ------------------------------------------------------------------
    // Traversal
    // ------------------------------------------------------------------

    /// Return a valid linear application order for the sub-DAG reachable from
    /// `start_heads`, using Kahn's algorithm (BFS-based topological sort).
    ///
    /// The result is ordered *roots-first* — the earliest dependency appears
    /// at index 0. Ties are broken by sorting hashes lexicographically,
    /// ensuring deterministic output regardless of `HashMap` iteration order.
    pub fn topological_sort(&self, start_heads: &HashSet<Blake3Hash>) -> Vec<Blake3Hash> {
        // 1. Collect the sub-DAG reachable from `start_heads`.
        let reachable = self.ancestors_inclusive(start_heads);

        // 2. Compute in-degree within the sub-DAG.
        let mut in_degree: Blake3HashMap<usize> = Blake3HashMap::default();
        for &id in &reachable {
            let count = self.edges.get(&id).map_or(0, |deps| {
                deps.iter().filter(|&d| reachable.contains(d)).count()
            });
            in_degree.insert(id, count);
        }

        // 3. Seed the queue with roots (in-degree 0). Sort for determinism.
        let mut queue: VecDeque<Blake3Hash> = {
            let mut roots: Vec<Blake3Hash> = in_degree
                .iter()
                .filter(|&(_, &deg)| deg == 0)
                .map(|(&id, _)| id)
                .collect();
            roots.sort();
            roots.into()
        };

        // 4. Standard Kahn's: peel off roots, decrement neighbours.
        let mut order = Vec::with_capacity(reachable.len());
        while let Some(id) = queue.pop_front() {
            order.push(id);
            if let Some(children) = self.reverse_edges.get(&id) {
                let mut sorted: Vec<Blake3Hash> = children
                    .iter()
                    .filter(|&c| reachable.contains(c))
                    .copied()
                    .collect();
                sorted.sort();
                for child in sorted {
                    let deg = in_degree.get_mut(&child).unwrap();
                    *deg -= 1;
                    if *deg == 0 {
                        queue.push_back(child);
                    }
                }
            }
        }

        order
    }

    /// Return a deterministic topological order restricted to `selected` IDs.
    ///
    /// The returned order is roots-first. Callers that need newest-first can
    /// reverse the resulting vector.
    pub fn topological_sort_ids(&self, selected: &BTreeSet<ChangeId>) -> Vec<ChangeId> {
        let heads: HashSet<Blake3Hash> = selected.iter().copied().map(Blake3Hash::from).collect();
        let mut order = self.topological_sort(&heads);
        order.retain(|id| selected.contains(&ChangeId::from(*id)));
        order.into_iter().map(ChangeId::from).collect()
    }

    /// Return sorted direct parent IDs for `id`.
    pub fn parent_ids(&self, id: ChangeId) -> Vec<ChangeId> {
        let hash = Blake3Hash::from(id);
        let mut out: Vec<ChangeId> = self
            .edges
            .get(&hash)
            .into_iter()
            .flat_map(|deps| deps.iter().copied())
            .map(ChangeId::from)
            .collect();
        out.sort();
        out
    }

    /// Return sorted direct child IDs for `id`.
    pub fn child_ids(&self, id: ChangeId) -> Vec<ChangeId> {
        let hash = Blake3Hash::from(id);
        let mut out: Vec<ChangeId> = self
            .reverse_edges
            .get(&hash)
            .into_iter()
            .flat_map(|children| children.iter().copied())
            .map(ChangeId::from)
            .collect();
        out.sort();
        out
    }

    /// Return all tip IDs (nodes without children) in deterministic order.
    pub fn tip_ids(&self) -> BTreeSet<ChangeId> {
        let mut tips = BTreeSet::new();
        for &hash in self.nodes.keys() {
            let is_tip = self
                .reverse_edges
                .get(&hash)
                .is_none_or(|children| children.is_empty());
            if is_tip {
                tips.insert(ChangeId::from(hash));
            }
        }
        tips
    }

    // ------------------------------------------------------------------
    // Reachability
    // ------------------------------------------------------------------

    /// All ancestors of `heads` *including* `heads` themselves (BFS over deps).
    pub fn ancestors(&self, heads: &HashSet<Blake3Hash>) -> HashSet<Blake3Hash> {
        self.ancestors_inclusive(heads)
    }

    /// Internal helper: BFS backward through forward edges.
    fn ancestors_inclusive(&self, heads: &HashSet<Blake3Hash>) -> HashSet<Blake3Hash> {
        let mut visited = HashSet::new();
        let mut queue: VecDeque<Blake3Hash> = heads.iter().copied().collect();

        while let Some(id) = queue.pop_front() {
            if !visited.insert(id) {
                continue;
            }
            if let Some(deps) = self.edges.get(&id) {
                for &dep in deps {
                    if !visited.contains(&dep) {
                        queue.push_back(dep);
                    }
                }
            }
        }

        visited
    }

    // ------------------------------------------------------------------
    // Merge base (Lowest Common Ancestors)
    // ------------------------------------------------------------------

    /// Find the **Lowest Common Ancestors** between two sets of heads.
    ///
    /// An LCA is a common ancestor that is not a *strict* ancestor of any
    /// other common ancestor. In a DAG there can be multiple LCAs —
    /// unlike Git's single-LCA assumption.
    pub fn merge_base(
        &self,
        heads_a: &HashSet<Blake3Hash>,
        heads_b: &HashSet<Blake3Hash>,
    ) -> HashSet<Blake3Hash> {
        let ancestors_a = self.ancestors(heads_a);
        let ancestors_b = self.ancestors(heads_b);

        let common: HashSet<Blake3Hash> = ancestors_a.intersection(&ancestors_b).copied().collect();

        if common.is_empty() {
            return HashSet::new();
        }

        // Collect strict ancestors of every common ancestor (within
        // the `common` set). LCAs = common − strict_ancestors.
        let mut strict_ancestors = HashSet::new();
        for &id in &common {
            if let Some(deps) = self.edges.get(&id) {
                for &dep in deps {
                    if common.contains(&dep) {
                        let mut queue: VecDeque<Blake3Hash> = VecDeque::new();
                        queue.push_back(dep);
                        while let Some(cur) = queue.pop_front() {
                            if !strict_ancestors.insert(cur) {
                                continue;
                            }
                            if let Some(parent_deps) = self.edges.get(&cur) {
                                for &p in parent_deps {
                                    if common.contains(&p) && !strict_ancestors.contains(&p) {
                                        queue.push_back(p);
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        common.difference(&strict_ancestors).copied().collect()
    }
}

impl Default for ChangeGraph {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{HashMap, HashSet};

    use super::*;

    /// Helper: create a Change with the given deps and a unique atom
    /// so that each change hashes to a distinct id.
    fn make_change(deps: HashSet<Blake3Hash>, label: &str) -> Change {
        let (author, signing_key) = crate::store::author::test_keypair();
        // Use a label-derived hash to give each change a unique content hash.
        let content_hash: [u8; 32] = *blake3::hash(label.as_bytes()).as_bytes();
        Change::new(
            deps,
            vec![crate::algebra::Atom::Insert {
                at: vec![label.to_string()],
                content_hash,
            }],
            "test",
            author,
            &signing_key,
        )
    }

    /// Build a diamond DAG:
    /// ```text
    ///     A
    ///    / \
    ///   B   C
    ///    \ /
    ///     D
    /// ```
    fn diamond() -> (ChangeGraph, Blake3Hash, Blake3Hash, Blake3Hash, Blake3Hash) {
        let mut g = ChangeGraph::new();

        let a = make_change(HashSet::new(), "a");
        let b = make_change(HashSet::from([a.id]), "b");
        let c = make_change(HashSet::from([a.id]), "c");
        let d = make_change(HashSet::from([b.id, c.id]), "d");

        let (aid, bid, cid, did) = (a.id, b.id, c.id, d.id);
        g.add_change(a);
        g.add_change(b);
        g.add_change(c);
        g.add_change(d);

        (g, aid, bid, cid, did)
    }

    #[test]
    fn test_topological_sort() {
        let (g, a, b, c, d) = diamond();

        let heads = HashSet::from([d]);
        let order = g.topological_sort(&heads);

        assert_eq!(order.len(), 4, "all four nodes must appear");

        let pos: HashMap<Blake3Hash, usize> =
            order.iter().enumerate().map(|(i, &id)| (id, i)).collect();

        assert!(pos[&a] < pos[&b], "A must precede B");
        assert!(pos[&a] < pos[&c], "A must precede C");
        assert!(pos[&b] < pos[&d], "B must precede D");
        assert!(pos[&c] < pos[&d], "C must precede D");
    }

    #[test]
    fn test_ancestors() {
        let (g, a, b, c, d) = diamond();

        let anc = g.ancestors(&HashSet::from([d]));
        assert_eq!(anc, HashSet::from([a, b, c, d]));

        let anc_b = g.ancestors(&HashSet::from([b]));
        assert_eq!(anc_b, HashSet::from([a, b]));
    }

    #[test]
    fn test_merge_base() {
        let (g, a, _b, _c, d) = diamond();

        // E is an independent branch off A.
        let e = make_change(HashSet::from([a]), "e");
        let eid = e.id;
        let mut g = g;
        g.add_change(e);

        let lca = g.merge_base(&HashSet::from([d]), &HashSet::from([eid]));
        assert_eq!(lca, HashSet::from([a]), "LCA of D and E must be A");
    }

    #[test]
    fn test_merge_base_no_common_ancestor() {
        let mut g = ChangeGraph::new();
        let x = make_change(HashSet::new(), "x");
        let y = make_change(HashSet::new(), "y");
        let (xid, yid) = (x.id, y.id);
        g.add_change(x);
        g.add_change(y);

        let lca = g.merge_base(&HashSet::from([xid]), &HashSet::from([yid]));
        assert!(lca.is_empty(), "disjoint graphs have no common ancestor");
    }

    #[test]
    fn test_merge_base_multiple_lcas() {
        //   A   B        (two independent roots)
        //    \ / \
        //     C   D      C depends on {A,B}, D depends on {A,B}
        //
        // ancestors(C) = {A,B,C}, ancestors(D) = {A,B,D} → common = {A,B}
        // Neither A nor B is an ancestor of the other → LCA = {A,B}
        let mut g = ChangeGraph::new();
        let a = make_change(HashSet::new(), "a");
        let b = make_change(HashSet::new(), "b");
        let c = make_change(HashSet::from([a.id, b.id]), "c");
        let d = make_change(HashSet::from([a.id, b.id]), "d");
        let (aid, bid, cid, did) = (a.id, b.id, c.id, d.id);
        g.add_change(a);
        g.add_change(b);
        g.add_change(c);
        g.add_change(d);

        let lca = g.merge_base(&HashSet::from([cid]), &HashSet::from([did]));
        assert_eq!(lca, HashSet::from([aid, bid]), "both A and B are LCAs");
    }
}
