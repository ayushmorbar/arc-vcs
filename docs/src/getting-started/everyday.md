# Everyday Workflow

This page covers the commands you will use in every arc session.

---

## Starting a Session

```sh
cd my-project
arc status          # see what has changed since the last snap
```

---

## Recording Changes

```sh
# Record everything in the working directory
arc snap -m "feat: add widget factory"

# Interactive staging — accept or reject individual AST atoms
arc snap -i -m "refactor: clean up widget module"
```

`arc snap` parses every `.rs` file with the Tree-sitter Rust plugin, computes the AST delta relative to the current view's materialised state, and creates a signed `Change`.

---

## Inspecting State

```sh
arc status          # what atoms differ from the last snap
arc diff            # formatted per-atom diff
arc log             # full change history for the current view
arc log --short     # condensed one-line format
```

---

## Undoing and Restoring

```sh
arc undo            # pop the last operation from the OpLog
arc restore src/widget.rs    # revert a single file to its last-snapped state
```

`arc undo` is safe — it replays the inverse operation atomically. It does **not** rewrite history; it creates a new reverse change.

---

## Views

arc Views are named sets of DAG heads. Think of them as branches that know about algebra.

```sh
arc view create feature/my-work   # create a new view forked from the current heads
arc switch feature/my-work        # switch working directory to that view
arc view list                     # list all views
arc merge feature/my-work         # merge a view into the current one (with commutativity check)
```

---

## Merging

```sh
arc merge feature/experiment
```

arc computes the LCA, extracts the exclusive deltas on each side, and runs a cross-product commutativity check. If all pairs commute, the merge is instant and automatic. If any pair conflicts, arc writes `.arc/conflict` and reports the exact pair of change IDs.

---

## Garbage Collection

```sh
arc gc
```

Prunes changes that are causally stable — reachable by all known operationally-connected heads — and not referenced by any live view. Reports retained and pruned counts.

---

## Viewing Configuration

```sh
arc config get remotes
arc config set alias.st "status"
arc config get aliases
```

---

## Sync

```sh
arc remote add origin http://arc-server:8080
arc fetch origin
arc pull origin main
arc push origin main
```

---

## Telemetry During Debugging

```sh
ARC_TRACE=1 arc merge feature/complex-work
```

This activates compact structured tracing to stderr so you can see exactly what `merge_heads()` is computing.
