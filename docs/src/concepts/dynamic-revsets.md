# Dynamic Revsets

## BLUF

In arc, revsets are typed DAG queries over semantic changes, not line-history grep. They are intended to be composed and executed repeatedly as operational filters.

---

## Core Mental Model

A revset compiles into a change-ID iterator.

- Graph topology comes from `ChangeGraph` ancestry.
- Symbol resolution maps user-facing names (including `@`) to concrete change IDs.
- Function resolvers map metadata-backed references such as bookmarks and tags.

Because revsets target semantic changes, they align cleanly with AST-aware workflows.

---

## Why These Four Functions Matter

- `ancestors()` gives stable history closure for any investigation.
- `touched("path")` maps directly to atom path impact.
- `bookmarks()` and `tags()` connect query logic to human coordination points.

Together, these functions are enough for most production triage and release slicing tasks.

---

## Design Constraint

Revset correctness is favored over permissive parsing.

- Wrong arity fails.
- Wrong argument type fails.
- Unknown symbols fail.

This keeps automation deterministic and safe.

---

## See Also

- [Revsets](../reference/revsets.md)
- [Revset-Driven Investigation](../howto/revset-driven-investigation.md)
