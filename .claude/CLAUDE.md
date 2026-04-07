# CLAUDE.md for arc-vcs

This file contains always-on instructions for contributors working inside the
`arc-vcs` repository.

See [AGENTS.md](AGENTS.md) for the broader agent contract and [.github/copilot-instructions.md](../.github/copilot-instructions.md)
for Copilot-specific guidance.

## Project summary

`arc-vcs` is a Rust 2024 multi-crate workspace for semantic version control over
syntax trees, BLAKE3 CAS storage, replayable change graphs, and CLI workflows.

## Always-on rules

- Treat `arc` as a semantic DAG VCS, not a line-diff Git wrapper.
- Keep pure crates free of filesystem, network, process, random, and clock I/O.
- Preserve crate boundaries and explicit typed contracts.
- Prefer deterministic, auditable behavior over convenience.
- Never invent semantic-diff or patch-theory behavior that is not implemented.
- If logic is missing, leave explicit markers and scaffolding rather than fake
  completion.

## Pipeline taxonomy

When useful, model major flows as:

1. discover
2. negotiate
3. transfer
4. materialize
5. finalize

## Validation commands

- `cargo test -p <crate-name>`
- `cargo check -p gix`
- `just check`
- `just test`
- `cargo fmt`
- `cargo clippy --workspace --all-targets -- -D warnings -A unknown-lints --no-deps`

## Skills

Use project skills when relevant:

- `arc-patch-theory` for AST operations, commutativity, and Change semantics
- `arc-cas-storage` for CAS paths, hashing, mmap, and persistence rules
- `arc-semver-policy` for crate versions, schema epochs, and protocol bumps
- `arc-git-commit` for commit message generation
- `arc-docs` for docs migration, templates, and audience-aware writing
- `arc-causal-graph` for frontier/causality/convergence reasoning
- `arc-redb-indexes` for metadata/index transaction design
- `arc-tree-sitter` for parser/lifetime/query constraints
- `arc-intent-reasoning` for semantic decomposition of large changes
- `arc-semantic-query` for intent-search and vibe query behavior
- `arc-sync-protocol` for discover/negotiate/transfer/materialize/finalize flows
- `arc-ghost-nodes` for provisional agent checkpoints and sponsorship transitions
- `arc-property-testing` for invariants and property-based law verification

## Working style

- Read the nearest relevant files before editing.
- Make small, auditable changes.
- Prefer explicit invariants over implicit coupling.
- Explain schema and epoch consequences when serialized structs change.