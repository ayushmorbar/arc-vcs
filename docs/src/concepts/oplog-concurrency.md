---
title: "OpLog and Optimistic Concurrency"
description: "How arc uses an append-only operation ledger and optimistic replay to keep multi-actor workflows safe."
---

# OpLog and Optimistic Concurrency

## BLUF

arc treats repository mutations as operations in an append-only OpLog. This yields deterministic recovery boundaries and safer concurrent workflows through operation replay.

> **Note:** Operation-level recovery is mathematically distinct from change-level editing: operation replay moves repository state across time, while change commands mutate the DAG frontier.

---

## Core Model

| Layer | Primitive | Responsibility |
| --- | --- | --- |
| Change layer | `ChangeId`, `Atom::{Insert,Delete,Conflict}` | Semantic content evolution |
| Snapshot layer | `SnapshotId` | Typed evidence for operation boundaries |
| Operation layer | `OpLog` entries | Ordered state transitions (`before` -> `after`) |
| Projection layer | `MaterializedState` | Deterministic worktree materialization |

This separation keeps concurrency errors local and recoverable.

---

## Why Optimistic Concurrency Works Here

1. Operations append intent and boundary state.
2. Replay uses recorded heads rather than guessing from current filesystem state.
3. `arc op restore` and `arc op revert` target stable operation ids, while `arc undo` rolls back the latest view-mutating operation.

Expected workflow:

```bash
arc op log
```

```text
ID           TIME                VIEW    CMD        BEFORE -> AFTER
9d27c4b14e8f 2026-04-05T10:12Z   main    snap       a1b2c3 -> 8f7e6d
71ac91ff22d0 2026-04-05T10:09Z   main    merge      91aa20 -> a1b2c3
```

---

## Failure Boundary

If an operation replay cannot complete safely, arc keeps the repository out of partial-transition states.

> **Note:** In regulated environments, run `arc verify --workspace-policy` after restoration to validate root policy invariants.

---

## See Also

- [Time-Travel With Operation Log](../how-to/oplog-time-travel.md)
- [Conflict Algebra in Arc](conflict-algebra.md)

