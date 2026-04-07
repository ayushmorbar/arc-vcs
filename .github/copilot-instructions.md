# Copilot Instructions for arc-vcs

You are assisting in `arc-vcs`, a Rust 2024 workspace for a semantic,
AST-aware, content-addressed Spacetime DAG version control system.

## Core model

- `arc` is not a line-diff VCS.
- Prefer semantic operations over text-patch reasoning.
- Changes are typed semantic atoms over syntax trees.
- Content is immutable in BLAKE3 CAS.
- Metadata and indexes may be stored separately from blobs.

## Hard constraints

- Do not suggest Myers diff, line-number patching, or regex-based diff logic
  for core semantic workflows.
- Do not introduce filesystem, network, process, or clock I/O into pure crates.
- Do not add hidden global mutable state to represent repository state.
- Do not fabricate semantic outputs when parser or classifier support is absent.

## Layering

- Keep pure algebra, graph, and patch logic free of boundary I/O.
- Keep platform-specific and I/O-heavy code in boundary crates.
- Preserve explicit typed contracts across crates.

## Storage

- Use `blake3` for canonical hashing.
- Keep immutable blobs in CAS paths.
- Use Redb for structured metadata and indexes where appropriate.
- Treat canonical encoding changes as compatibility-sensitive.

## AI-specific rules

- AI-generated or AI-resolved changes remain advisory until verified.
- Unverified autonomous work should remain provisional.
- Preserve provenance, auditability, and deterministic validation.

## Coding style

- Favor explicit invariants over clever shortcuts.
- Prefer narrow, auditable changes.
- Use `thiserror` for library boundaries and `anyhow` for application layers
  when appropriate.
- Avoid `unsafe`; if unavoidable, isolate it and document invariants.

## Validation

Prefer the narrowest useful validation first:

- `cargo test -p <crate-name>`
- `cargo check -p gix`
- `just check`
- `just test`
- `cargo fmt`
- `cargo clippy --workspace --all-targets -- -D warnings -A unknown-lints --no-deps`

## When uncertain

Choose deterministic, auditable behavior over convenience and state uncertainty
explicitly.