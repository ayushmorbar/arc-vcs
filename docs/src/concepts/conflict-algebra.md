---
title: "Conflict Algebra in Arc"
description: "Conceptual model of non-commuting changes and explicit conflict state."
---

# Conflict Algebra in Arc

## BLUF

Arc conflict handling is algebraic: conflicts are explicit graph state (`Atom::Conflict`) produced when changes do not commute. This avoids reducing semantic disagreement to fragile text-marker history.

---

## Commutativity First

Merge attempts evaluate whether change pairs commute over structural intent.

- If commuting: merge can advance normally.
- If non-commuting: conflict state is persisted, not discarded.

This is why arc can preserve conflict meaning across further operations.

---

## Persistent Conflict State

Arc writes pending conflict context into `.arc/conflict` as typed data (`PendingConflict`).

That state includes:

- current view
- target heads
- conflicting change pairs

Resolution engines operate against this persisted state and produce explicit resolved atoms.

---

## Resolution Is Two-Phase by Design

1. Resolve: produce staged Ghost Node (`PendingAiChange`) with resolved inserts.
2. Approve: `arc ai approve` cryptographically commits and advances the view.

This separation preserves human control and auditability.

---

## External Merge Tool Role

Merge tools are optional resolution producers, not authority over history finalization.

- Merge-tool output is validated and staged as pending AI change.
- Approval remains mandatory.

---

## See Also

- [Conflict Resolution Protocol](../reference/conflicts.md)
- [Resolve Conflicts with AI or Merge Tool](../howto/resolve-conflicts-with-ai-or-merge-tool.md)
