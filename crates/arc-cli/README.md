# arc-cli

![crate](https://img.shields.io/badge/crate-arc--cli-blue)
![role](https://img.shields.io/badge/role-orchestrator-4c8)

## BLUF

`arc-cli` is the product entrypoint and orchestration surface for arc workflows. It composes semantic, storage, transport, and interop crates into one operator-facing command experience.

## Architectural Role (The DAG)

- Depends on: `arc-ai`, `arc-algebra`, `arc-algebra-types`, `arc-change`, `arc-engine`, `arc-network`, `arc-revset`, `arc-store-cas`, `arc-store-graph`, `arc-store-types`, `arc-store-view`, `arc-lang`, `arc-net`, `arc-git`, `arc-git-bridge`.
- Depended on by: `arc-daemon`.
- Position: top-level orchestration layer; no lower crate should depend on `arc-cli`.

## Purity & I/O Boundary

`arc-cli` is an **I/O boundary**.

- Reads and writes repository working trees.
- Initiates network sync and Git bridge interactions.
- Coordinates durable state updates through storage crates.

## Key Types/Exports

- `repo::Repository`
- `sync::{fetch, pull, push}`
- `generate`, `semantic_diff`, `graph_render` module surfaces

```rust
use arc_cli::repo::Repository;

let mut repo = Repository::open(".")?;
let _ = repo.log()?;
# Ok::<(), anyhow::Error>(())
```
