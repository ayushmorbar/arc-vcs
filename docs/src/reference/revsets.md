---
title: "Revsets"
description: "Typed DAG query reference for selecting ChangeId sets in arc."
---

# Revsets

## BLUF

Use revsets to query the change DAG precisely. In current arc, the highest-value built-ins are:

- `ancestors(<revset>)`
- `touched("path")`
- `bookmarks()`
- `tags()`

You typically execute revsets with `arc log -r <expression>`.

---

## What Arc Evaluates

Revsets are parsed and compiled in `arc-revset` and evaluated over change IDs in the DAG.

- Symbol resolution supports IDs, `@`, `HEAD`, and named refs resolved by the repository.
- `ancestors()` computes transitive closure over graph ancestry.
- `touched()` filters by atom paths (file-aware node paths).
- `bookmarks()` and `tags()` resolve metadata-backed heads.

---

## Core Functions

### ancestors()

```bash
arc log -r 'ancestors(@)'
```

Selects all ancestors of the input revset.

### touched(path)

```bash
arc log -r 'touched("src/main.rs")'
```

Selects changes that touch the given repository path.

Important: `touched()` expects exactly one string literal argument.

### bookmarks()

```bash
arc log -r 'bookmarks()'
```

Selects heads currently referenced by bookmarks.

### tags()

```bash
arc log -r 'tags()'
```

Selects heads currently referenced by tags.

---

## Composition Patterns

```bash
# Show all tagged or bookmarked heads and their ancestry intersection with current history
arc log -r 'ancestors(bookmarks() | tags()) & ancestors(@)'

# Changes in current history that touched a specific file
arc log -r 'ancestors(@) & touched("src/main.rs")'
```

---

## Failure Modes

- Unknown symbol: revset references a name arc cannot resolve.
- Wrong arity: function called with the wrong number of arguments.
- Wrong argument type: `touched()` argument is not a string literal.

---

## See Also

- [Dynamic Revsets](../concepts/dynamic-revsets.md)
- [Revset-Driven Investigation](../how-to/revset-driven-investigation.md)

