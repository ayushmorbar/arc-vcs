---
title: "Safe Linked Workspaces"
description: "Safe workflow for linked workspaces, sparse cones, and mounts."
---

# Safe Linked Workspaces

## BLUF

Use linked workspaces for parallel streams, sparse projections for bounded materialization, and mounts for sub-repository composition. Keep safety by respecting `WorkspaceManifest` boundaries and shared-root validation.

---

## Step 1: Add a linked workspace

```bash
arc workspace add ../arc-hotfix --view main
arc workspace list
```

## Step 2: Apply sparse projection

```bash
arc sparse set src tests
arc sparse list
```

## Step 3: Add and sync mount

```bash
arc mount add --path libs/parser --url /repos/parser --target main
arc mount sync
```

## Step 4: Verify root and linkage

```bash
arc workspace root ../arc-hotfix
```

If a workspace points at another shared root, commands that require linkage trust will fail by design.

---

## Cleanup Operations

```bash
arc workspace forget ../arc-hotfix
arc sparse reset
```

---

## See Also

- [Workspaces, Sparse, and Mounts](../reference/workspaces-sparse-mounts.md)
