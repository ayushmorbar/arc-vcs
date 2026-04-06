# arc Crate Status

This document provides a transparent capability matrix for arc workspace crates.

Legend:
- `implemented`: available and used in regular flows
- `partial`: available but incomplete or with known limitations
- `planned`: intended but not yet delivered

## Primary 14-Crate Capability Matrix

| Crate | Role | Status | Notes |
|---|---|---|---|
| `arc-algebra-types` | core domain types | implemented | canonical algebra-level data contracts |
| `arc-store-types` | storage domain types | implemented | shared persistence-level type contracts |
| `arc-change` | change representation | implemented | typed change model for replay and merge |
| `arc-algebra` | pure algebra semantics | implemented | no boundary I/O allowed |
| `arc-engine` | replay/merge engine | partial | correctness-first; high-conflict memory work ongoing |
| `arc-revset` | query language for revision sets | partial | expanding coverage and explainability |
| `arc-store-cas` | content-addressed storage | implemented | deterministic storage contract |
| `arc-store-graph` | DAG persistence and traversal | implemented | core causal graph persistence surface |
| `arc-store-view` | view/snapshot persistence | implemented | active surface for branch/view materialization |
| `arc-network` | transport payload semantics | partial | protocol hardening continues |
| `arc-lang` | language/AST plugin boundary | partial | language coverage grows iteratively |
| `arc-cli` | user-facing command surface | implemented | primary operator UX |
| `arc-daemon` | editor/agent daemon surface | partial | orchestration-only boundary policy |
| `arc-git-bridge` | interoperability boundary | partial | compatibility and edge-case parity track |

## Supporting Crates (Workspace)

Supporting crates such as `arc-core`, `arc-net`, `arc-error`, `arc-policy`,
`arc-store-policy`, `arc-transaction`, `arc-testtools`, and others are
tracked under the same status model and promoted into the primary matrix when
their external contract significance increases.

## Maintenance Rules

1. Update this file when a crate changes status.
2. Link major status shifts to ADRs when architectural scope changes.
3. Keep status claims aligned with tests and release notes.
