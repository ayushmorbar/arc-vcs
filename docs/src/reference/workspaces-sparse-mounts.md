---
title: "Workspaces, Sparse, and Mounts"
description: "Reference for linked workspace manifests, sparse projection, and mount sync."
---

# Workspaces, Sparse, and Mounts

## BLUF

Arc supports linked workspaces with a shared CAS root, sparse projections over materialization, and mount atoms for sub-repository composition. These are safety-bounded operations, not ad-hoc filesystem tricks.

---

## Workspace Manifest

Linked workspaces persist `.arc-workspace` with a typed `WorkspaceManifest`:

- `shared_root`: canonical repository root containing `.arc/`
- `view`: checked-out view name
- `sparse_patterns`: active sparse cone list

This manifest is the trust boundary for workspace linkage.

---

## Workspace Commands

```bash
arc workspace add <path> --view <name>
arc workspace list
arc workspace root [path]
arc workspace forget <path>
arc workspace rename <old_path> <new_path>
arc workspace update-stale
```

Cross-repo safety bound: operations that mutate workspace linkage verify manifest `shared_root` matches the current repository.

---

## Sparse Commands

```bash
arc sparse set src tests
arc sparse set --add benches
arc sparse list
arc sparse edit
arc sparse reset
```

`arc sparse` updates projection state and rematerializes the working directory.

Sparse safety bounds:

- patterns are validated
- out-of-cone files are removed from work root projection
- reset restores full checkout behavior

---

## Mount Commands

```bash
arc mount add --path libs/parser --url /repos/parser --target main
arc mount sync
```

Mounts are represented as `Atom::Mount` and materialized as mount tokens that `mount sync` resolves into concrete sub-repository updates.

---

## See Also

- [Workspace and Sparse Boundaries](../concepts/workspace-boundaries.md)
- [Safe Linked Workspaces](../howto/safe-linked-workspaces.md)
