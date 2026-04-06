# arc Shortcomings

This document tracks known limitations in arc with explicit impact, rationale,
and mitigation plans. It is intentionally public and should be updated whenever
new constraints are discovered.

## Scoring Model

- Severity: `critical`, `high`, `medium`, `low`
- Horizon: `immediate`, `near-term`, `long-term`
- Status: `open`, `mitigating`, `resolved`

## Active Shortcomings

### 1) High Memory During Heavy Conflict Resolution

- Area: CRDT spacetime merge and conflict materialization
- Severity: high
- Horizon: near-term
- Status: open

Why it exists:
- Arc stores enough causality and replay context to preserve deterministic,
  conflict-safe outcomes. During high-conflict merges, retained spacetime
  context can increase peak memory.

User impact:
- Large repos or branches with many divergent edits may observe higher memory
  than traditional line-oriented merge tools.

Mitigation roadmap:
1. Add conflict-window chunking to reduce simultaneous in-memory state.
2. Add bounded cache controls for conflict evaluation.
3. Add per-stage memory telemetry (`discover`, `negotiate`, `transfer`,
   `materialize`, `finalize`) to guide optimization.

### 2) CRDT Graph Explainability for New Contributors

- Area: graph/replay mental model
- Severity: medium
- Horizon: near-term
- Status: mitigating

Why it exists:
- Spacetime DAG semantics provide stronger correctness guarantees but have a
  steeper learning curve than linear VCS history.

User impact:
- Slower onboarding for contributors who are new to causal graph systems.

Mitigation roadmap:
1. Expand architecture examples with end-to-end replay traces.
2. Add CLI explainers for merge/revset decisions.
3. Add glossary-first references to all major docs.

### 3) Cross-Platform Edge Cases in Boundary Integrations

- Area: CLI/daemon/git bridge boundaries
- Severity: medium
- Horizon: ongoing
- Status: open

Why it exists:
- Platform-specific filesystem and process behavior can differ at boundaries,
  even when core algebra remains platform-neutral.

User impact:
- Intermittent behavior differences in tool integration flows.

Mitigation roadmap:
1. Expand boundary integration test matrix across OS targets.
2. Keep platform-sensitive logic isolated in boundary crates.
3. Track known failures with links to reproducible fixtures.

## Governance Rules

1. Every newly discovered limitation gets an entry within one PR cycle.
2. Entries must include impact and mitigation, not only a symptom.
3. Resolved items remain for one release cycle with `resolved` status, then may
   be archived.
