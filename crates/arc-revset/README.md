# arc-revset

![crate](https://img.shields.io/badge/crate-arc--revset-blue)
![role](https://img.shields.io/badge/role-query%20engine-6a5acd)

## BLUF

`arc-revset` is the DAG query language engine for arc. It parses revset expressions and lazily evaluates them against change graphs and reference resolvers.

## Architectural Role (The DAG)

- Depends on: `arc-change`, `arc-store-graph`, parser libraries.
- Depended on by: `arc-cli` and compatibility facades.
- Position: query planning/evaluation layer above graph storage and below command orchestration.

## Purity & I/O Boundary

`arc-revset` is Pure Compute / Math.

- Parser and evaluator only.
- No filesystem or network side effects.

## Key Types/Exports

- `parse`
- `compile`, `compile_change_ids_with_refs`
- `RevsetExpression`

```rust
let expr = arc_revset::parse("ancestors(@)")?;
let _ = format!("{expr:?}");
# Ok::<(), String>(())
```
