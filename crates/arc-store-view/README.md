# arc-store-view

![crate](https://img.shields.io/badge/crate-arc--store--view-blue)
![role](https://img.shields.io/badge/role-crash%20consistent%20state-f6a)

## BLUF

`arc-store-view` owns crash-consistent persistence for mutable pointers and operation history. It is where views, oplog entries, synthesis snapshots, and tempfile tracking are durably written.

## Architectural Role (The DAG)

- Depends on: `arc-store-graph`, `arc-store-types`, and filesystem primitives.
- Depended on by: `arc-cli`, `arc-daemon`, `arc-engine`, `arc-net`, and compatibility facades.
- Position: mutable-state persistence boundary above CAS and graph models.

## Purity & I/O Boundary

`arc-store-view` is an I/O Boundary.

- Performs atomic rename write paths for mutable state.
- Maintains append-only operation log storage.
- No network side effects.

## Key Types/Exports

- `view::View`
- `oplog::{OpLog, Operation, RewriteTransaction}`
- `synthesis` snapshot helpers
- `tempfile` signal-safe tracking helpers

```rust
use std::collections::HashSet;
let view = arc_store_view::View::new("main", HashSet::new());
# let _ = view;
```
