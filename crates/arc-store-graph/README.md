# arc-store-graph

![crate](https://img.shields.io/badge/crate-arc--store--graph-blue)
![role](https://img.shields.io/badge/role-dag%20topology-6a5acd)

## BLUF

`arc-store-graph` owns in-memory DAG topology logic for arc. It provides ancestry traversal, topological ordering, and bisect state machinery over signed changes.

## Architectural Role (The DAG)

- Depends on: `arc-change`, `arc-store-types`.
- Depended on by: `arc-engine`, `arc-revset`, `arc-cli`, `arc-store-view`.
- Position: graph semantics layer between change payloads and rewrite/query/orchestration crates.

## Purity & I/O Boundary

`arc-store-graph` is Pure Compute / Math.

- In-memory traversal and bisect logic.
- No filesystem or network side effects in core graph APIs.

## Key Types/Exports

- `ChangeGraph`
- `graph` traversal APIs
- `bisect::{BisectEngine, BisectState, BisectMark}`

```rust
use arc_store_graph::ChangeGraph;
let g = ChangeGraph::new();
assert!(g.is_empty());
```
