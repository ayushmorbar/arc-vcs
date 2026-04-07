---
title: ADR 001 Change Algebra
description: Documentation page for ADR 001 Change Algebra.
---

# ADR 001 - Change Algebra over Snapshot Diffs

| Field        | Value         |
| ------------ | ------------- |
| **Status**   | Accepted      |
| **Date**     | 2026-04-04    |
| **Deciders** | arc core team |

---

## Context

Traditional VCS systems model history as snapshot deltas reconstructed from line-oriented text differences. That model is universal, but it creates persistent issues for software development:

- False conflicts when independent edits are line-adjacent.
- Poor semantic signal for review and automation.
- No formal way to prove two changes can safely commute.
- Weak representation of structure-preserving operations like rename, move, and extraction.

arc is designed as an AI-native VCS where changes must be machine-reasonable and cryptographically attributable. The representation layer therefore cannot be plain textual diff hunks.

---

## Decision

arc records history as **AST-aware algebraic Atoms** inside signed `Change` objects instead of line-diff snapshots.

Key properties:

- Source changes are represented as typed operations (`Insert`, `Delete`, `Move`, `SemanticsPreserving`, `Conflict`, `Blob`, `Mount`).
- The merge predicate is defined mathematically (`commutes(a, b)`), not heuristically.
- The canonical identifier of a change is a BLAKE3 hash over its semantic payload.
- Each change is signed with Ed25519, giving provenance at the same granularity as algebraic intent.

For non-source/binary material where AST plugins do not apply, arc falls back to blob atoms while preserving the same cryptographic framing.

---

## Consequences

**Positive:**

- Dramatically fewer false conflicts compared to line-based merge.
- Deterministic, auditable change semantics suitable for AI tooling.
- Algebraic history operations (merge/cherry-pick/replay) are grounded in formal commutativity.
- Security and provenance attach directly to semantic operations, not only to snapshots.

**Negative:**

- Requires language-aware parsing for full semantic fidelity.
- Adds implementation complexity versus plain text differencing.
- AST reconstruction quality depends on parser/unparser determinism.

---

## Alternatives Considered

- Line-based textual patching only.
- Token-based differencing.
- Snapshot-only CRDT layering with no semantic atom model.

All were rejected because they cannot provide the commutativity guarantees and AI-native semantic surface required by arc.

---

## References

- [Patch Theory](patch_theory.md)
- [AST Diffing](ast_diffing.md)
- [Spacetime Operation Log](oplog.md)
