---
title: "ADR-005: Gitoxide Report Extraction and Replication Policy"
description: "Decision record for how external engineering reports are translated into enforceable architecture, CI, and governance actions in arc."
status: Implemented and Archived
date: 2026-04-07
scope: v0.1 external-signal extraction ledger
note: "Historical ADR preserved as the source rationale for replication and governance policy."
---

# ADR 005 - External Report Extraction to Internal Policy

| Field        | Value                                                          |
| ------------ | -------------------------------------------------------------- |
| Status       | Implemented and Archived                                       |
| Date         | 2026-04-07                                                     |
| Deciders     | arc core architecture and governance maintainers               |
| Source Input | Gitoxide monthly and annual reports, component graph artifacts |

## Context

arc needed a repeatable way to learn from external project execution quality without copying implementation details blindly.

The previous ledger captured many useful observations, but it was hard to operate as policy because:

1. Findings were very detailed but not grouped into stable decision categories.
2. Replication actions were mixed with historical notes.
3. Teams lacked a concise checklist for turning outside signals into arc-native changes.

## Decision

arc adopts a formal extraction-to-policy method for external engineering reports:

1. Classify every signal into one of five policy domains: architecture, reliability, security, release discipline, maintainer throughput.
2. Keep only transferable patterns; avoid source-specific coupling.
3. Promote adopted patterns into enforceable controls first (CI, scripts, typed contracts), docs second.
4. Require evidence links for report claims and governance statements.
5. Treat review throughput and maintenance work as first-class delivery outcomes.

## Synthesized Patterns Adopted in arc

The report extraction led to these durable arc patterns:

1. Architecture as machine-readable contract via component graph metadata and layering policy.
2. Stage-oriented operation model: discover, negotiate, transfer, materialize, finalize.
3. Deterministic policy gates for drift and reporting quality.
4. Explicit platform-foundation boundaries for lock/temp/atomic file operations.
5. Baseline-first quality model for API compatibility, benchmarks, and parser/protocol robustness.

## Implemented State (Verified)

The extraction policy outcomes are reflected in active repository controls:

1. Architecture metadata and ownership tiers in docs/src/architecture/component-graph.json.
2. CI layering enforcement in scripts/ci/enforce-layering.sh.
3. API drift gate in scripts/ci/check-api-drift.sh with baselines in docs/architecture/api-baselines/.
4. Architecture drift summary in scripts/ci/detect-arch-drift.sh.
5. Monthly report quality lint in scripts/ci/lint-reports.sh.
6. Stage taxonomy and SLO hooks in crates/arc-core/src/ops.rs.

## Consequences

Positive:

1. External learning is now operationalized through objective checks, not only narrative retrospectives.
2. Contributors can map strategy claims directly to code, scripts, and CI jobs.
3. Decision quality improves because trade-offs are explicit and revisitable.

Trade-offs:

1. Documentation and reporting now carry stronger maintenance burden.
2. Some improvements remain process-heavy until full automation exists.
3. Strict gates can raise short-term friction for rapid experimentation.

## DX and Best-Practice Guidance

1. When adding a new external lesson, write the policy delta first, then add implementation notes.
2. Prefer one clear measurable control over multiple qualitative recommendations.
3. Use the same wording in docs, CI checks, and release gates to avoid semantic drift.
4. Preserve historical depth in appendices, but keep active policy concise.

## Historical Follow-Through Handoff

The following items were identified by this ADR and are now tracked in active planning/reporting documents (roadmap and monthly/annual reports):

1. Automate maintainer throughput KPI ingestion in monthly reporting.
2. Publish benchmark and fuzz trend artifacts with stable cadence and evidence links.
3. Add explicit risk-register delta publication to each monthly report.

## References

1. docs/src/architecture/decisions/004-gitoxide-architecture-study.md
2. docs/src/architecture/component-graph.json
3. .github/workflows/ci.yml
4. scripts/ci/enforce-layering.sh
5. scripts/ci/check-api-drift.sh
6. scripts/ci/detect-arch-drift.sh
7. scripts/ci/lint-reports.sh
