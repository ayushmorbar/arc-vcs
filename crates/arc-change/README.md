# arc-change

![crate](https://img.shields.io/badge/crate-arc--change-blue)
![role](https://img.shields.io/badge/role-provenance%20envelope-6a5acd)

## BLUF

`arc-change` defines signed immutable change envelopes and deterministic content hashing. It is the provenance-critical unit connecting atom streams, causal dependencies, and Ed25519 signatures.

## Architectural Role (The DAG)

- Depends on: `arc-algebra-types`, `arc-store-types`, content-hash derive support.
- Depended on by: algebra, graph, network, and orchestration crates.
- Position: shared signed payload model above type crates and below all DAG logic.

## Purity & I/O Boundary

`arc-change` is Pure Compute / Math.

- Deterministic hashing and signature verification only.
- No filesystem or network I/O.

## Key Types/Exports

- `Change`
- `Change::new`, `Change::new_canonical_from_seed`
- `Change::verify_signature`
- `ContentHash` derive/traits

```rust
use std::collections::HashSet;
use arc_change::Change;
# use arc_algebra_types::Atom;
# use arc_store_types::author::test_keypair;
# let (a,k)=test_keypair();
let c = Change::new(HashSet::new(), vec![Atom::Directory{path:vec!["dir".into()]}], "init", a, &k);
assert!(c.verify_signature());
```
