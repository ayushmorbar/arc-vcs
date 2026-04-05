# Tutorial: Linked Workspace with Sparse Scope

## Goal

Create a second workspace, limit projection to relevant paths, and synchronize a mount.

---

## Step 1: Add workspace

```bash
arc workspace add ../arc-feature --view main
arc workspace list
```

## Step 2: Narrow projection

```bash
arc sparse set src tests
arc sparse list
```

## Step 3: Add mount and sync

```bash
arc mount add --path libs/parser --url /repos/parser --target main
arc mount sync
```

## Step 4: Validate linkage

```bash
arc workspace root ../arc-feature
```

---

## Result

You now have a bounded workspace projection backed by shared CAS and explicit mount semantics.
