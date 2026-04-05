# Workspace and Sparse Boundaries

## BLUF

Arc workspaces are linked projections over a shared CAS root. Sparse and mount behavior is bounded by typed manifests and repository-validated linkage, not implicit trust in directory layout.

---

## Shared Root vs Work Root

- `shared_root` holds repository truth (`.arc/`).
- `work_root` holds materialized projection for a specific workspace/view.

`WorkspaceManifest` explicitly binds these roots and the active view.

---

## Sparse as Projection Contract

Sparse patterns define which file paths are materialized.

- Out-of-scope files are removed from projection.
- In-scope files are rematerialized from graph state.

This is a projection operation, not a partial repository copy.

---

## Mount as Algebra

`Atom::Mount` encodes sub-repository intent in history. `mount sync` realizes declared mounts by fetching/updating target repositories.

Mounts are therefore reviewable and replayable semantics, not ad-hoc local scripts.

---

## Cross-Repo Safety Bound

Workspace maintenance commands verify linked manifests point to the same shared root. Misbound workspaces are rejected.

This prevents accidental operations across unrelated repositories.

---

## See Also

- [Workspaces, Sparse, and Mounts](../reference/workspaces-sparse-mounts.md)
- [Safe Linked Workspaces](../howto/safe-linked-workspaces.md)
