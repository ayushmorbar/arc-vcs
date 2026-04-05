---
title: Disaster Recovery
description: Documentation page for Disaster Recovery.
---

# Disaster Recovery Runbook

Status: Stable
Audience: SREs, incident commanders, and repository administrators

This runbook defines how to respond to perceived data-loss events in Arc repositories.

## Philosophy

Arc is designed so that recovery is pointer-oriented, not rewrite-oriented:

- CAS objects are immutable and content-addressed by BLAKE3.
- View movement is logged in the local OpLog with `before_heads` and `after_heads`.
- `arc undo` restores prior view heads in O(1) without deleting history.

Operational implication:
Most "lost work" incidents are state-navigation incidents, not irreversible data destruction.

## Immediate Triage Protocol

1. Freeze mutation commands (`snap`, `view merge`, `compact`) until evidence is collected.
2. Capture current state:

```sh
arc status
arc op log
arc verify
```

3. If behavior is unclear, rerun the failing command with trace enabled:

```sh
ARC_TRACE=1 arc <command> <args>
```

4. Only then execute corrective actions.

## Incident Taxonomy Matrix

| Incident class                   | Typical symptom                                                          | Primary command path                                            | Safety model                                  | Escalation trigger                            |
| -------------------------------- | ------------------------------------------------------------------------ | --------------------------------------------------------------- | --------------------------------------------- | --------------------------------------------- |
| Accidental merge / bad state     | Working tree changed unexpectedly after merge/cherry-pick                | `arc op log` -> `arc undo`                                      | O(1) pointer rollback; CAS unchanged          | Undo does not restore expected state          |
| CAS corruption or missing object | Verify/read errors, missing blob behavior during materialization or sync | `arc verify`, then `arc fetch`/`arc pull` from healthy peer     | Content-addressed rehydration from remote CAS | Repeated hash/read mismatch after clean fetch |
| Stale conflict metadata          | Resolver flow reports pending conflict after manual cleanup              | inspect/remove `.arc/conflict` only after snapshot sanity check | Metadata cleanup only; no DAG rewrite         | Conflict state repeatedly reappears           |

## Playbook A: Accidental Merge or Bad State

1. Inspect recent operations:

```sh
arc op log
```

2. Roll back one operation:

```sh
arc undo
```

3. Validate resulting state:

```sh
arc status
arc log
```

4. Repeat `arc undo` only if the operator confirms the prior operation is also undesired.

Notes:

- `arc undo` changes view heads; it does not delete underlying Change objects.
- If rollback needs to be auditable as a forward change, use `arc revert` after restoration.

## Playbook B: CAS Corruption / Missing Blob Recovery

Important scope:

- `arc verify` validates graph cryptographic provenance (Change id/signature consistency).
- Blob integrity relies on BLAKE3-addressed storage and hash-checked ingestion paths.

Procedure:

1. Baseline verification:

```sh
arc verify
```

2. Rehydrate from a known-good remote:

```sh
arc remote list
arc fetch origin main
arc pull origin main
```

3. Re-verify and rematerialize target view:

```sh
arc verify
arc status
```

4. If object mismatch persists, capture traces and stop write operations:

```sh
ARC_TRACE=1 arc pull origin main
ARC_TRACE_EVENT=./arc-trace.jsonl arc pull origin main
```

5. Open incident with trace artifacts and environment details.

## Playbook C: Stale Metadata from Interrupted AI Resolution

Symptom:
Resolver commands indicate pending conflict/AI state although manual resolution was already snapped.

Procedure:

1. Confirm repository state is already correct (`arc status`, `arc diff`).
2. If manual resolution is snapped and no unresolved conflict remains, remove stale marker:

```sh
# Run from the repository shared root (the directory that contains `.arc/`).

# PowerShell
Remove-Item .arc\conflict -ErrorAction Stop

# POSIX shell
rm -f .arc/conflict
```

3. Re-run status and intended command.

Guardrail:
Never delete `.arc/conflict` before validating that desired file state is already captured or intentionally preserved.

## Forensics Before Mutation

Use this capture sequence before making corrective changes in high-severity incidents:

```sh
arc status
arc op log
arc verify
ARC_TRACE=1 arc status
```

For machine-parseable incident timelines:

```sh
ARC_TRACE_EVENT=./arc-trace.jsonl arc <repro-command>
```

Attach `arc-trace.jsonl` and command transcript to the incident record.
