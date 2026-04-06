---
title: 003 Crdt Over Ot
description: Documentation page for 003 Crdt Over Ot.
---

# ADR 003 — CRDT + Commutativity over Operational Transform

| Field | Value |
|---|---|
| **Status** | Accepted |
| **Date** | 2026-03-02 |
| **Deciders** | arc core team |

---

## Context

arc must support distributed collaboration: two or more peers each make changes to the same repository, then synchronise. The system must produce a consistent, correct final state regardless of the order in which changes arrive.

Two families of algorithms address this problem:

**Operational Transform (OT):**
- Requires a central server to establish a canonical ordering of concurrent operations.
- The transformation algebra (generating all `T(op1, op2)` and `T(op2, op1)` functions) grows as $O(n^2)$ with the number of operation types.
- Used by Google Docs, Etherpad, and early collaborative editors.
- Cannot work peer-to-peer without a central authority.

**Conflict-free Replicated Data Types (CRDTs):**
- Require no central coordinator.
- The data structure itself is designed so that any two replicas can be merged via a join operation defined on a lattice.
- Used by Riak, Redis Cluster, Automerge, Yjs, and decentralised protocols broadly.
- The correctness proof follows from the lattice laws, not from complex per-operation transforms.

arc's specific approach combines CRDTs with commutative patch theory:
- The **ChangeGraph** with BLAKE3-addressed nodes is a grow-only CRDT (objects are only added, never mutated or deleted from the authoritative log).
- View head sets are **join-semilattices** under set union.
- Rather than defining OT transforms, arc defines `commutes(a, b)` — a binary predicate on `Atom` pairs.
- If all pairs commute, the merge is automatically conflict-free. If any pair does not commute, the conflict is surfaced for human (or AI) resolution. There are no silent incorrect merges.

---

## Decision

Use a **CRDT join-semilattice on view head sets** combined with an **AST-level commutativity predicate** for change merging, rather than Operational Transform.

There is no central server required for correctness. arc nodes are fully peer-to-peer. `arc-net` acts as a dumb object server (read-only HTTP); merging logic runs entirely on the client.

---

## Consequences

**Positive:**
- No single point of failure. arc works correctly even if peers never connect to a central server.
- Merge correctness follows from the lattice laws (join-semilattice associativity, commutativity, idempotency) — formally provable.
- The commutativity check is $O(|\Delta A| \times |\Delta B|)$ — quadratic in the size of the concurrent change sets. For typical feature-branch merges (tens to hundreds of changes), this is fast.
- Sync is purely additive: no mutations to existing objects, no rebases, no rewrite of history.

**Negative:**
- The commutativity check is $O(|\Delta A| \times |\Delta B|)$. In pathological cases — two developers each accumulating thousands of changes that all touch the same files — this becomes slow. Documented in [SHORTCOMINGS.md](../../../SHORTCOMINGS.md).
- arc has no "interactive rebase" equivalent. History is always cumulative. Documented in [SHORTCOMINGS.md](../../../SHORTCOMINGS.md#6-no-interactive-rebase).
- The `arc-net` server is currently pull-only: remote pushes require the remote to also run `arc-net`. Full bidirectional mesh topology is a future milestone.

---

## References

- Shapiro et al., "A Comprehensive Study of Convergent and Commutative Replicated Data Types" (2011)
- Kleppmann, "Designing Data-Intensive Applications", Chapter 9
- Automerge CRDT: [https://automerge.org/](https://automerge.org/)
- Pijul's CRDT patch theory: [https://pijul.org/model](https://pijul.org/model)
- [CRDT Sync Design Doc](../../design/crdt_sync.md)
