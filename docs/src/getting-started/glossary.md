# Glossary

---

**Atom**
The smallest unit of change in arc. Current atom variants include `Insert { at, content_hash }`, `Delete { at, prior_hash }`, `Move { from, to }`, `SemanticsPreserving { at, description }`, `Directory { path }`, `Blob { path, hash }`, `Mount { path, url, target }`, and `Conflict { bases, sides, at }`. Multiple atoms are grouped into a `Change`.

**Blake3Hash / BLAKE3Hash**
A 32-byte content identifier computed with the BLAKE3 cryptographic hash function. Every `Atom`, `Change`, `Tag`, and blob object in the CAS is identified by its BLAKE3 hash. BLAKE3 provides 256-bit security at approximately 3× the speed of SHA-256.

**CAS (Content-Addressable Storage)**
The object store at `.arc/blobs/`. Objects are stored and retrieved by their BLAKE3 hash. An object cannot be forged or tampered with without changing its hash, providing automatic integrity verification.

**Causal Stability**
A change is causally stable with respect to a set of views when every view that is operationally connected to the repository has that change in its ancestry. Causally stable changes can be safely garbage-collected without losing any reachable history.

**LCA (Lowest Common Ancestor)**
The nearest shared ancestor change between two or more heads in the `ChangeGraph`. Arc uses LCA/merge-base computation to derive exclusive deltas before commutativity checks during merge.

**Change**
A signed, immutable record of one or more `Atom`s. A `Change` carries: parent head hashes, the atom list, the author's public key, an Ed25519 signature, a commit message, and a timestamp. Its identity is its BLAKE3 hash over all these fields.

**ChangeGraph**
The directed acyclic graph (DAG) of all `Change` objects in the repository. Edges point from child to parents. `ChangeGraph` provides `ancestors()`, `merge_base()`, and ancestry diffing.

**commutes(a, b)**
The core predicate of arc's algebraic model. Returns `true` if `Change` `a` and `Change` b can be applied in either order without semantic difference. Returns `false` if they conflict — i.e., they both modify the same AST structural node in incompatible ways.

**Hook**
An external command registered in `.arc/config.json` under the `hooks` key. Hooks are triggered by arc lifecycle events (`pre-snap`, `post-merge`). They are run in `work_root` with `shlex`-parsed arguments. A non-zero exit code aborts the current operation.

**MaterializedState**
The in-memory map from file paths to AST node content, produced by replaying all `Change`s reachable from a set of heads. `write_state_to_working_dir()` writes this state to disk.

**Materialization**
The process of projecting graph state into real files under `work_root`. In Arc, materialization can project normal content or deterministic conflict markers derived from structured conflict atoms.

**OpLog**
A local, append-only log at `.arc/oplog` recording every mutating operation (`snap`, `merge`, `restore`, etc.) with enough information to replay the inverse. Powers `arc undo`.

**Sparse Checkpoint / Atom::Mount**
An `Atom::Mount` embeds sparse checkout patterns directly into the change graph. When a view containing a `Mount` atom is materialised, only files matching the declared path patterns are written to `work_root`.

**Split-Root Workspace**
A configuration where `shared_root` (containing `.arc/`) and `work_root` (the checked-out files) are different directories. Multiple work roots can share a single CAS store via a `WorkspaceManifest`.

**NewType Pattern**
A type-safety pattern where semantically distinct values wrap primitive data in dedicated types (for example, wrapping raw byte arrays or strings in domain-specific structs). Arc uses strong domain types to reduce accidental misuse across storage, graph, and identity boundaries.

**Trace2 / ARC_TRACE**
arc's structured telemetry system, modelled after Git's Trace2 architecture. Zero overhead when disabled. Activated via `ARC_TRACE=1` (compact stderr) or `ARC_TRACE_EVENT=<path>` (JSON append file).

**View**
A named set of `Blake3Hash` heads in the `ChangeGraph`. A View is **not** a pointer to a single snapshot (unlike a Git branch). It tracks the frontier of a continuous, multi-headed stream of semantic intents. Views are stored as JSON at `.arc/views/<name>.json`.

**work_root**
The directory where arc writes materialised source files. Equivalent to Git's working tree. In a single-root repository, `work_root` and `shared_root` are the same. In a split-root workspace, they differ.

**ADR (Architecture Decision Record)**
A short document capturing a significant architectural decision: the context, the decision made, and the consequences. arc's ADRs live in `docs/src/architecture/ADRs/`. See the [Governance](../../../GOVERNANCE.md) document for the ADR process.
