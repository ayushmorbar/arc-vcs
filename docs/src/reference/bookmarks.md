---
title: "Bookmarks Reference"
description: "Command and behavior reference for mutable bookmarks and their interaction with revsets in arc."
---

# Bookmarks Reference

## BLUF

Bookmarks are mutable, named references to `ChangeId` heads. They are explicit coordination anchors for humans and automation.

---

## Commands

```bash
arc bookmark create <name> [rev]
arc bookmark set <name> [rev]
arc bookmark move <name> <rev> [--allow-backwards]
arc bookmark delete <name>
arc bookmark list
```

Default `rev` for `create`/`set` is `@`.

---

## Expected Output Examples

```bash
arc bookmark create trunk/main @
```

```text
Created bookmark 'trunk/main' at e4b8a1f0
```

```bash
arc bookmark move trunk/main a1b2c3d4 --allow-backwards
```

```text
Moved bookmark 'trunk/main' to a1b2c3d4
```

---

## Revset Interop

Use bookmarks in revset filters:

```bash
arc log -r "bookmarks()"
arc log -r "ancestors(bookmarks()) & touched(\"src/main.rs\")"
```

---

## Safety Notes

> **Note:** `move` enforces forward-only movement by default. Use `--allow-backwards` only when intentionally rewriting publication anchors.

| Behavior | Default |
| --- | --- |
| Create missing bookmark | Allowed via `create`/`set` |
| Backward move | Blocked |
| Grouped listing | `bookmark list` groups names by target change |

---

## See Also

- [Revsets](revsets.md)
- [Workspaces, Sparse, and Mounts](workspaces-sparse-mounts.md)
