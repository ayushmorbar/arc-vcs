# API Stability

arc uses a four-tier stability classification. Understanding which tier an API belongs to tells you how much you can rely on it across minor and patch releases.

---

## Tier 1 — Production-Stable

Breaking changes require a **major version bump** (semver `x.0.0`), a formal RFC, and an Architecture Decision Record.

**Core algebra & storage:**
- `Atom` enum variants: `Insert`, `Delete`, `SemanticsPreserving`, `Blob`, `Mount`
- `Change` struct public fields: `id`, `parents`, `atoms`, `author`, `signature`
- `Blake3Hash` type alias and its serialization
- `commutes(a: &Change, b: &Change) -> bool`
- `apply_change()` — delta-apply semantics and error contract
- `ObjectStore` public API: `read_object()`, `write_object()`, `has_object()`

**Identity:**
- `Author` struct and its serialization format
- `load_identity()` / `save_identity()` — keypair location and file format
- Ed25519 signature encoding over `Change` bytes

**View model:**
- `View` struct: `name`, `heads: HashSet<Blake3Hash>`
- `View::load()`, `View::save()`, `View::new()`
- `snap()` — creates a `Change` from the working-directory delta
- `merge_heads()` — algebraic merge with commutativity check

**Stable CLI commands:**
`arc init`, `arc snap`, `arc log`, `arc status`, `arc diff`, `arc restore`,
`arc undo`, `arc view`, `arc switch`, `arc merge`, `arc auth`

---

## Tier 2 — Stable with Caveats

Stable in behaviour; minor surface changes permitted in a minor release (`1.x.0`). Changes require a rationale in the PR description.

- `arc-net` HTTP endpoints: `GET /cas/:hash`, `GET /view/:name`
- `arc fetch`, `arc pull`, `arc push` command semantics
- `RepoConfig` JSON format (existing fields are stable; new fields always use `#[serde(default)]`)
- `GcResult` struct fields
- `WorkspaceManifest` on-disk format

---

## Tier 3 — Experimental

These features are in active design. The surface may change in any minor release without a deprecation notice. Usable and documented, but expect churn.

**Hook engine:**
- Supported event names (`pre-snap`, `post-merge`) — new events may be added; none will be removed
- Command execution model (currently `std::process::Command`)

**AI resolution:**
- `AiResolver` trait — method signatures will evolve as more resolvers are integrated
- `PendingConflict` on-disk format

**Workspace:**
- `WorkspaceManifest.work_roots` layout — additional fields may be added

**Telemetry:**
- `ARC_TRACE` / `ARC_TRACE_EVENT` JSON event schema is not yet frozen

---

## Tier 4 — Unstable / Internal

Internal implementation details. May change in any patch release. Do not build external tooling against them.

- `ChangeGraph` internal traversal methods (`ancestors()`, `merge_base()` internals)
- `algebra/apply.rs` — delta application internals
- `bincode` encoding of `Change` — the exact byte layout is not guaranteed
- `MaterializedState` private structure
- `OpLog` on-disk binary format

---

## Deprecation Policy

When a Tier 1 or Tier 2 API is scheduled for removal:

1. Marked `#[deprecated(since = "x.y.z", note = "...")]` for at least one minor release.
2. A migration guide published in `docs/`.
3. The removal documented in `CHANGELOG.md`.

---

## Checking Your API Usage

```sh
# Identify deprecated items in the public API
cargo doc --no-deps -p arc-core 2>&1 | grep deprecated
```
