---
title: ADR 003 Git Bridge
description: Documentation page for ADR 003 Git Bridge.
---

# ADR 003 - Pure Local DAG with Translation on the Wire

| Field        | Value         |
| ------------ | ------------- |
| **Status**   | Accepted      |
| **Date**     | 2026-04-04    |
| **Deciders** | arc core team |

---

## Context

arc stores history as BLAKE3-addressed, Ed25519-signed semantic changes. Most external hosting ecosystems (for example GitHub/GitLab) speak Git objects and Smart HTTP.

A direct migration to Git internals locally would weaken arc's core guarantees:

- SHA-1 object identity would leak into local storage semantics.
- Local `.git` object management would duplicate state and complexity.
- arc's algebraic graph model would be constrained by Git snapshot assumptions.

We need interoperability without sacrificing local architecture.

---

## Decision

arc follows a **Pure Local, Translation on the Wire** architecture:

- Local state remains exclusively arc-native (BLAKE3 + Ed25519 + algebraic DAG).
- During `arc push <url>`, arc performs just-in-time translation of selected arc state into ephemeral Git objects:
  - materialize the target view,
  - compile tree/blob/commit payloads,
  - encode a packfile,
  - send via Git Smart HTTP (`info/refs?service=git-receive-pack` + `git-receive-pack`).
- Translation artifacts are transient and are not persisted as a local `.git` repository.

This yields compatibility with Git remotes while preserving arc-native local security and semantics.

---

## Consequences

**Positive:**

- Native push compatibility to Git hosting without polluting local storage.
- arc keeps BLAKE3/Ed25519 trust boundaries for all local operations.
- Bridge implementation remains dependency-light and auditable.
- Future transport backends can reuse the same translation boundary.

**Negative:**

- Push path must faithfully map arc semantics into Git's snapshot model.
- Bridge correctness requires careful protocol and packfile testing.
- Some arc-native metadata must be projected into commit trailers for round-trip provenance.

---

## Security Notes

- Local object integrity and identity remain BLAKE3-addressed.
- Local author provenance remains Ed25519-signed.
- Git translation commits embed arc provenance trailers (`Arc-Change-Id`, author type, signature metadata) so exports remain auditable.

---

## References

- [Network Transport](network_transport.md)
- [CRDT Network Sync](crdt_sync.md)
- [README](../../../README.md)
