# arc-algebra

![crate](https://img.shields.io/badge/crate-arc--algebra-blue)
![role](https://img.shields.io/badge/role-pure%20math-6a5acd)

## BLUF

`arc-algebra` is the pure patch-theory engine for replay, commutation, inversion, and sparse-aware application. It defines deterministic semantics for applying typed atoms over materialized state.

## Architectural Role (The DAG)

- Depends on: `arc-algebra-types`, `arc-change`, `arc-store-types`.
- Depended on by: `arc-engine`, `arc-cli`, `arc-daemon`, and compatibility facades.
- Position: semantic kernel below orchestration, above type-level models.

## Purity & I/O Boundary

`arc-algebra` is Pure Compute / Math.

- No filesystem I/O.
- No network I/O.
- Reads blobs only through the injected `BlobStore` trait boundary.

## Key Types/Exports

- `apply::{apply_change, apply_change_scoped, MaterializedState, BlameState}`
- `commute::{commutes, commute_pair}`
- `inverse::{invert_atom, invert_change}`
- `sparse::SparseMatcher`

```rust
use arc_algebra::sparse::SparseMatcher;
let matcher = SparseMatcher::from_patterns(&["src".to_string()]);
assert!(matcher.matches_file_path("src/lib.rs"));
```
