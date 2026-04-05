# arc-algebra-types

![crate](https://img.shields.io/badge/crate-arc--algebra--types-blue)
![role](https://img.shields.io/badge/role-type%20spine-6a5acd)

## BLUF

`arc-algebra-types` defines the canonical atom vocabulary and hash/path primitives used across the workspace. It is the schema-level contract for semantic operations.

## Architectural Role (The DAG)

- Depends on: serialization support only.
- Depended on by: semantic, storage, transport, and orchestration crates.
- Position: foundational type layer at the bottom of the dependency DAG.

## Purity & I/O Boundary

`arc-algebra-types` is Pure Compute / Math.

- Data model and helpers only.
- No disk or network side effects.

## Key Types/Exports

- `Atom`
- `Blake3Hash`
- `NodePath`

```rust
use arc_algebra_types::Atom;
let atom = Atom::Directory { path: vec!["dir".into(), "src".into()] };
assert_eq!(atom.paths().len(), 1);
```
