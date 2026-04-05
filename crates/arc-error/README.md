# arc-error

![crate](https://img.shields.io/badge/crate-arc--error-blue)
![role](https://img.shields.io/badge/role-error%20spine-6a5acd)

## BLUF

`arc-error` provides shared error wrappers and result-extension traits for consistent diagnostic framing across crates. It captures caller-location context while keeping error handling lightweight.

## Architectural Role (The DAG)

- Depends on: Rust standard error traits.
- Depended on by: compatibility and shared infrastructure layers.
- Position: utility layer for uniform error propagation.

## Purity & I/O Boundary

`arc-error` is Pure Compute / Math.

- Type-level error wrappers only.
- No disk or network side effects.

## Key Types/Exports

- `Exn<E>`
- `Frame`
- `ResultExt`

```rust
use arc_error::ResultExt;
let value: Result<u8, std::io::Error> = Ok(1);
let _ = value.or_raise(|| std::io::Error::other("ctx"))?;
# Ok::<(), arc_error::Exn<std::io::Error>>(())
```
