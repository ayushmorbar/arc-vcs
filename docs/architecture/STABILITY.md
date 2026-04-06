# arc Stability Policy

This guide defines how arc applies semantic versioning, stability tiers, and
breaking-change cadence across the workspace.

## Terminology

- Workspace crate: any crate under `crates/*`
- Dependent crate: crate directly depending on another workspace crate
- Downstream: external project depending on one or more workspace crates
- Breaking change: source-level change requiring consumer code updates

## Stability Tiers

### Tier 1: Product Surfaces

- Intended crates: `arc-cli`, `arc-daemon`, `arc-git-bridge`
- Contract:
  - Strong UX and compatibility guarantees
  - Breaking changes require ADR + migration notes
  - Breaking release cadence: no more than once per 6 months

### Tier 2: Public Core/Platform APIs

- Intended crates: core algebra/engine/store/network/lang crates used by
  higher-level surfaces
- Contract:
  - Stable by default, explicit deprecation path preferred
  - Breaking changes batched and released no more than once per 4 weeks

### Tier 3: Initial Development / Experimental

- Intended crates: rapidly evolving internal or experimental crates
- Contract:
  - Faster iteration permitted
  - Breaking changes allowed with clear changelog and docs notes

## Primary 14-Crate Map

This map is the governance focus for cross-crate compatibility tracking.

1. `arc-algebra-types`
2. `arc-store-types`
3. `arc-change`
4. `arc-algebra`
5. `arc-engine`
6. `arc-revset`
7. `arc-store-cas`
8. `arc-store-graph`
9. `arc-store-view`
10. `arc-network`
11. `arc-lang`
12. `arc-cli`
13. `arc-daemon`
14. `arc-git-bridge`

Additional workspace crates are governed by this same model and can be promoted
into the primary map as their external contracts stabilize.

## Promotion Criteria

A crate may move from Tier 3 to Tier 2 (or Tier 2 to Tier 1) when:

1. Its intended scope is documented and largely complete.
2. Its public API has examples and usage guidance.
3. Integration tests for its boundaries are present and reliable.
4. Dependent crates no longer require frequent adaptation churn.

## Breaking Change Protocol

1. Open ADR describing rationale and alternatives.
2. Add deprecation notes where feasible.
3. Include migration guidance in release notes/docs.
4. Split commits to isolate the actual breaking change from adaptations.
