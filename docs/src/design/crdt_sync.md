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
