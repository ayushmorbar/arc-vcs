---
title: "Resolve Conflicts with AI or Merge Tool"
description: "Operational guide for staged conflict resolution and approval."
---

# Resolve Conflicts with AI or Merge Tool

## BLUF

Run `arc ai resolve` to stage conflict resolutions as a pending Ghost Node, then finalize with `arc ai approve`. If a merge tool is configured, arc attempts that path first and falls back to provider-based AI when needed.

---

## Prerequisites

- Active conflict state exists (`.arc/conflict`).
- If using provider-based AI fallback, `ARC_AI_API_KEY` is set.
- If using merge tool, `merge.tool` and `[merge-tools.<name>]` are configured.

---

## Step 1: Trigger resolution

```bash
arc ai resolve
```

Outcomes:

- merge-tool resolution staged (if configured and successful), or
- fallback AI resolution staged.

Both produce a pending change file under `.arc/ai/pending.json`.

## Step 2: Review staged result

```bash
arc diff
arc status
```

Ensure resolved files match engineering intent and compile constraints.

## Step 3: Finalize history update

```bash
arc ai approve
```

This signs and commits the pending Ghost Node.

---

## Failure Handling

- "no pending conflict": conflict file already consumed or never created.
- "pending AI change already exists": approve or discard pending state first.
- merge-tool execution failure: arc automatically tries AI fallback for supported failure classes.

---

## See Also

- [Conflict Resolution Protocol](../reference/conflicts.md)
