# arc-core

Core algebra and storage foundation for arc.

## Responsibilities

- Defines semantic change atoms and materialization model.
- Stores and loads content-addressed changes and blobs.
- Maintains DAG graph, views, operation log, tags, and author identity.
- Provides revset parsing/compilation and core error types.

## Module Areas

- `algebra`: atom model, apply/commute/inverse logic.
- `store`: CAS, change graph, view/tag/oplog/author primitives.
- `revset`: query language parser and execution engine.
- `engine`: core engine scaffolding.
- `ai`: core AI-facing interfaces and local vector utilities.

## Invariants

- Change identity is deterministic from serialized semantic content.
- Storage operations are content-addressed and hash-verified.
- Graph operations preserve DAG constraints.
- Core crate remains independent from CLI/network/editor layers.

## Usage

```toml
[dependencies]
arc-core = { path = "../arc-core" }
```
