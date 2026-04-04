# Performance and Maintenance Runbook

Status: Stable
Audience: SREs, platform engineers, and monorepo maintainers

This runbook defines operational maintenance for Arc repositories at enterprise scale.

## CAS Architecture and Why It Differs from Git GC Heuristics

Arc storage model is immutable, content-addressed CAS, not packfile mutation cycles.

Key characteristics:

- BLAKE3 identity for Change and blob references.
- Two-level shard layout for change objects: `.arc/store/<00..ff>/<rest-of-hash>`.
- Blob store keyed by full hash in `.arc/blobs/`.
- Zero-copy reads for larger objects via `memmap2`.

Operational consequence:
Arc does not rely on Git-style repacking heuristics to maintain correctness. Maintenance focuses on reclaiming unreachable, causally stable objects rather than repacking mutable packs.

## Maintenance Operations

| Operation | Command | Primary effect | Safety property | Recommended cadence |
|---|---|---|---|---|
| Reachability cleanup | `arc gc` | Removes unreachable, causally stable changes and orphaned blobs | Protects view heads and OpLog-referenced heads | Weekly on active repos; after branch/view cleanup |
| History compaction | `arc compact` | Collapses causally stable history into a genesis base and updates epoch map | No live Change mutation; compatibility via epoch remap | Monthly or release-window only |
| Provenance verification | `arc verify` | Validates graph cryptographic consistency | Detects tampered/invalid Change signatures | Before/after maintenance windows |

## Safe Thresholds for Enterprise Operations

Use these thresholds as conservative default policy:

1. Run `arc gc` when any of the following is true:
   - `.arc/store` grows > 20% week-over-week.
   - Large feature branch retirement completed.
   - CI cache pressure increases due to stale CAS objects.
2. Run `arc compact` only when all of the following are true:
   - Repository is in a planned maintenance window.
   - No active incident on sync/history paths.
   - Operators validated rollback path and post-compact verification plan.
3. Always execute `arc verify` before and after `gc`/`compact`.

## Standard Maintenance Procedure

```sh
# 1) Pre-check
arc verify
arc status

# 2) Reclaim unreachable stable objects
arc gc

# 3) Optional scheduled compaction
arc compact

# 4) Post-check
arc verify
arc status
```

If post-check fails, stop and collect traces before additional mutation.

## Profiling Massive Repositories with Structured Trace

Use event traces to isolate I/O bottlenecks, command latency spikes, and sync-path regressions.

```sh
ARC_TRACE_EVENT=./arc-trace.jsonl arc pull origin main
ARC_TRACE_EVENT=./arc-trace.jsonl arc gc
```

What to inspect:

- Long gaps between sync stages (network/storage contention).
- Repeated fetch/retry patterns (remote health or transport instability).
- GC duration growth over time (root-set expansion or retention policy drift).

For interactive triage:

```sh
ARC_TRACE=1 arc pull origin main
```

## Capacity Planning Notes

1. Treat `.arc/store` growth and `.arc/blobs` growth as separate signals.
2. Prefer regular `arc gc` over infrequent heavy maintenance events.
3. Keep trace artifacts for failed maintenance jobs to support trend analysis.

## Escalation Checklist

Escalate to platform owners when any condition persists after one clean maintenance cycle:

- `arc verify` failure
- Recurrent missing-blob behavior after fetch/pull
- Sustained maintenance runtime regression across two or more cycles
