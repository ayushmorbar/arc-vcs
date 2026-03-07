# Spacetime Operation Log

The **spacetime operation log** (`arc op log`) is an append-only ledger that
records every view-mutating operation together with the full DAG frontier
**before** and **after** the mutation.  It is the substrate for `arc undo` and
for future AI attribution, audit, and Delta-Impact Guard features.

---

## Design Goals

| Goal | Mechanism |
|---|---|
| O(1) undo | Store `before_heads`; restore is a single pointer write |
| Full auditability | Every mutation records who (Human / AI) did what and when |
| Bounded storage | Sliding-window compaction at **1 000 entries** |
| Backward compat | `#[serde(alias = "previous_heads")]` — old `oplog.json` files are still readable |
| Local-only | Never synced over the network; one log per working copy |

---

## Data Model

```
Operation {
    id:           String          // 8-char BLAKE3(timestamp_le ‖ command)
    timestamp:    u64             // Unix seconds
    command:      String          // "snap" | "merge" | "cherry-pick" | …
    view:         String          // name of the mutated view
    agent:        OperationAgent  // Human (default) | Ai
    before_heads: HashSet<Blake3Hash>
    after_heads:  HashSet<Blake3Hash>
}
```

All operations are serialized as a JSON array in `.arc/oplog.json`.

---

## O(1) Undo

`arc undo` performs a **pure pointer-swap**:

1. Pop the most-recent `Operation` from the log.
2. Write `View { heads: op.before_heads }` to `.arc/views/<view>`.
3. Rematerialize the working directory from `op.before_heads`.

No `Change` objects are deleted from the CAS.  The operation is
**reversible** (re-snap what you had) and **safe** (nothing is rewritten).

This is identical in spirit to [Jujutsu's](https://github.com/martinvonz/jj)
operation log, which inspired this design.

---

## Sliding-Window Compaction

When the log reaches **1 000 entries**, `OpLog::append()` evicts the oldest
entry before writing the new one, keeping the file size constant.  The window
can be tuned by changing `MAX_ENTRIES` in `arc-core/src/store/oplog.rs`.

---

## Agent Attribution

Every `Operation` carries an `OperationAgent` field:

| Variant | Label | Usage |
|---|---|---|
| `Human` (default) | 👤 Human | Interactive `arc snap`, `arc merge`, etc. |
| `Ai` | 🤖 AI | Automated conflict resolution, AI-generated snaps |

`OperationAgent::Ai` is set via `Operation::new_with_agent()` in code paths
driven by the `AiResolver` trait.

---

## Local-Only Semantics

The oplog is **never** included in `arc push` / `arc pull` payloads.  Each
working copy maintains its own independent history of local operations.
Sharing operation history across machines is a deliberate non-goal to keep
sync semantics simple and to prevent privacy leaks on shared repositories.

---

## Future Work

- **Phase 37 — Delta-Impact Guard:** Before any AI-driven mutation, the oplog
  can be inspected to surface the blast radius of the proposed change.
- **`arc op undo <id>`:** Jump to an arbitrary point in the log, not just the
  most-recent entry.
- **`arc op diff`:** Show the working-directory delta between two operation IDs.
