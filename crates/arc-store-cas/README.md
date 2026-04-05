# arc-store-cas

![crate](https://img.shields.io/badge/crate-arc--store--cas-blue)
![role](https://img.shields.io/badge/role-disk%20boundary-f6a)

## BLUF

`arc-store-cas` is the local content-addressed storage boundary for arc objects and blobs. It owns on-disk layout, BLAKE3-addressed read/write paths, and mmap-backed blob reads.

## Architectural Role (The DAG)

- Depends on: `arc-store-types`, `arc-algebra-types`, local filesystem primitives.
- Depended on by: `arc-cli`, `arc-lang`, `arc-net`, `arc-engine`, and compatibility facades.
- Position: primary persistence boundary for immutable object/blob payloads.

## Purity & I/O Boundary

`arc-store-cas` is an I/O Boundary.

- Reads/writes `.arc/store` and `.arc/blobs`.
- Uses memory-mapped reads for efficient blob access.
- No network side effects.

## Key Types/Exports

- `ObjectStore`
- `CasBytes`
- `blake3_hasher` utilities

```rust
let store = arc_store_cas::ObjectStore::new(".");
let h = store.write_blob(b"hello")?;
let _bytes = store.read_blob(&h)?;
# Ok::<(), arc_store_cas::CasError>(())
```
