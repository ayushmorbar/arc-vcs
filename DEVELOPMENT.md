# Development Guide

A practical guide to building, testing, profiling, and releasing the arc codebase.

---

## Prerequisites

| Tool | Version | Install |
|------|---------|---------|
| Rust | ≥ 1.85 (edition 2024) | `curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \| sh` |
| `clippy` | bundled | `rustup component add clippy` |
| `rustfmt` | bundled | `rustup component add rustfmt` |
| `just` | any recent | `cargo install just` |
| `mdbook` | ≥ 0.4 | `cargo install mdbook` |
| `cargo-deny` | any recent | `cargo install cargo-deny` (recommended) |
| `cargo-audit` | any recent | `cargo install cargo-audit` (recommended) |

All core development tasks work with plain `cargo`. `just` and `mdbook` are optional but strongly recommended.

---

## Building

```sh
# Debug build (fast compile, all crates)
cargo build --workspace

# Release build
cargo build --workspace --release
```

The workspace compiles with **zero warnings** under `cargo clippy --all-targets -- -D warnings`. This is a hard requirement for every commit.

---

## Running Tests

```sh
# All tests (count evolves over time; 0 failures required)
cargo test --workspace

# Single crate
cargo test -p arc-core
cargo test -p arc-cli

# Specific test by name
cargo test -p arc-cli -- repo::tests::test_snap
```

All tests are deterministic and isolated. They use `tempfile::TempDir` working directories and make no network calls.

---

## Linting & Formatting

```sh
# Zero-warning clippy (enforced in CI)
cargo clippy --all-targets -- -D warnings

# Format check (enforced in CI)
cargo fmt --all -- --check

# Apply formatting in-place
cargo fmt --all
```

---

## Using the justfile

```sh
just test        # cargo test --workspace
just lint        # cargo clippy --all-targets -- -D warnings
just fmt         # cargo fmt --all
just docs        # mdbook build docs  →  output in docs/book/
just docs-serve  # mdbook serve --open docs  (live-reload at http://localhost:3000)
just clean       # cargo clean
just ci          # test + lint + fmt check (full local CI gate)
just release     # cargo build --workspace --release
just verify-fast # quick lane (format/lint/test/docs)
just verify-full # full lane (fast + security + policy tests)
```

---

## Building the Documentation Book

```sh
just docs          # static HTML → docs/book/
just docs-serve    # live-reload server, opens browser automatically
```

The book source lives in `docs/src/`. The table of contents is `docs/src/SUMMARY.md`. Every filename referenced in `SUMMARY.md` must exist on disk — mdBook fails hard on broken links.

---

## Workspace Layout

```
arc-vcs/
├── Cargo.toml              # workspace manifest
├── justfile                # task runner
├── docs/
│   ├── book.toml
│   └── src/
│       ├── SUMMARY.md
│       ├── introduction.md
│       ├── getting-started/   tutorial, everyday, git-migration, glossary
│       ├── reference/         cli-reference, config, ignore, ai-intents
│       ├── design/            patch_theory, crdt_sync, ast_diffing
│       ├── howto/             custom-hooks
│       └── architecture/ADRs/ 001, 002, 003
├── crates/
│   ├── arc-core/           # algebra + CAS + graph + author identities
│   ├── arc-lang/           # tree-sitter language plugins
│   ├── arc-net/            # network services and sync protocol
│   ├── arc-git-bridge/     # just-in-time Git object translation
│   ├── arc-cli/            # binary + repository orchestration
│   └── arc-daemon/         # JSON-RPC daemon backend for editors
├── research/               # design notes and reading lists
└── target/                 # cargo build output (gitignored)
```

---

## Telemetry During Development

```sh
# Compact structured output to stderr
ARC_TRACE=1 arc snap -m "debug run"

# Append JSON events to a log file
ARC_TRACE_EVENT=/tmp/arc.jsonl arc merge feature/my-view
cat /tmp/arc.jsonl | jq .
```

---

## Profiling

```sh
cargo install flamegraph
# On Linux:
cargo flamegraph --bin arc -- snap -m "profile target"
# On Windows (requires perf or dtrace):
# Use Windows Performance Recorder or `cargo-instruments` on macOS.
```

---

## Security Auditing

```sh
cargo audit
```

Prefer `just verify-security` and `just verify-full` for release-prep checks.
These lanes are intentionally stricter than the default CI path and help catch
issues before push.

---

## Contributor Workflow Norms

1. Prefer small, focused PRs for iterative change.
2. Use ADR-backed design proposals for architectural changes.
3. Keep pure crates side-effect free; place side effects in boundary crates.
4. When modifying docs/contracts, update `docs/architecture/CRATE_STATUS.md`,
   `docs/architecture/STABILITY.md`, or `docs/architecture/SHORTCOMINGS.md`
   as appropriate.

---

## Release Checklist

1. `cargo test --workspace` — 0 failures
2. `cargo clippy --all-targets -- -D warnings` — 0 warnings
3. `cargo fmt --all -- --check` — clean
4. `mdbook build docs` — 0 broken links
5. Update `CHANGELOG.md` with new version and date
6. Bump version in all published crate `Cargo.toml` files
7. `git tag -s v1.0.0 -m "arc 1.0.0 stable"`
8. `cargo publish -p arc-core && cargo publish -p arc-lang && cargo publish -p arc-net && cargo publish -p arc-git-bridge && cargo publish -p arc-cli && cargo publish -p arc-daemon`
