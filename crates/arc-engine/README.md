# arc-engine

![crate](https://img.shields.io/badge/crate-arc--engine-blue)
![role](https://img.shields.io/badge/role-rewrite%20orchestrator-6a5acd)

## BLUF

`arc-engine` orchestrates high-level history rewrites on top of pure algebra and DAG topology. It powers operations like squash and mutation planning while preserving causal correctness.

## Architectural Role (The DAG)

- Depends on: `arc-algebra`, `arc-change`, `arc-store-graph`, `arc-store-view`, `arc-store-types`.
- Depended on by: `arc-cli` and compatibility facades.
- Position: rewrite coordination layer between pure algebra and user-facing orchestration.

## Purity & I/O Boundary

`arc-engine` is Pure Compute / Math.

- No direct filesystem or network side effects.
- Consumes graph and change models as inputs and emits rewritten state plans.

## Key Types/Exports

- `spacetime::squash_into`
- `mutator` module rewrite helpers

```rust
use arc_engine::spacetime;
let _ = std::any::type_name_of_val(&spacetime::squash_into);
```
