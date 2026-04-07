---
title: Crdt Sync
description: Documentation page for Crdt Sync.
---

# CRDT Network Sync

This document describes arc's network synchronisation model: how views and changes are distributed across nodes, how the merge lattice is defined, and how causal stability enables safe garbage collection.

---

## The Sync Model: Views as a Join-Semilattice

A `View` is a `HashSet<Blake3Hash>` of head `Change` IDs. The set of all views across all nodes forms a **join-semilattice**: the join operation is set union, and the partial order is subset inclusion.

Formally, for two nodes $N_1$ and $N_2$ each holding view $V$:

$$V_{N_1} \sqcup V_{N_2} = V_{N_1} \cup V_{N_2}$$

$$V_{N_1} \leq V_{N_2} \iff V_{N_1} \subseteq V_{N_2}$$

This gives arc its CRDT (Conflict-free Replicated Data Type) property: **merging is idempotent, commutative, and associative**. You can sync nodes in any order and the result is always the same.

---

## The CAS as a Distributed Store

Every `Change` is uniquely identified by its BLAKE3 hash over its full content (atoms, parents, author, message, signature, timestamp). This means:

- The same `Change` has the same ID on every node that holds it.
- Syncing is purely additive: nodes exchange objects they don't have yet.
- There is no "push conflict" at the object level — the only conflicts are semantic ones detected by `commutes()`.

---

## Fetch, Pull, Push

`arc-net` exposes a read-only HTTP server with two endpoints:

- `GET /cas/:hash` — fetch a single CAS object by BLAKE3 hash
- `GET /view/:name` — fetch the current head set for a named view

**`arc fetch <remote>`** downloads all CAS objects reachable from the remote's views that the local node doesn't already have.

**`arc pull <remote> <view>`** fetches and then calls `merge_heads()` to integrate the remote view into the local one. The full commutativity check runs as part of the merge.

**`arc push <remote> <view>`** POSTs local objects and view heads to the remote. The remote performs its own commutativity check on merge.

---

## Causal Stability

A `Change` $c$ is **causally stable** with respect to a set of views $\mathcal{V}$ when every view in $\mathcal{V}$ has $c$ in its causal history:

$$\text{stable}(c, \mathcal{V}) \iff \forall V \in \mathcal{V}: c \in \text{ancestors}(V.\text{heads})$$

Causally stable changes are safe to garbage-collect: no future merge can produce a state that requires them. Pruning a stable change cannot affect any reachable materialisation.

---

## Garbage Collection (`arc gc`)

`arc gc` implements causal-stability GC:

1. Enumerate all local views and their head sets.
2. Compute the full ancestor set for all heads.
3. Find the set of changes that are ancestors of **every** view (the causal frontier).
4. Prune CAS objects for changes below the frontier that are not referenced by any live view or tag.
5. Report `GcResult { retained: usize, pruned: usize }`.

The `OpLog` is also consulted: changes referenced by OpLog entries are never pruned, ensuring `arc undo` always has sufficient history.

---

## Incremental Sync: The CAS Advantage

Because the CAS is content-addressed, arc's sync protocol is natively incremental:

1. The receiver requests the remote's view head set.
2. It computes the set difference: which hashes does it not have locally?
3. It fetches only those objects.

There is no pack file negotiation, no delta compression, and no "thin pack" complexity. The protocol is a simple recursive object download: fetch a `Change`, find its parent hashes, fetch any parents not locally present, recurse.

---

## Split-Root Workspaces and Shared CAS

In a split-root workspace, multiple `work_root` directories share a single `shared_root` containing `.arc/`. The `WorkspaceManifest` at `.<shared_root>/.arc-workspace` records all registered work roots.

Each work root can have its own sparse checkout patterns (via `Atom::Mount` in the view), so different team members can materialize different subsets of the codebase while sharing the same change history and CAS.

---

## PO-Log Compaction & Epoch Maps

A long-lived arc repository will accumulate thousands of `Atom::Delete` tombstones and superseded `Atom::Insert` versions — the inevitable consequence of a purely grow-only CRDT history. Over time this increases hydration cost and repository size without adding semantic value. PO-Log Compaction permanently solves this problem.

### The Genesis Change

`arc compact` performs a **state-collapse**: it materialises the complete AST state at the causal-stability frontier and encodes it as a single synthetic `Change` with **empty deps**:

```
Genesis Change {
    id:    blake3(atoms + intent + author),
    deps:  [],   // <-- the algebraic root; no predecessors
    atoms: [ Insert(file/main.rs/fn_a), Insert(file/lib.rs/struct_Widget), Blob(assets/logo.png), ... ],
    intent: "Compacted Base State",
}
```

The Genesis Change is written to the CAS exactly like any other change. The old history behind it is then physically deleted from `.arc/store/`.

### The Epoch Map

Because arc uses BLAKE3 content-addressed identities, no `Change` object can be mutated without changing its ID. If a live, unstable `Change` has a dep that points into the now-deleted stable history, direct hydration would fail.

The Epoch Map resolves this by rewriting the **read path** rather than stored objects:

```
.arc/epochs  (JSON)
{
  "<old_stable_id_1>": "<genesis_id>",
  "<old_stable_id_2>": "<genesis_id>",
  ...
}
```

In `hydrate_heads()`, before each BFS CAS read:

```rust
if let Some(&genesis_id) = epoch_map.get(&id) {
    // redirect: load the Genesis Change instead
    queue.push_back(genesis_id);
    continue;
}
```

This means:
- Live `Change` objects on peer nodes that still have their `deps` pointing to old IDs continue to work correctly — their hydration is transparently redirected.
- The Epoch Map is append-only: running `compact()` multiple times composes correctly without invalidating earlier epochs.
- Peers that have not yet compacted remain fully interoperable with peers that have.

### Invariants

| Invariant | Guaranteed by |
|---|---|
| No live `Change` object is mutated | `compact()` only deletes CAS files; never rewrites them |
| BLAKE3 integrity of all active changes | IDs are computed only from immutable fields; deps pointers in live changes are untouched |
| CRDT commutativity is preserved | Genesis is a commit with empty deps; it commutes with everything |
| Epoch Map composes over multiple rounds | Map is append-only; each round adds new entries without removing old ones |
| Blob files are retained | `.arc/blobs/` is never touched by `compact()`; Genesis `Atom::Blob` atoms reference them |

### Example: 10-Year Repository

```
Before compact():  5,000,000 changes in .arc/store/  (~40 GB)
After compact():   1 Genesis Change in .arc/store/   (~2 MB)
                   .arc/blobs/ unchanged              (~10 GB)
                   .arc/epochs entries: 5,000,000
Net saving:        ~30 GB deleted; O(1) hydration cost
```

The repository remains fully functional. `arc log` on any view walks back to the Genesis Change and stops (it has no parents). `arc merge`, `arc snap`, and all CRDT sync operations continue normally.
