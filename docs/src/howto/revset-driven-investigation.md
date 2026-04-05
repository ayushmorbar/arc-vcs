---
title: "Revset-Driven Investigation"
description: "Practical revset workflow for narrowing causative changes quickly."
---

# Revset-Driven Investigation

## BLUF

When history gets noisy, start with `ancestors(@)` and narrow with `touched("path")`, then pivot to `bookmarks()` and `tags()` for release context.

---

## Step 1: Scope to current history

```bash
arc log -r 'ancestors(@)'
```

This sets the baseline query window.

## Step 2: Restrict to impacted path

```bash
arc log -r 'ancestors(@) & touched("src/main.rs")'
```

Use this to isolate likely causative changes quickly.

## Step 3: Add release anchors

```bash
arc log -r 'ancestors(@) & (bookmarks() | tags())'
```

This reveals branch/tag anchor points inside your active ancestry.

## Step 4: Verify specific candidate

```bash
arc show <change-id>
arc diff
```

Confirm semantic intent before acting.

---

## Fast Troubleshooting

- Query fails with argument error: verify `touched()` uses exactly one quoted path.
- Query returns too much: intersect with `ancestors(@)`.
- Query returns nothing: validate path string and repository-relative location.

---

## See Also

- [Revsets](../reference/revsets.md)
