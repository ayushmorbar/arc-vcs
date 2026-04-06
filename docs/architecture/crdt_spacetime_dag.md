# Discovery: CRDT Spacetime DAG

Status: initial discovery record

## Goal

Document the architectural intent and invariants of arc's CRDT Spacetime DAG so maintainers and agents can reason about correctness, convergence, and performance trade-offs.

## Problem Space

arc requires a history model that supports:

1. concurrent change authoring without central lockstep coordination
2. deterministic replay and convergence
3. semantic conflict representation instead of opaque textual state
4. durable, auditable operation history with tractable synchronization

## Core Model

- Nodes represent immutable operations/changes.
- Edges encode causality and dependency constraints.
- Materialized state is derived by replay over a valid frontier.
- Conflicts are explicit first-class outcomes, not hidden merge side effects.

## Invariants

1. Change identity is content-derived and immutable.
2. Replay over equivalent dependency sets is deterministic.
3. Frontier updates preserve acyclicity and causality.
4. Invalid or unverifiable operations are rejected before materialization.

## Trust and Safety Boundaries

- Remote histories are untrusted until validated.
- Store/object decoding must be bounded and fail-closed.
- Filesystem writes from materialization must enforce path safety.
- Signature/identity checks gate acceptance of authored operations.

## Operational Concerns

### Consistency

- Readers should observe stable snapshots for a chosen frontier.
- Writers must not publish partially valid graph states.

### Resource Usage

- Graph traversal, cache policy, and snapshot synthesis must scale with large histories.
- Long-lived processes need explicit refresh strategies for evolving stores.

## Open Questions

1. What are the strongest convergence guarantees across partial sync topologies?
2. Which graph indexes should be persisted versus derived lazily?
3. What is the preferred conflict UX for large-scale multi-head frontiers?

## Next Discovery Artifacts

- sync handshake and trust model
- snapshot synthesis consistency contract
- lock and commit publication semantics
