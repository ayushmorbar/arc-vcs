# arc Architecture (Code-Verified)

Status: Stable for current workspace crates, with explicitly marked gaps.

This file is the wiki-facing architecture index for Arc. The canonical implementation details live in the mdBook pages under `docs/src/`.

## Read This First

- mdBook architecture overview: `docs/src/architecture/overview.md`
- Documentation map (SSOT for doc coverage): `docs/src/architecture/documentation-map.md`
- CLI contract: `docs/src/reference/cli-reference.md`

## System Snapshot

Arc in this workspace builds as a Cargo workspace with crates under `crates/*`.

- `arc-core`: algebra, CAS, DAG, identity, revsets, operation log
- `arc-lang`: language plugins (Rust plugin via tree-sitter)
- `arc-net`: sync transport and server-side networking pieces
- `arc-git-bridge`: Git translation boundary (push/import interop)
- `arc-cli`: command surface and repository orchestration
- `arc-daemon`: daemon/runtime integration surface

## Data Path (High Level)

1. CLI command enters via `arc-cli`.
2. Repository orchestration calls into `arc-core`.
3. AST-level semantics route through `arc-lang`.
4. Sync paths use `arc-net` and/or `arc-git-bridge`.
5. CAS objects are content-addressed and replayed through graph/view state.

## Explicit Scope Boundary

Top-level `src/` exists and is useful for historical/prototype context, but workspace build truth is `crates/*` as declared in `Cargo.toml`.

## Non-Speculation Rule

If behavior is not implemented in code, it must be documented in `SHORTCOMINGS.md` and/or roadmap docs, not as current capability.
