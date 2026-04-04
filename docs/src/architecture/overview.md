# Architecture Overview

This page is the implementation-level map of arc's crates and data flow.

## Workspace Crates

| Crate            | Layer         | Responsibility                                                |
| ---------------- | ------------- | ------------------------------------------------------------- |
| `arc-core`       | Foundation    | Atom algebra, CAS, DAG graph, views, author identity, revsets |
| `arc-lang`       | Language      | Tree-sitter-backed AST plug-ins and projection helpers        |
| `arc-net`        | Network       | HTTP endpoints, sync protocol, AI provider integration        |
| `arc-git-bridge` | Interop       | Git object translation and Smart HTTP push boundary           |
| `arc-cli`        | Orchestration | Command execution, repository workflows, user-facing behavior |
| `arc-daemon`     | IDE           | Long-lived JSON-RPC bridge for editor tooling                 |

## Dependency Rules

- `arc-core` is lowest-level and independent.
- `arc-lang`, `arc-net`, and `arc-git-bridge` depend on `arc-core`.
- `arc-cli` composes core, language, network, and Git bridge crates.
- `arc-daemon` depends on `arc-cli` and `arc-core` only.

## Data Flow

1. User runs a command (`arc snap`, `arc pull`, `arc merge`, and others).
2. `arc-cli` calls into `arc-core` to evaluate DAG/state transitions.
3. Language-aware transforms route through `arc-lang` as needed.
4. Remote synchronization uses `arc-net` or, for Git remotes, `arc-git-bridge`.
5. IDE clients consume status via `arc-daemon` JSON-RPC.

## Storage Model

- Changes are content-addressed via BLAKE3.
- Views hold named head sets over the change DAG.
- Blob payloads are stored separately from typed atom metadata.
- Materialization projects semantic state back into working files.

## Interop Boundaries

- Native arc-to-arc sync remains arc-typed end to end.
- Git interoperability is done just-in-time at push boundary.
- No local `.git` object store is required for normal arc operation.

## Related Pages

- [Patch Theory](../design/patch_theory.md)
- [AST Diffing](../design/ast_diffing.md)
- [Network Transport](../design/network_transport.md)
- [CLI Reference](../reference/cli-reference.md)
