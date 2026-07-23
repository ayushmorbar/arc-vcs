# AGENTS.md for arc-vcs

You are working in `arc-vcs`, a Rust workspace for a semantic, AST-aware,
content-addressed, replayable Spacetime DAG version control system.

## Mission

Help build and operate `arc` as a semantic VCS, not as a Git clone with
different branding.

## Non-negotiable model

- Changes are typed semantic atoms over syntax trees, not line patches.
- Repository content is immutable in BLAKE3-addressed CAS.
- Operation metadata and indexes may be stored in Redb, but object blobs stay in
  raw CAS paths.
- AI output is advisory until verified and, when required, explicitly sponsored.
- Deterministic, auditable behavior is preferred over convenience.

## Pipeline taxonomy

Map substantial workflows to these stages:

1. discover
2. negotiate
3. transfer
4. materialize
5. finalize

Errors, telemetry, and status reporting should preserve stage context.

## Architectural boundaries

- Keep pure math, algebra, graph, and patch-theory logic free of filesystem,
  network, process, and wall-clock I/O.
- Boundary crates may perform I/O; pure crates must remain deterministic and
  wasm-friendly where intended.
- Never introduce legacy line-diff assumptions into semantic workflows.
- Never fabricate semantic results; if semantic logic is unavailable, emit
  explicit scaffold markers such as `semantic-unavailable`.

## Core agent behaviors

When proposing or implementing changes:

- Prefer semantic operations and AST-aware reasoning.
- Preserve crate layering and repository purity boundaries.
- Avoid hidden global state.
- Keep cross-crate contracts explicit and typed.
- Validate external inputs at boundaries.
- Treat migrations, epochs, and serialized schema changes as first-class design
  concerns, not incidental implementation details.

## Commands

- `cargo test -p <crate-name>`
- `cargo check -p gix`
- `just check`
- `just test`
- `cargo fmt`
- `cargo clippy --workspace --all-targets -- -D warnings -A unknown-lints --no-deps`

## Build targets

Release binaries are built for:

| Target | OS | Runner |
|---|---|---|
| `x86_64-apple-darwin` | macOS Intel | `macos-latest` |
| `aarch64-apple-darwin` | macOS Apple Silicon | `macos-latest` |
| `x86_64-unknown-linux-gnu` | Linux glibc x86_64 | `ubuntu-24.04` |
| `aarch64-unknown-linux-gnu` | Linux glibc aarch64 | `ubuntu-24.04` |
| `x86_64-unknown-linux-musl` | Linux musl x86_64 | `ubuntu-24.04` |
| `x86_64-pc-windows-msvc` | Windows x86_64 | `windows-latest` |

Cross-compilation uses `cargo-cross` for non-native targets. See
`scripts/ci/install-cross-tools.sh` and `scripts/ci/build-release.sh`.

## Before finishing work

- Run the narrowest useful validation first, then widen scope.
- Keep pure crates free of new side effects.
- Call out any schema, network epoch, or serialization impact explicitly.
- State uncertainty clearly instead of guessing.