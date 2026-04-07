---
title: Patch Theory
description: Documentation page for Patch Theory.
---

# Patch Theory

This document provides a formal treatment of arc's algebraic foundation. It is intended for contributors, researchers, and engineers who want to reason precisely about correctness guarantees.

---

## Motivation

Traditional version control systems — including Git — represent changes as textual diffs against a file snapshot. When two changes modify nearby lines, the merge algorithm cannot determine whether those changes are semantically compatible. The result is a "conflict" that is often a false positive: the lines changed for completely independent reasons, and any combination is valid.

arc eliminates this class of false conflicts by operating on a richer semantic representation.

---

## Atoms

An `Atom` is the smallest indivisible unit of change. The arc algebra defines five atom variants:

| Variant | Meaning |
|---|---|
| `Insert { at: NodePath, content: String }` | Insert a new AST node at `at` |
| `Delete { at: NodePath }` | Remove the AST node at `at` |
| `SemanticsPreserving { at, from, to }` | Transform the node at `at` without changing its semantic role |
| `Blob { path: String, hash: Blake3Hash }` | Whole-file content addressed by hash (used for non-Rust files) |
| `Mount { path: String, patterns: Vec<String> }` | Sparse checkout directive |

A `NodePath` is a `Vec<String>` representing the path from the root of the syntax tree to the target node — for example, `["file", "src/lib.rs", "fn_widget", "body", "stmt_3"]`.

---

## Changes

A `Change` wraps a set of atoms with metadata:

```
Change {
    id:       Blake3Hash       // hash of all other fields
    parents:  Set<Blake3Hash>  // parent Change IDs (empty for root)
    atoms:    Vec<Atom>
    message:  String
    author:   Author
    signature: Ed25519Signature
    timestamp: i64
}
```

The `ChangeGraph` is a DAG where nodes are `Change` objects and edges point from child to parents.

---

## The Commutativity Predicate

Two changes $A$ and $B$ *commute* if and only if applying them in either order produces identical results:

$$\text{apply}(B, \text{apply}(A, S)) = \text{apply}(A, \text{apply}(B, S))$$

for all states $S$.

In arc's implementation, `commutes(a, b)` checks whether any atom in `a` and any atom in `b` target the same `NodePath`:

- **No overlap** → the changes commute trivially (they modify disjoint parts of the tree).
- **Overlap** → the changes conflict. The exact conflicting atom pairs are recorded.

This check is conservative: it may report a conflict where one does not exist semantically, but it will never miss a genuine conflict.

---

## Merge as Commutativity Check

`merge_heads(target_heads)` implements the following algorithm:

1. Compute the lowest common ancestor (LCA) of `current_heads` and `target_heads` in the `ChangeGraph`.
2. Compute $\Delta A$ = changes reachable from `current_heads` but not from LCA.
3. Compute $\Delta B$ = changes reachable from `target_heads` but not from LCA.
4. For every pair $(a \in \Delta A, b \in \Delta B)$: if `!commutes(a, b)`, record the conflict.
5. If no conflicts: union the heads. The merge is complete.
6. If conflicts exist: serialize a `PendingConflict` and abort.

This is exactly the **LCA-based commutativity merge** described in the Darcs and Pijul literature, applied to an AST-level atom representation.

---

## Relationship to Darcs and Pijul

arc's patch theory is directly inspired by Darcs (David Roundy, 2003) and Pijul (Pierre-Étienne Meunier, 2017). The key differences:

| Property | Darcs | Pijul | arc |
|---|---|---|---|
| Atom granularity | Text hunks | Text hunks | **AST nodes** |
| Conflict representation | Unresolved patches in graph | Unresolved patches in graph | **`PendingConflict` + AI resolver** |
| Object identity | SHA-1 | BLAKE2 | **BLAKE3** |
| Implementation language | Haskell | Rust | **Rust** |
| Signatures | None | None | **Ed25519 per-Change** |

arc's AST-level atoms mean that two changes that happen to be adjacent in the text file but modify different AST nodes will always commute — eliminating the most common source of false conflicts in text-hunk-based systems.

---

## Formal Properties

The arc algebra satisfies the following properties (by construction):

**P1. Determinism:** `commutes(a, b) == commutes(b, a)` — commutativity is symmetric.

**P2. Independence:** If `commutes(a, b)`, the merged state is identical regardless of application order.

**P3. Conflict minimality:** A conflict is reported if and only if two atoms target the same `NodePath`. No other condition causes a conflict.

**P4. Integrity:** Every `Change` in the graph is signed and hash-verified on load. A tampered change cannot be introduced without detection.
