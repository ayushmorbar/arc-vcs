---
title: ADR 002 Jujutsu Workflow
description: Documentation page for ADR 002 Jujutsu Workflow.
---

# ADR 002 - Jujutsu-Style Workflow as the Default UX

| Field        | Value         |
| ------------ | ------------- |
| **Status**   | Accepted      |
| **Date**     | 2026-04-04    |
| **Deciders** | arc core team |

---

## Context

Git-era workflows rely on a staging area (`add`) and local stack-manipulation conventions that are powerful but cognitively heavy:

- Users continuously shuttle state between working tree, index, and commit graph.
- Undo semantics are distributed across multiple commands with different guarantees.
- Conflicts are represented as text marker side effects rather than first-class graph objects.

arc targets a developer experience where history editing is continuous, safe, and algebraic by default.

---

## Decision

arc adopts a **Jujutsu-inspired interaction model** with three core decisions:

1. **No staging area as a required step**
   - The working copy is automatically tracked as an amendable head state.
   - Users run `arc snap` to finalize intent; no `arc add` equivalent is required.

2. **Global undo backed by OpLog**
   - Every view-mutating operation records before/after heads in the operation log.
   - `arc undo` restores prior frontier state in O(1) pointer-swap semantics.

3. **First-class conflicts in the algebra**
   - Conflicts are represented explicitly as `Atom::Conflict` instead of being only text markers.
   - Resolution is a typed graph transition, not an opaque file mutation.

---

## Consequences

**Positive:**

- Reduced cognitive overhead in day-to-day iteration (edit, snap, continue).
- Safer experimentation via universal undo semantics.
- Conflict handling is explicit and machine-processable.
- Better fit for AI-assisted workflows where semantic state must be inspectable.

**Negative:**

- Users migrating from Git must unlearn index/staging habits.
- Tooling and docs must clearly explain implicit auto-amend behavior.
- Operation log durability becomes critical infrastructure.

---

## Alternatives Considered

- Preserve a mandatory index/staging layer (Git parity).
- Keep undo as ad-hoc command-specific logic.
- Keep conflicts as file-system marker artifacts only.

These were rejected because they preserve legacy complexity and reduce formal guarantees.

---

## References

- [Spacetime Operation Log](oplog.md)
- [History Rewriting](history_rewriting.md)
- [Migrating from Git](../getting-started/git-migration.md)
