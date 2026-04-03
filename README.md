# arc — Atomic Replayable Changes

[![CI](https://img.shields.io/github/actions/workflow/status/ayushmorbar/arc-vcs/ci.yml?branch=main&label=CI)](https://github.com/ayushmorbar/arc-vcs/actions)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](LICENSE-MIT)
[![Crates.io](https://img.shields.io/crates/v/arc-cli.svg)](https://crates.io/crates/arc-cli)
[![Docs](https://img.shields.io/badge/docs-arc--book-orange)](https://ayushmorbar.github.io/arc-vcs/)

**arc eliminates textual merge conflicts — mathematically.** Rather than comparing text line-by-line the way Git has since 2005, arc records every code mutation as a typed algebraic _Atom_ and proves — via a formal commutativity predicate — whether two changes can coexist or structurally conflict. If they commute, the merge is automatic and exact. If they don't, the conflict is precise, semantic, and AI-resolvable. Separately, arc handles 50 GB binary files instantly via zero-copy `memmap2` I/O: the file is never loaded into memory; the Blake3 hash is computed directly from the OS page cache.

## arc vs. the world

| Capability               | Git                                   | jj (Jujutsu)         | **arc**                                                                        |
| ------------------------ | ------------------------------------- | -------------------- | ------------------------------------------------------------------------------ |
| Conflict detection       | Line-based heuristic                  | Line-based heuristic | **Algebraic commutativity**                                                    |
| Diff granularity         | Lines                                 | Lines                | **AST atoms (Rust today; multi-lang roadmap)**                                 |
| Sparse checkouts         | Path globs                            | Path globs           | **Semantic `Atom::Mount` directives**                                          |
| Large binary I/O         | Pack files (copy-on-read)             | Pack files           | **Zero-copy `memmap2`**                                                        |
| Cryptographic provenance | SHA-1 (legacy)                        | SHA-256              | **BLAKE3 + Ed25519 per-change signatures; SLSA L4 zero-trust ingress on push** |
| Hook configuration       | Hidden shell scripts in `.git/hooks/` | Hidden shell scripts | **Declarative JSON in `.arc/config.json`**                                     |
| Observability            | None                                  | None                 | **Trace2-style `ARC_TRACE` telemetry**                                         |
| Conflict resolution      | Manual                                | Manual               | **AI-assisted (`arc resolve`)**                                                |
| Workspace model          | Worktrees                             | Workspaces           | **Split-root workspaces with shared CAS**                                      |

## Key Features

- **Algebraic patch theory** — changes commute or conflict; no ambiguous three-way merges
- **BLAKE3 content-addressable storage** — every atom and change is cryptographically hashed; 256-bit security at 3× SHA-256 speed
- **Semantic AST diffs** — Rust source diffed at syntax-tree level via Tree-sitter; line-level noise is eliminated
- **Zero-copy binary I/O** — `memmap2` maps files directly from the OS page cache; 50 GB assets hashed in milliseconds
- **Split-root workspaces** — share a single `.arc` CAS store across multiple working trees
- **Declarative hook engine** — lifecycle hooks in `.arc/config.json`, not hidden shell scripts
- **Trace2-style telemetry** — zero-overhead by default; `ARC_TRACE=1` or `ARC_TRACE_EVENT=<path>`
- **Hierarchical config + aliases** — global `~/.config/arc/config.json` merged with per-repo config
- **AI-assisted conflict resolution** — semantic conflicts routed to a pluggable `AiResolver` interface
- **Causal stability GC** — garbage collection only prunes causally-stable changes

## Quick Start

```sh
# Set up your identity (once)
arc auth login --name "Ada Lovelace" --email "ada@example.com"

# Initialise a repository
arc init

# Record a change
arc snap -m "feat: add widget"

# Show history
arc log

# Rewrite history: squash a linear sequence into one canonical change
arc squash --into HEAD~3

# Two-step external-editor history rewrite
arc diffedit --prepare HEAD~2
# (edit the materialised file, then:)
arc diffedit --apply

# Push to a registered remote
arc push origin main

# Views are not branches — they are named sets of DAG heads
arc view create feature/my-work
arc switch feature/my-work
arc merge feature/my-work
```

> **New to arc?** Start with the [Tutorial](docs/src/getting-started/tutorial.md). Coming from Git? Read [Why arc is Not Branch-Based](docs/src/getting-started/git-migration.md).

## Installation

```sh
cargo install --path crates/arc-cli
# Requires Rust 1.85+ (edition 2024) — https://rustup.rs
```

## Telemetry

arc ships with zero-overhead tracing disabled by default — no subscriber is installed and `tracing` macros compile away entirely.

| Environment variable     | Effect                                            |
| ------------------------ | ------------------------------------------------- |
| _(unset)_                | No subscriber; `tracing` macros compile to no-ops |
| `ARC_TRACE=1`            | Compact human-readable output to stderr           |
| `ARC_TRACE_EVENT=<path>` | Structured JSON events **appended** to `<path>`   |

## Hook Engine

Lifecycle hooks are declared in `.arc/config.json`:

```json
{
  "hooks": {
    "pre-snap": ["./scripts/lint.sh --strict"],
    "post-merge": ["./scripts/notify.sh"]
  }
}
```

Hooks are parsed by `shlex` and run with `work_root` as the working directory. A non-zero exit aborts the operation immediately.

> **Windows:** shell built-ins like `echo` are not PATH executables. Use `cmd /C echo ...` or a real binary.

## Documentation

| Topic                                                           | Link                                                                         |
| --------------------------------------------------------------- | ---------------------------------------------------------------------------- |
| Tutorial (zero to first snap)                                   | [docs/src/getting-started/tutorial.md](docs/src/getting-started/tutorial.md) |
| CLI Reference                                                   | [docs/src/reference/cli-reference.md](docs/src/reference/cli-reference.md)   |
| Patch Theory deep-dive                                          | [docs/src/design/patch_theory.md](docs/src/design/patch_theory.md)           |
| History Rewriting (squash, diffedit, inversion algebra)         | [docs/src/design/history_rewriting.md](docs/src/design/history_rewriting.md) |
| Network Transport (DeltaPayload, zero-trust ingress, CRDT sync) | [docs/src/design/network_transport.md](docs/src/design/network_transport.md) |
| Custom hooks how-to                                             | [docs/src/howto/custom-hooks.md](docs/src/howto/custom-hooks.md)             |
| Architecture Decision Records                                   | [docs/src/architecture/ADRs/](docs/src/architecture/ADRs/)                   |

Build the full book locally: `just docs` (requires [mdBook](https://rust-lang.github.io/mdBook/)).

## Stability & Known Limitations

See [STABILITY.md](STABILITY.md) for the production-ready API surface and [SHORTCOMINGS.md](SHORTCOMINGS.md) for honest engineering limits.

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) for the 4-crate workspace architecture, commit conventions, and AI-authorship signature protocol.

## License

Licensed under either of

- [MIT License](LICENSE-MIT)
- [Apache License, Version 2.0](LICENSE-APACHE)

at your option.
