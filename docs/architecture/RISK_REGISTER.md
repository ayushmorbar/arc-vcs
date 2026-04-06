# Architecture Risk Register

## Metadata

- Scope: arc architecture and platform risks
- Last updated: 2026-04-07
- Review cadence: monthly (engineering report) and annual strategy review
- Status: Active

This register tracks top architectural risks and turns them into explicit operational controls.

## Active Risks

| ID | Risk | Impact | Likelihood | Owner | Early Indicators | Trigger for Escalation | Mitigation Plan | Status |
|---|---|---|---|---|---|---|---|---|
| AR-001 | State explosion in CRDT spacetime DAG | High: unbounded graph and CAS growth can degrade storage economics and replay latency. | Medium-High | core-architecture | Rising node cardinality per repository, increasing replay p95, storage growth outpacing change volume | Monthly growth exceeds budget for two cycles or replay SLO breach trend appears | Compaction lanes, retention windows, and periodic cardinality audits tied to CI/reporting | Monitoring |
| AR-002 | Partition convergence latency | High: long partitions can delay convergence and cause operator-visible inconsistency windows. | Medium | networking | Increased sync retries, slow transfer/materialize stage times, prolonged divergence windows | Convergence SLO breach in representative partition simulations or production traces | Strengthen anti-entropy scheduling, stage telemetry, and deterministic replay tests under partition scenarios | Monitoring |
| AR-003 | Wasm runtime resource constraints | High: memory/CPU limits can cause degraded throughput or OOM in heavy graph/materialization paths. | Medium | platform-runtime | wasm benchmarks regress, memory ceilings reached, feature fallbacks frequently invoked | Repeated wasm build/test instability or benchmark regressions above agreed threshold | Keep memory-bounded algorithms, wasm fixtures, wasm build gates, and explicit native-only feature controls | Monitoring |

## Governance Rules

1. Every active risk must have an owner, indicator set, and escalation trigger.
2. Risk score changes must be reflected in monthly reports with evidence links.
3. Mitigation progress must be mapped to roadmap workstreams where relevant.
4. New critical risks are added before implementation work starts, not after incidents.

## Reporting Integration

1. Monthly report includes risk register delta: added, resolved, re-scored, or escalated risks.
2. Annual strategy includes top 3 risks with mitigation effectiveness review.
3. Incident postmortems must reference related risk IDs when applicable.

## Current Gaps

1. Some early indicators are not yet fully automated in monthly report generation.
2. Risk trend charts are not yet published as standard report artifacts.
