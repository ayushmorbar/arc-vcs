# Arc v0.2 Roadmap

## Metadata

- Version: v0.2 initialization
- Last updated: 2026-04-07
- Primary owner: core-architecture
- Program status: Active planning and staged delivery

## Mission

Deliver an Autonomous Conflict Resolver that is deterministic at core semantics, auditable at every decision boundary, and safe for opt-in autonomous operation on replayable change graphs.

## Scope

In scope:

1. Conflict resolution improvements for AST-aware CRDT workflows.
2. Guarded LLM-assisted proposal and ranking interfaces at boundary crates.
3. End-to-end observability and governance controls for autonomous paths.

Out of scope for initial v0.2 milestone:

1. Fully autonomous default-on conflict resolution for all users.
2. Relaxation of v0.1 CI/release/security guardrails.
3. New non-essential workflow surfaces unrelated to conflict resolution correctness.

>Note: this roadmap is a living document and is subject to change based on ongoing discoveries, risk assessments, and governance feedback. Regular updates will be published in the `docs/reports/roadmap/` directory with clear versioning and change logs.

## Workstreams

### Workstream A: Deterministic Merge Core

Goals:

1. Extend CRDT merge with language-aware structural reconciliation.
2. Resolve on typed atom paths and semantic identities, not line spans.
3. Preserve replayability and provenance for all automated outcomes.

Definition of done:

1. Deterministic baseline passes against representative conflict corpus.
2. Reverse-order and replay-invariance tests pass for merged output.
3. Stage-tagged traces emitted for discover, negotiate, transfer, materialize, finalize.

### Workstream B: Guarded LLM Boundary

Goals:

1. Add explicit LLM hook interfaces for proposal generation and ranking.
2. Keep core state transitions deterministic and side-effect free.
3. Enforce policy checks before materialization/finalization.

Definition of done:

1. Full audit record exists per resolution attempt (prompt context hash, model metadata, selected rationale).
2. Bounded execution and explicit deterministic fallback are enforced.
3. Boundary crates own all model invocation paths.

### Workstream C: Reliability, Risk, and Rollback

Goals:

1. Track autonomous-path correctness and latency under realistic repository fixtures.
2. Define rollback controls and operator runbooks.
3. Keep opt-in gating until SLO and quality targets are sustained.

Definition of done:

1. Differential validation against deterministic baseline is automated.
2. Rollback path is tested and documented.
3. Governance sign-off includes observability and risk coverage evidence.

## Delivery Constraints

1. No relaxation of existing v0.1 P0/P1 CI and release guardrails.
2. Maintain wasm-safe core boundaries and purity contracts.
3. Keep autonomous resolution opt-in until SLO and regression criteria are met.

## Milestone Exit Criteria

The milestone is considered ready only if all criteria below are satisfied:

1. Conflict auto-resolution benchmark suite meets agreed correctness and latency targets.
2. Differential validation shows no unacceptable divergence from deterministic baseline on representative repositories.
3. Governance review approves risk controls, observability coverage, and rollback procedures.
4. Monthly report includes explicit evidence links for these readiness claims.

## DX and Best-Practice Notes

1. Keep contributor ergonomics high: deterministic failure reasons should be human-readable and stage-labeled.
2. Prefer typed policy boundaries over ad hoc runtime flags.
3. Treat auditability as a product requirement, not post-hoc compliance work.
4. Keep roadmap updates small and frequent; avoid stale milestone narrative.
