---
name: arc-sync-protocol
description: >
  Rules for the arc-vcs five-stage sync pipeline. Use when implementing
  network sync, replica reconciliation, transfer encoding, or any operation
  that moves change atoms or graph state between arc instances.
---

# arc-sync-protocol

## Purpose
`arc` synchronization is not rsync, git fetch, or file replication.
It is a CRDT-aware causal exchange protocol operating over BLAKE3-addressed
immutable objects and DAG frontier deltas.

## The Five Stages

Every substantial sync operation must map to these stages.
Errors and telemetry must include their stage context.

### 1. discover
Determine what the remote knows.
- Exchange Frontier hashes (leaf DAG nodes), not HEAD pointers.
- Compute the symmetric difference of the two frontiers.
- Do not transfer objects yet.
- Output: a set of missing BLAKE3 hashes on each side.

### 2. negotiate
Agree on what will be transferred.
- Confirm which missing hashes are needed.
- Check for epoch compatibility before proceeding.
- If network epochs differ, reject the session cleanly.
- Output: a validated, prioritized transfer manifest.

### 3. transfer
Move immutable objects.
- Transfer only the BLAKE3-addressed blobs agreed in the manifest.
- Objects are immutable; never patch or delta-encode the blob payload.
- Verify the BLAKE3 hash of every received object before persisting.
- Output: locally stored, verified CAS objects.

### 4. materialize
Integrate transferred objects into the local causal graph.
- Apply incoming OpRecords to the local DAG using CRDT merge rules.
- If concurrent changes conflict at the AST level, emit a Conflictor node.
- Do not reorder received history to fit local chronology.
- Output: updated local DAG with new frontier.

### 5. finalize
Commit the updated state.
- Persist the new Frontier to the Redb index.
- Update any local caches or derived views.
- Emit telemetry with stage context and object counts.
- Output: consistent, stable local repository state.

## Hard Rules
- Never use line-diff delta encoding for object transfer.
- Never trust wall-clock ordering for causal sequencing.
- Never silently drop or rewrite received objects.
- Always validate epoch compatibility before materialization.
- A failed stage must leave the repository in a consistent prior state.