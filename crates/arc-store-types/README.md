# arc-store-types

![crate](https://img.shields.io/badge/crate-arc--store--types-blue)
![role](https://img.shields.io/badge/role-identity%20spine-6a5acd)

## BLUF

`arc-store-types` is the shared type spine for IDs, authorship, refs, and tags. It keeps provenance and storage contracts uniform across all layers.

## Architectural Role (The DAG)

- Depends on: core serialization/crypto primitives.
- Depended on by: nearly all storage, algebra, network, and orchestration crates.
- Position: foundational identity/reference model directly above basic atom/hash types.

## Purity & I/O Boundary

`arc-store-types` is mostly Pure Compute / Math with local helper I/O.

- Core exports are type and validation contracts.
- Includes local identity/ref loading helpers.
- No network side effects.

## Key Types/Exports

- `author::{Author, load_identity, save_identity}`
- `newtypes::{ChangeId, SnapshotId, MutationId}`
- `refs` helper APIs
- `tag::Tag`

```rust
use arc_store_types::newtypes::ChangeId;
let id = ChangeId::from_hex("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")?;
assert_eq!(id.to_hex().len(), 64);
# Ok::<(), anyhow::Error>(())
```
