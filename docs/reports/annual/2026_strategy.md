# Arc 2026 Strategy (Year Zero)

## Metadata

- Year: 2026
- Report owner: Arc Strategic Lead
- Generated on: 2026-04-07
- Confidence level: medium
- Report status: baseline year strategy and governance checkpoint

## North Star

Total Git Independence and SOTA Agentic Autonomy.

## 1. Year in Numbers

| Metric | Value | Previous Year | Delta |
| ------ | ----: | ------------: | ----: |
| Total commits | Baseline setup | N/A | N/A |
| Active contributors | Founding team | N/A | N/A |
| Crates maintained | 27 | N/A | N/A |
| Security advisories | 0 known unresolved | N/A | N/A |
| CI success rate | Baseline tracked in CI | N/A | N/A |

Notes:

1. 2026 is a baseline year; several metrics are qualitative until monthly automation matures.
2. Metrics without numeric history should be treated as instrumentation gaps, not success claims.

## 2. Strategic Outcomes

- Major wins:
  - P0 and P1 governance guardrails formalized as code and policy.
  - Architecture layering and release preflight controls wired into automation.
  - Parser fuzzing and benchmark trend scaffolding established.
- Major misses:
  - Full production-grade distributed convergence SLO instrumentation is not yet complete.
  - Cross-platform fuzz smoke execution remains environment-dependent.
- Most impactful architectural decisions:
  - Codifying operation-stage taxonomy across heavy paths.
  - Treating architecture metadata as machine-readable contract.

DX interpretation:

1. Contributors now benefit from stronger guardrails and clearer failure surfaces.
2. Remaining friction is mainly around incomplete telemetry automation and environment parity.

## 3. Plan vs Reality

| Objective | Planned | Actual | Status | Why |
| --------- | ------- | ------ | ------ | --- |
| Governance as code | CI/release policy enforcement | Implemented in P0/P1 | on-track | Directly integrated into workflow gates |
| Parser hardening | Fuzzing baseline | Initial target shipped | at-risk | Needs stable scheduled execution matrix |
| Performance visibility | Benchmark governance | Baseline criterion bench added | on-track | Ready for trend tracking cycle |

Key lesson:

1. Baseline scaffolding is useful, but strategy confidence requires trend artifacts and stable collection cadence.

## 4. Security Posture Review

- Threat model updates completed: foundational controls implemented; quarterly review pending.
- Incident response effectiveness: no major incidents recorded in currently tracked project artifacts for this period.
- Vulnerability classes observed: dependency and workflow hardening focus areas.
- Preventive controls added: code scanning, dependency policy checks, release preflight gates.
- Evidence links: .github/workflows/, deny.toml, docs/architecture/ (baseline period; full SLA dashboards pending).

Security follow-through for next cycle:

1. Publish SLA timing metrics even when incidents are zero (explicit zero-state reporting).
2. Add quarterly threat-model review evidence references.

## 5. Quality and Reliability Review

- Top recurring bug categories: integration and environment parity issues.
- Mean time to fix: baseline collection initialized; directional trend assessment pending additional monthly samples.
- Regression trends: no persistent critical regression class identified in currently tracked baseline data.
- Largest correctness lessons: encode architecture contracts into CI, not prose.
- Evidence links: CI workflows, policy tests, architecture docs (quantitative rollups begin with monthly reporting cadence).

Reliability follow-through for next cycle:

1. Convert directional statements into numeric trend deltas as data matures.
2. Tie reliability outcomes to specific risk IDs from the architecture risk register.

## 6. Fuzzing and Parser Robustness

- Fuzzing coverage trend: baseline established with arc-lang parser target.
- Top finding classes: to be measured in quarterly fuzz runs.
- Critical unresolved fuzz findings: none reported yet.
- CI/scheduled fuzz job status: smoke command defined; full scheduled lane pending host standardization.
- Regression tests derived from fuzz findings: none yet.
- Evidence links: fuzz/Cargo.toml, fuzz/fuzz_targets/fuzz_lang_parser.rs.

Fuzzing maturity goal:

1. Move from baseline presence to scheduled freshness and finding-class trend reporting.

## 7. Performance Review

- Biggest improvements: benchmark scaffolding added for core CAS operations.
- Biggest regressions: none confirmed in this planning snapshot.
- Benchmark governance maturity: baseline complete, monthly deltas pending.
- Evidence links: crates/arc-core/benches/core_ops.rs, just bench-trend.

Performance maturity goal:

1. Include per-stage latency deltas for high-cost operations, not only aggregate benchmark commentary.

## 8. Release and Supply Chain Review

- Release cadence consistency: preflight controls now enforce consistency.
- Automation maturity: improving via layered CI controls.
- Dependency/supply-chain hardening status: deny policy and scanning in place.
- Evidence links: release workflow, CI workflow, deny policy.

Release maturity goal:

1. Publish release gate outcomes with pass/fail trend and remediation lead time.

## 9. Maintainer and Community Health

- Review throughput trend: KPI collection scaffold exists; trending begins this cycle.
- Bus-factor and ownership notes: owner metadata defined in component graph.
- Contributor onboarding outcomes: improved via formalized development lanes.
- Evidence links: component graph, KPI scripts, development docs.

Maintainer health goal:

1. Turn review throughput into first-class KPI with monthly trend and backlog thresholds.

## 10. Risks Entering Next Year

| Risk ID | Risk | Impact | Likelihood | Owner | Mitigation |
| ---- | ---- | ------ | ---------- | ----- | ---------- |
| AR-001 | State explosion under high write volume | High | Medium-High | core-architecture | Compaction policies + storage audits |
| AR-002 | Partition convergence lag under real-world networks | High | Medium | networking | Anti-entropy controls + convergence SLOs |
| AR-003 | Wasm runtime limits on heavy operations | High | Medium | platform-runtime | Wasm-specific profiling and bounded execution modes |

Risk governance note:

1. These risks should stay synchronized with docs/src/architecture/RISK_REGISTER.md IDs and scoring.

## 11. Next-Year Bets

- Bet 1: Autonomous Conflict Resolver
  - Expected outcome: materially reduced manual merge burden.
  - Success metric: >70% of eligible conflicts auto-resolved without regression.
  - Deadline window: 2026 H2.
  - Rollback condition: correctness degradation above agreed threshold.
- Bet 2: Fully replayable distributed sync telemetry
  - Expected outcome: deterministic diagnosis of convergence and causality issues.
  - Success metric: end-to-end stage visibility for top sync workflows.
  - Deadline window: 2026 H2.
  - Rollback condition: overhead exceeds budgeted latency envelope.
- Bet 3: Production-grade wasm parity lane
  - Expected outcome: dependable cross-target core behavior.
  - Success metric: stable wasm validation in standard CI lanes.
  - Deadline window: 2026 Q4.
  - Rollback condition: sustained tooling or runtime blockers.

Execution discipline for bets:

1. Each bet must map to explicit monthly report evidence artifacts.
2. Any missed milestone must include scope cut or mitigation decision in the next report cycle.

## 12. Appendix

- Data extraction method: repository metadata, CI artifacts, and architecture governance files.
- Limitations/confounders: early-stage baseline period with incomplete historical comparisons.
- Evidence links: docs/architecture/, docs/reports/, scripts/, .github/workflows/.

## 13. 2026 Q2 Immediate Actions

1. Automate maintainer KPI ingestion and publish first stable trend line.
2. Add benchmark and fuzz freshness checks to monthly reporting evidence.
3. Publish risk-register delta section in each monthly engineering report.
