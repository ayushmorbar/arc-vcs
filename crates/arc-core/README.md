# arc-core

Pure kernel for the **arc** version-control system. Contains no I/O, no CLI, no language-specific logic — only the formally-verified algebra of atomic changes and the persistent data store.

## Crate layout

```
arc-core
├── algebra/          – Atom, ASTNode, NodePath, Blake3Hash, patch algebra (apply, commute)
├── ai/               – Resolver trait and MockResolver for conflict resolution
└── store/            – CAS, Change, ChangeGraph, View, Author identity
```

## Mathematical model

Every file is an **AST** represented as a Merkle DAG whose leaves are [`Atom`](src/algebra/mod.rs) values. A `Change` is a signed bundle of atoms. Two changes *commute* when their atom-sets are disjoint or semantics-preserving (see [`commute.rs`](src/algebra/commute.rs)).

## Usage

```toml
[dependencies]
arc-core = { path = "../arc-core" }
```

```rust
use arc_core::store::view::View;
use arc_core::algebra::Blake3Hash;
```
