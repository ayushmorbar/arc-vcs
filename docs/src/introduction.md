---
title: "Introduction"
description: "BLUF orientation to arc's semantic VCS model, ADR-004 architecture, and where to start in the docs."
---

# arc: Atomic Replayable Changes

Bottom line up front: arc is a semantic VCS built to control complexity under concurrent change. It records typed changes over a DAG, verifies provenance cryptographically, and isolates side effects to explicit storage and transport boundaries.

## Mental Model

1. You edit files.
2. `arc snap` computes semantic deltas and records signed `Change` objects.
3. Change objects and blobs are content-addressed by BLAKE3.
4. Views point to named head sets in the change graph.
5. Merge and rewrite operations execute algebraic rules, not line heuristics.

## Architecture Snapshot

ADR-004 decomposed the old monolithic core into micro-crate slices:

- Domain types: `arc-algebra-types`, `arc-store-types`, `arc-change`
- Pure semantics: `arc-algebra`, `arc-engine`, `arc-revset`
- Persistence: `arc-store-cas`, `arc-store-graph`, `arc-store-view`
- Transport: `arc-network`, `arc-net`
- Product surfaces: `arc-cli`, `arc-daemon`, `arc-git-bridge`, `arc-lang`, `arc-ai`

> **Note:** `arc-core` is now a compatibility facade during migration, not the long-term architecture center.

## Purity And Crash Consistency

- Semantic crates are side-effect free by contract.
- Disk and network effects are isolated to dedicated crates.
- Crash consistency is enforced with atomic rename update paths and append-only operation logging.

## What Is Code-Verified Today

- BLAKE3 content addressing across changes and blobs.
- Ed25519-backed author provenance and zero-trust ingress checks in sync paths.
- DAG graph and view pointer model with rewrite-aware workflows.
- Rust AST-aware plugin flow via `arc-lang`.
- CLI and daemon orchestration surfaces on top of split crates.

## Start Here

- New users: [getting-started/tutorial.md](getting-started/tutorial.md)
- Daily workflows: [getting-started/everyday.md](getting-started/everyday.md)
- Commands and flags: [reference/cli-reference.md](reference/cli-reference.md)
- System architecture: [architecture/overview.md](architecture/overview.md)

## Scope Truth

Workspace build truth is `crates/*`. If behavior is not code-verified, it must be documented as a limitation, not presented as shipped capability.
