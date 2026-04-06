# Copilot Instructions for arc

This repository is `arc`, a Rust 2024 multi-crate workspace for replayable
change graphs, CAS-backed storage, and CLI workflows.

## Architectural Axioms

1. Axiom of Purity
- Math/algebra crates must not perform filesystem, network, process, or clock I/O.
- Keep I/O only in boundary crates (`cli`, `interop`, `network`, storage adapters).

2. Wasm Boundary
- Core computation should remain platform-neutral and wasm-safe.
- Platform-specific code must stay behind explicit boundary modules.

3. Five-Stage Pipeline Taxonomy
- Every heavy operation should map to these stages:
  - `discover`
  - `negotiate`
  - `transfer`
  - `materialize`
  - `finalize`
- Errors and telemetry should include stage context.

## Code and Safety Rules

- Prefer immutable data and explicit transitions over in-place mutation.
- Avoid adding new `unsafe`; if unavoidable, isolate and document invariants.
- Validate all external input at boundaries.
- Keep cross-crate contracts explicit and typed.

## Testing and Verification

- Run targeted crate tests during iteration, then validate workspace integrity.
- Keep `cargo check --workspace` green for all meta/config updates.
- For policy changes, prefer deterministic checks in CI.

## CI and Security Expectations

- Use least-privilege permissions in GitHub workflows.
- Pin third-party actions by commit SHA where practical.
- Keep supply-chain checks (`cargo-deny`, code scanning, action scanning) active.
