# Patch Theory

arc's patch theory draws on Darcs / Pijul operational-transform research, adapted for typed ASTs.

## Atoms

An `Atom` is the smallest unit of change. The five atom kinds are:

| Kind | Meaning |
|------|---------|
| `Insert { at, node }` | Add node at path |
| `Delete { at }` | Remove node at path |
| `Move { from, to }` | Relocate a subtree |
| `SemanticsPreserving { at, description }` | Whitespace / formatting only |
| `Directory { path }` | Ensure a directory node exists |

## Commutativity

Two atoms **commute** (can be freely reordered) when they do not name the same NodePath. Formally:

$$
p \circ q = q' \circ p' \quad \text{when} \quad \mathsf{paths}(p) \cap \mathsf{paths}(q) = \emptyset
$$

The `commute` module in `arc-core` implements this decision procedure. When commutativity fails, a `ConflictError` is returned and the AI resolver is invoked.

## Change graph

Changes form a DAG (`ChangeGraph`) whose edges encode causal dependency. A `View` is a set of change IDs that constitute the "checked out" state. The topological sort of reachable changes, applied in order, deterministically reconstructs the file tree.
