---
name: arc-causal-graph
description: >
  Rules for causal histories, DAG frontiers, vector clocks, and CRDT 
  state convergence in arc-vcs. Use when implementing synchronization, 
  merging concurrent changes, tracking repository state, or traversing 
  the intent graph.
---

# arc-causal-graph

## Purpose
`arc` does not have "branches" and "commits" in the traditional sense; it has a Spacetime DAG of semantic changes. State is determined by causal history and frontiers.

## Core Time Rules

### 1. The Frontier
- The repository state is defined by its `Frontier` (the set of DAG leaf nodes), not a single `HEAD` pointer.
- When a new operation is applied, it must explicitly list the `Frontier` nodes it observed as its causal parents.

### 2. Concurrent Resolution (CRDT)
- Never implement standard Git 3-way merges (e.g., LCA diffing).
- When two changes are concurrent (neither is an ancestor of the other), their AST operations must be merged using CRDT commutativity laws (see `arc-patch-theory`).
- If operations mathematically conflict (e.g., both explicitly modify the exact same AST node incompatibly), emit a `Conflictor` intent node. Do not insert inline `<<<<<<< HEAD` text conflict markers into the user's code.

### 3. Vector Clocks & Logical Time
- Wall-clock time (`std::time::SystemTime`) is for telemetry and UI only.
- Core ordering and causality must be determined purely by topological sorting of the DAG and logical vector clocks/Lamport timestamps.

## Graph Traversal
- Traversing the graph should be done lazily using iterators. Do not load the entire DAG into memory.
- Use topological sorts (Kahn's algorithm) when flattening the graph for replay, ensuring dependencies are always yielded before the nodes that depend on them.