---
title: "ADR-004: Gitoxide Architecture Study to Arc Architecture Policy"
description: "Decision record that converted the Gitoxide study into enforceable architecture, CI, and DX policy in arc."
status: Implemented and Archived
date: 2026-04-07
scope: v0.1 architecture hardening
note: "Historical ADR preserved to explain why specific architecture and CI guardrails exist."
---

# ADR 004 - Gitoxide Study to Enforceable Arc Policy

| Field        | Value                                                                       |
| ------------ | --------------------------------------------------------------------------- |
| Status       | Implemented and Archived                                                    |
| Date         | 2026-04-07                                                                  |
| Deciders     | arc core architecture maintainers                                           |
| Source Input | gix component maps, report extraction ledger, arc v0.1 architecture reviews |

## Context

The original study identified a recurring issue: arc had strong architectural intent, but too much of it was documented as narrative rather than enforced as policy.

The main gaps were:

1. Layering policy was described but not machine-enforced.
2. Heavy operations did not share one standardized stage taxonomy across crates.
3. Platform primitives (locks, temp files, atomic publish) needed stronger isolation.
4. Governance and report quality checks needed CI enforcement.
5. API drift and architecture drift needed objective, repeatable gates.

## Decision

arc adopts five architecture governance decisions from this study:

1. Use a machine-readable component graph as the architecture source of truth.
2. Enforce layering constraints in CI using explicit forbidden class edges.
3. Standardize operation telemetry on five stages: discover, negotiate, transfer, materialize, finalize.
4. Isolate filesystem safety primitives into a foundation crate.
5. Treat architecture and reporting hygiene as blocking CI concerns, not optional process guidance.

## Implemented State (Verified)

The decisions above are implemented in the repository as of 2026-04-07.

1. Component graph and policy are defined in docs/src/architecture/component-graph.json.
2. CI enforces architecture layering via scripts/ci/enforce-layering.sh.
3. CI enforces API baselines via scripts/ci/check-api-drift.sh and docs/architecture/api-baselines/.
4. CI detects architecture drift via scripts/ci/detect-arch-drift.sh.
5. CI validates reporting quality via scripts/ci/lint-reports.sh.
6. CI includes package-size guardrails via scripts/ci/check-package-size.sh.
7. Core operation staging is implemented in crates/arc-core/src/ops.rs and repository sync wrappers.
8. ARC_SYNC_SLO_MS controls operation SLO warning thresholds in stage-timed flows.
9. Platform file-ops concerns are isolated in crates/arc-fs-ops.

## Consequences

Positive:

1. Architecture intent is now testable and reviewable as code and policy.
2. Regressions in crate boundaries are caught early in CI.
3. Operational telemetry is consistent across heavy workflows, improving observability and incident triage.
4. DX improves because contributors can see exactly which policy failed and where.
5. Release and governance quality checks moved from best-effort to enforced practice.

Trade-offs:

1. CI is stricter and can feel slower when policy scripts fail on documentation/process drift.
2. Adding or reclassifying crates now requires coordinated updates to graph policy and migration notes.
3. Teams must maintain API baselines and report sections as first-class artifacts.

## DX and Best-Practice Guidance

To keep this ADR effective for contributors:

1. Update docs/src/architecture/component-graph.json in the same PR as any crate-boundary change.
2. If a crate class or stability tier changes, include rationale and migration impact in the PR description.
3. Keep operation spans aligned with discover/negotiate/transfer/materialize/finalize for new heavy paths.
4. Prefer failing fast with clear script output instead of silent policy drift.
5. Keep CI gates deterministic and file-based so they work in clean CI checkouts.

## Residual Gaps and Next Actions

Implemented policy is strong, but two high-value follow-ups remain:

1. Automate review-throughput KPI collection in monthly reports (open PR age, median turnaround, stale queue).
2. Publish benchmark trend artifacts with evidence links as part of monthly report hydration.

## References

1. docs/src/architecture/decisions/005-gitoxide-report-extraction.md
2. docs/src/architecture/component-graph.json
3. .github/workflows/ci.yml
4. scripts/ci/check-api-drift.sh
5. scripts/ci/enforce-layering.sh
6. scripts/ci/detect-arch-drift.sh
7. scripts/ci/lint-reports.sh
8. crates/arc-core/src/ops.rs
9. crates/arc-fs-ops/
