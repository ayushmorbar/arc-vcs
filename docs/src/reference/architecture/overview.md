---
title: "Architecture Overview"
description: "Reference map of ADR-004 micro-crate boundaries, purity contracts, and runtime data flow."
---

# Architecture Overview

Bottom line up front: arc manages complexity by separating pure semantics from side effects. Algebra and identity logic stay deterministic; persistence and transport execute effects in narrow, auditable slices.

## ADR-004 Slice Map

| Slice            | Crates                                                          | Responsibilities                                                |
| ---------------- | --------------------------------------------------------------- | --------------------------------------------------------------- |
| Domain types     | `arc-algebra-types`, `arc-store-types`, `arc-change`            | Atoms, IDs, authors, refs, change structure                     |
| Pure semantics   | `arc-algebra`, `arc-engine`, `arc-revset`                       | Replay algebra, rewrite math, revset parse/compile              |
| Persistence      | `arc-store-cas`, `arc-store-graph`, `arc-store-view`            | CAS storage, DAG state, view pointers, operation log            |
| Transport        | `arc-network`, `arc-net`                                        | Payload protocol, sync ingress/egress                           |
| Product surfaces | `arc-cli`, `arc-daemon`, `arc-git-bridge`, `arc-lang`, `arc-ai` | UX orchestration, IDE bridge, Git boundary, AST and AI adapters |
| Compatibility    | migration facade                                                | Transitional shim while consumers finish direct micro-crate adoption |

## Dependency Rules

- Pure semantic crates must not perform filesystem or network I/O.
- Persistence and transport crates are side-effect boundaries.
- CLI and daemon orchestrate workflows and must not duplicate lower-layer semantics.
- Git translation remains a boundary concern in `arc-git-bridge`.

## Data Flow

1. A command enters through `arc-cli` or `arc-daemon`.
2. Semantic operations execute through algebra, engine, and graph slices.
3. State persists through CAS plus view/oplog slices.
4. Sync transports typed payloads and verifies signatures before writes.
5. Git interop is generated on demand at the bridge boundary.

## Purity And Determinism

The architecture relies on a simple invariant: replay math must be deterministic and side-effect free. This keeps reasoning local and test surfaces small.

> **Note:** If a crate can touch disk or network, it is an infrastructure slice and must not redefine semantic rules.

## Crash-Consistency Model

- Mutable pointers are written with atomic rename patterns.
- Operation sequencing uses append-only logging.
- Durability boundaries are explicit and code-reviewed.

## Related Pages

- [../design/patch_theory.md](../design/patch_theory.md)
- [../design/history_rewriting.md](../design/history_rewriting.md)
- [../design/network_transport.md](../design/network_transport.md)
- [../reference/cli-reference.md](../reference/cli-reference.md)
