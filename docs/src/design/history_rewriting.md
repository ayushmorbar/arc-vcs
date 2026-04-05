---
title: "History Rewriting"
description: "Design reference for algebraic inversion, commutation, squash, and diffedit workflows."
---

# History Rewriting

arc's algebraic foundation makes history rewriting precise, safe, and
mathematically auditable.  Unlike Git's `rebase --interactive` (which
replays textual patches on a new base), arc rewrites history by composing,
inverting, and commuting typed **Atoms** — operations that carry their own
commutativity proof at the type level.

---

## 1. Motivation

Two common operations that Git handles poorly:

1. **Squashing** — collapse "WIP" or "fixup" commits into one clean logical
   change before merging.  Git's `git squash` loses provenance; arc's
   `squash_into` produces a new content-addressed Change that is
   mathematically equivalent to the entire linear spine.

2. **Diffedit** — "I want to open `vi` and change exactly what a past commit
   did".  Git requires a dangerous `rebase --interactive`; arc provides a
   safe two-step prepare/apply workflow backed by a lockfile and the
   inversion algebra.

Both operations rely on two mathematical primitives: **inversion** and
**commutation**.

---

## 2. Inversion Algebra

Every atomic mutation has a mathematical inverse:

| Forward atom | Inverse |
|---|---|
| `Insert { at, content_hash }` | `Delete { at, prior_hash: content_hash }` |
| `Delete { at, prior_hash }` | `Insert { at, content_hash: prior_hash }` |
| `Move { from, to }` | `Move { from: to, to: from }` |
| `SemanticsPreserving { .. }` | `SemanticsPreserving { .. }` (identity) |

`invert_atom(a)` maps each atom to its inverse.
`invert_change(c)` maps the entire atom list to its element-wise inverse,
**reversed** — because the inverse of a composition `a₁ ∘ a₂` is
`a₂⁻¹ ∘ a₁⁻¹`.

The key invariant: `apply(apply(state, c), invert_change(c)) == state`.

This is implemented in `arc-algebra::inverse`.

---

## 3. Commutation — The 4 Gates

Two changes **commute** if applying them in either order produces the same
result.  `commute_pair(a, b)` returns `Some((b′, a′))` when commutation is
safe, where `b′` and `a′` are the rewritten versions of `b` and `a` with any
path adjustments applied.

| Gate | Condition | Result |
|---|---|---|
| **Gate 1 — Independent paths** | `a` and `b` operate on disjoint AST paths | Always commutes; no rewriting needed |
| **Gate 2 — Insert/Delete at same path** | same path, opposite types | **Conflict** — returns `None` |
| **Gate 3 — Move source/target conflict** | `b` moves a node that `a` also touches | **Conflict** — returns `None` |
| **Gate 4 — Move path rewriting** | `a` is a `Move`; `b` operates on the moved path | Commutes; `b` is rewritten to use the new path |

Gate 4 is the key insight: if Alice moves `fn_render → fn_paint` and Bob
inserts a line inside `fn_render`, the pair can still commute — Bob's atom is
rewritten to target `fn_paint` after commutation.  This eliminates an entire
category of spurious textual conflicts that Git cannot handle.

---

## 4. `squash_into` — Linear Spine Fusion

`squash_into(target_rev)` collapses the changes between `HEAD` and
`target_rev` into a single new Change.

**Algorithm:**

1. `resolve_rev(target_rev)` → validate the target exists in the graph.
2. Walk from `HEAD` toward `target_rev` via `ChangeGraph::topological_sort`.
3. **Linear check** — assert each step has exactly one parent; bail if a
   merge node is found.
4. Collect all atoms from the linear spine in topo order.
5. Remove self-cancelling `Insert`/`Delete` pairs at the same path
   ("atom fusion" — intermediate states collapse to their net delta).
6. Construct a single new `Change` signed with the current identity.
7. Replace the spine in the view's heads with the single new change id.

The resulting repository state is mathematically identical to applying all
squashed changes in sequence.  The original intermediate changes remain in
the CAS (unreachable from the view) and are eligible for GC after
`arc compact`.

---

## 5. `diffedit` — Two-Step External-Editor Workflow

`arc diffedit` lets a human (or AI) edit the materialised effect of a past
change using any external editor, then re-inserts the edited version back into
the graph.

### Prepare phase (`--prepare <rev>`)

1. Look up `rev` in the graph; materialise its atom set into a temp file at
   `.arc/diffedit_target`.
2. Write a lockfile `.arc/diffedit_session` containing the change id.
3. Set a guard in `snap()` that rejects new snaps while a session is active
   (prevents accidental parallel mutations).

### Apply phase (`--apply`)

1. Read the lock file; verify the session is active.
2. Read the modified `.arc/diffedit_target`.
3. Diff the original materialised bytes against the edited bytes to produce
   a new atom list (using the same diffing engine as `arc snap`).
4. Construct a new `Change` from these atoms, signed with the current identity.
5. Insert the new change into the graph as a child of the original change's
   parent (not a child of the original change itself — the original is
   effectively replaced from the view's perspective).
6. Remove the lock file and temp target.

The original change is **not deleted** from the CAS — it remains permanently
for audit purposes and can be recovered via `arc log` on an archived view.

---

## 6. Safety Properties

| Property | Guarantee |
|---|---|
| CAS integrity | Every intermediate and final Change is content-addressed and signed — rewrites create new objects, never mutate existing ones |
| Lockfile atomicity | `diffedit` session lock uses the same atomic-rename protocol as View saves |
| Graph soundness | `squash_into` verifies linear topology before mutating the view |
| No orphan blobs | Squashed changes carry all blob references from the original atoms |
