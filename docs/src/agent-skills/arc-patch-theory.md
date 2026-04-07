---
name: arc-patch-theory
description: >
  Core mathematical rules for semantic change composition in arc-vcs. Use when
  implementing, modifying, merging, commuting, inverting, replaying, or storing
  Change values and AST-based semantic operations.
---

# arc-patch-theory

## Purpose

This skill defines the invariants for semantic change in `arc-vcs`.

`arc` changes are not line patches. They are typed semantic atoms over syntax
trees with explicit dependency structure and replay semantics.

## Core laws

### 1. AST supremacy

Never model repository edits as line-number deltas, regex patches, or textual
hunks when the operation is supposed to be semantic.

Semantic operations must be expressed in typed structural terms such as:

- `Insert(Node)`
- `Delete(Node)`
- `Move(From, To)`
- `Replace(Old, New)`
- `SemanticsPreserving(...)`

### 2. Commutativity law

If two deltas do not share structural dependencies, they must commute under the
defined patch algebra.

Informal law:

`apply(B, apply(A, S)) == apply(A', apply(B', S))`

When this does not hold, identify the violated dependency or prove why the
operations are not independent.

### 3. Explicit dependencies

Every `Change` must carry explicit dependency information sufficient to embed it
inside a partial-order graph.

Do not hide ordering assumptions in execution order or incidental traversal.

### 4. Replayability

Each semantic atom should be:

- replayable,
- inspectable,
- invertible when designed to support inverse application,
- auditable in terms of provenance and dependency edges.

### 5. No fabricated semantics

If the parser, AST mapping, or semantic classifier cannot support a confident
semantic interpretation, emit an explicit scaffold or
`semantic-unavailable` marker.

Do not silently degrade to text-diff semantics.

## Implementation guidance

- Use tree-based identity and structure, not line offsets, as the primary model.
- Keep semantic labels separated from confidence.
- Preserve provenance for AI-assisted suggestions.
- Keep change algebra pure where possible; boundary code should adapt external
  inputs before they enter the core model.

## Review checklist

Before finalizing a change involving patch theory, verify:

- Is this still AST-native?
- Are dependencies explicit?
- Can the operation replay deterministically?
- Is commutativity stated or tested where independence is claimed?
- Is fallback behavior explicit rather than silent?