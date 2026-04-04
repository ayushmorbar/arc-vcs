# arc: Atomic Replayable Changes

Status: Stable core workflow, with explicitly documented gaps.

Arc is a semantic, content-addressed version-control system. Instead of line hunks, Arc records typed change atoms and replays them over a DAG-backed state model.

## Mental Model

Use this model if you are new:

1. You edit files.
2. `arc snap` computes semantic deltas and records a signed `Change`.
3. Changes are stored by content hash in CAS.
4. Views point to head sets over the change graph.
5. Merge checks commutativity before combining heads.

## What Is Code-Verified Today

- Content addressing with BLAKE3-backed object IDs.
- Signed provenance via Ed25519 identity flow.
- Graph + view based history traversal and merge-base calculations.
- Rust AST-aware plugin path in `arc-lang`.
- CLI workflows for init/snap/log/status/merge/fetch/pull/push and related operations.

## Known Limits (Do Not Surprise Users)

- Conflict marker rendering to working files is limited and still evolving.
- Some atom categories are present in model discussions but not fully replay-supported in all paths.
- Top-level `src/` is useful for analysis context; workspace build truth is `crates/*`.

## Audience Paths

- New users: start with [Tutorial](getting-started/tutorial.md).
- Day-to-day users: [Everyday Workflow](getting-started/everyday.md).
- Power users and maintainers: [CLI Reference](reference/cli-reference.md) and [Architecture Overview](architecture/overview.md).

## Single Source of Truth

Documentation map and ownership rules live in [Documentation Map](architecture/documentation-map.md).
