# Vision: arc in the Agentic Era (2026)

`arc` is engineered as a verifiable, AI-native version control substrate for long-lived software systems. This document defines the architectural standards we apply to keep the platform reliable under autonomous and human collaboration.

## 1. Architectural Sovereignty

We build `arc` as a set of vertical slices with strict boundaries.

- **Vertical Slice Architecture:** features ship end-to-end with explicit ownership from CLI/API surface to storage and sync semantics.
- **Loose Coupling:** crates and modules communicate through narrow interfaces, minimizing incidental dependency chains.
- **Strictly Typed Interfaces:** domain contracts are encoded in types first, not conventions, so invalid operations fail at compile time whenever possible.

## 2. Systems Integrity

Correctness under failure is a hard requirement.

- **Crash-Consistency:** every write path is designed so abrupt termination cannot leave silent semantic corruption.
- **Atomic Writes:** state transitions commit as all-or-nothing units.
- **Write-Ahead Logging (WAL):** mutation intent is durably recorded before commit points.
- **Fail-Fast Idempotency:** retried operations converge to one valid outcome without duplicating side effects.

## 3. Hardware-Aware Optimization

`arc` treats performance as a systems concern, not a post-hoc optimization pass.

- **Zero-Copy Abstractions:** `memmap2` paths avoid unnecessary buffer churn and leverage OS page cache behavior.
- **Async I/O by Default:** `tokio`-based concurrency keeps sync and service paths responsive under load.
- **BLAKE3 Everywhere It Matters:** high-throughput cryptographic hashing underpins identity, integrity, and scalable CAS operations.

## 4. Agentic-Ready Codebase

Autonomous software agents must be able to reason about `arc` without guesswork.

- **Self-Documenting Structure:** module layout mirrors domain language so intent is discoverable from code shape.
- **NewType-Driven Domain Modeling:** impossible states are made unrepresentable through strongly typed wrappers.
- **Formal Verifiability Orientation:** algebraic invariants and deterministic transformations are preferred over implicit heuristics.

## 5. Hyper-Observability and Self-Healing

A distributed history engine must explain itself under stress.

- **Structured Logging:** machine-parseable events capture causality across storage, sync, and resolution paths.
- **Causal Analysis:** operational traces are designed for rapid root-cause reconstruction, not ad hoc debugging.
- **Self-Healing Hooks:** recovery workflows are designed around deterministic replay and bounded remediation.

## 6. Trustless Security

Security is embedded at architecture time.

- **Zero Trust by Default:** every boundary is explicit; no implicit network or process trust assumptions.
- **Shift-Left Security:** threat modeling and hardening requirements are integrated into design and implementation phases.
- **Ed25519 Verifiability of the AST:** semantic change history remains cryptographically attestable from atom to change closure.

## Non-Negotiable Outcome

`arc` is not only a better VCS UX. It is a resilient, cryptographically verifiable coordination engine for human and agentic development at scale.
