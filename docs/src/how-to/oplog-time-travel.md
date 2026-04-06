---
title: Oplog Time Travel
description: Documentation page for Oplog Time Travel.
---

# Time-Travel With Operation Log

## BLUF

Use `arc op log` to find an operation, then:

- `arc op restore <op-id>` to move your current view to the state after that operation.
- `arc op revert <op-id>` to undo that operation by restoring its pre-operation heads.

This lets you recover from bad merges, mistaken rewrites, or accidental view moves without guessing commit ranges.

---

## When To Use This

Use operation time-travel when your problem is about repository state transitions, not only a single code change.

Common cases:

- You ran a command that moved heads in an unexpected way.
- A history edit (rewrite/squash/amend) produced the wrong result.
- You want to return to a known-good operational checkpoint quickly.

---

## Step 1: Inspect Operations

```bash
arc op log
```

`arc op log` shows operations in reverse chronological order with:

- operation id (displayed from snapshot prefix when available),
- timestamp,
- view,
- agent (human/ai),
- command,
- before/after head summaries.

Pick the operation you want to target.

---

## Step 2A: Restore To After An Operation

```bash
arc op restore <op-id>
```

`restore` replays view state to the operation's post-state (`before -> after` as recorded at that time).

Use this when:

- you want to re-enter a previously known state,
- the selected operation represents the exact point you want active now.

Example:

```bash
arc op restore 4f3c9a21b6d0
```

---

## Step 2B: Revert A Specific Operation

```bash
arc op revert <op-id>
```

`revert` inverts the selected operation at the operation-log level by restoring its pre-operation heads.

Use this when:

- one specific operation was wrong,
- you want to negate that operation's effect.

Example:

```bash
arc op revert 4f3c9a21b6d0
```

---

## Choosing IDs Safely

- You can pass a short operation id or a snapshot-derived prefix shown by `arc op log`.
- If a prefix matches multiple operations, arc reports an ambiguity error.
- Prefer copying the displayed id directly from `arc op log`.

---

## Recovery Workflow Example

```bash
# 1) Inspect recent operations
arc op log

# 2) Move back to a known-good post-operation state
arc op restore 9d27c4b14e8f

# 3) If you instead need to negate a problematic operation
arc op revert 5ab1d0e7c3aa

# 4) Validate final state
arc status
arc log -r "ancestors(@)"
```

---

## Guarantees And Safety Notes

- Operation replay is crash-consistency aware: failures during replay avoid leaving view/worktree in a partial state.
- `restore` and `revert` operate on operation boundaries, complementing change-level commands like `arc revert <change>`.
- If you only need to revert code content from one change, prefer `arc revert`.

---

## Related Commands

- `arc undo`: roll back the latest view-mutating operation.
- `arc log -r <revset>`: inspect change graph state after time-travel.
- `arc verify --workspace-policy`: validate workspace policy after recovery in regulated repos.
