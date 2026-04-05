---
title: Ignore And Attributes
description: Documentation page for Ignore And Attributes.
---

# Ignore & Attributes

arc provides two mechanisms for controlling which files are tracked: `.arcignore` for exclusion and sparse checkout patterns for inclusion.

---

## `.arcignore`

arc respects `.arcignore` files using the same glob syntax as `.gitignore`. `.arcignore` is processed by the `ignore` crate (the same library used by `ripgrep`).

### Location

- Place `.arcignore` at the root of `work_root` for global ignores.
- Nested `.arcignore` files are supported (they apply to their containing directory and below).

### Syntax

```gitignore
# Comments start with #

# Ignore build output
target/

# Ignore all .o files
*.o

# Ignore a specific file
src/generated.rs

# Negate a pattern (unignore a previously ignored path)
!src/generated_keep.rs

# Double-star matches across directory boundaries
**/node_modules/
```

### Effect on arc Operations

| Command | Honours `.arcignore`? |
|---|---|
| `arc snap` | ✓ Yes — ignored files are never included in a `Change` |
| `arc status` | ✓ Yes — ignored files are not shown as modified |
| `arc diff` | ✓ Yes |
| `arc restore` | ✗ No — you can still restore an ignored file explicitly |

---

## Sparse Checkout (`Atom::Mount`)

Sparse checkouts in arc differ fundamentally from Git's path-glob sparse checkouts. In arc, sparse patterns are stored as `Atom::Mount` atoms — they are **part of the change graph** and are therefore version-controlled, signed, and auditable.

### Setting Patterns

```sh
# Replace all sparse patterns with a new set
arc sparse set "src/" "tests/"

# Add a pattern to the existing set
arc sparse add "benches/"

# Remove a specific pattern
arc sparse remove "benches/"

# List active patterns
arc sparse list
```

### Effect

When the active view contains one or more `Atom::Mount` atoms, `write_state_to_working_dir()` only writes files whose paths match at least one pattern. Files outside the sparse cone are removed from `work_root` (but remain in the CAS).

### Committing Sparse Patterns

`arc sparse set/add/remove` creates a new `Change` containing the `Atom::Mount` mutation. The sparse configuration is thereby part of the history and travels with the view when synced.

---

## Interaction with `.gitignore`

arc does **not** read `.gitignore` by default. If you want to reuse the same ignore rules, copy or symlink `.gitignore` to `.arcignore`. The syntax is identical.

---

## Binary Files

arc has no `.gitattributes` equivalent yet (see [SHORTCOMINGS.md](../../SHORTCOMINGS.md)). Files not recognised by any language plugin are automatically stored as `Atom::Blob` (whole-file). BLAKE3 hashing via `memmap2` ensures the operation is fast even for multi-gigabyte binaries.
