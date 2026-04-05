---
title: "Collaborate with Multiple Remotes"
description: "Configure origin/upstream style remotes and push/pull explicitly in arc without branch-name ambiguity."
---

# Collaborate with Multiple Remotes

## BLUF

Define remotes explicitly, then fetch/pull/push by remote and view. Keep bookmark movement deliberate to avoid accidental publication.

---

## Step 1: Register remotes

```bash
arc remote add upstream /repos/platform-core
arc remote add origin /repos/alice-fork
arc remote list
```

Expected output excerpt:

```text
origin   /repos/alice-fork
upstream /repos/platform-core
```

---

## Step 2: Pull target view from upstream

```bash
arc pull upstream main
```

Then inspect local frontier:

```bash
arc log -r "ancestors(@)"
```

---

## Step 3: Publish your view to fork remote

```bash
arc push origin main
```

> **Note:** Keep publication references stable using bookmarks when sharing reviewable milestones.

---

## Optional: Pin share points with bookmarks

```bash
arc bookmark set review/main @
arc bookmark list
```

---

## Remote Hygiene Table

| Practice | Why it matters |
| --- | --- |
| Keep `upstream` read-mostly | Reduces accidental direct publication |
| Push to fork (`origin`) first | Preserves review and rollback path |
| Use explicit view names | Avoids target ambiguity in CI and team scripts |

---

## See Also

- [CLI Reference](../reference/cli-reference.md)
- [Revset-Driven Investigation](revset-driven-investigation.md)
