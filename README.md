# arc — Atomic Replayable Changes

[![CI](https://img.shields.io/github/actions/workflow/status/ayushmorbar/arc-vcs/ci.yml?branch=main&label=CI)](https://github.com/ayushmorbar/arc-vcs/actions)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](LICENSE-MIT)
[![Crates.io](https://img.shields.io/crates/v/arc-cli.svg)](https://crates.io/crates/arc-cli)
[![Docs](https://img.shields.io/badge/docs-arc--book-orange)](https://ayushmorbar.github.io/arc-vcs/)

arc is a semantic, replayable VCS designed for AI-assisted development and
high-integrity change history. Instead of line-based patch heuristics, arc
stores typed CRDT changes in a content-addressed graph and verifies provenance
on sync boundaries.

## Why Arc Instead of Git

Git optimises for textual patch transport. arc optimises for correctness under
concurrent semantic change.

| Dimension | Git | arc |
|---|---|---|
| Merge primitive | line heuristics | algebraic commutativity over typed atoms |
| Conflict unit | text region | semantic operation with first-class conflict atoms |
| Provenance | commit hash chain | per-change Ed25519 signatures + BLAKE3 CAS |
| Identity on sync ingress | trust transport boundary | zero-trust signature verification before CAS writes |
| Architecture for refactors | broad core coupling | micro-crate vertical slices with narrow contracts |

## The 5-Stage Pipeline

Every heavy operation in arc follows a shared stage taxonomy:

```mermaid
flowchart LR
    A[Discover] --> B[Negotiate]
    B --> C[Transfer]
    C --> D[Materialize]
    D --> E[Finalize]
```

`arc sync` exposes this pipeline with staged terminal progress and
tracing-linked telemetry. Each stage is independently observable and
retryable.

## Quick Start

```sh
# 1. Set up identity once
arc auth login --name "Ada Lovelace" --email "ada@example.com"

# 2. Initialize repository
arc init

# 3. Record a semantic change
arc snap -m "feat: add parser pipeline"

# 4. Inspect history
arc log

# 5. Rewrite history algebraically
arc squash --into HEAD~3

# 6. Synchronise with remote
arc sync 127.0.0.1:8080

# 7. Push through Git Smart HTTP translation bridge
arc push https://github.com/<org>/<repo>.git
```

## Installation

### Prebuilt binaries (recommended)

Download the latest release for your platform from the
[Releases](https://github.com/ayushmorbar/arc-vcs/releases) page. Archives
are provided for:

| Platform | Target | Archive |
|---|---|---|
| macOS Apple Silicon | `aarch64-apple-darwin` | `arc-*.tar.gz` |
| macOS Intel | `x86_64-apple-darwin` | `arc-*.tar.gz` |
| Linux glibc (x86_64) | `x86_64-unknown-linux-gnu` | `arc-*.tar.gz` |
| Linux glibc (aarch64) | `aarch64-unknown-linux-gnu` | `arc-*.tar.gz` |
| Linux musl (x86_64) | `x86_64-unknown-linux-musl` | `arc-*.tar.gz` |
| Windows | `x86_64-pc-windows-msvc` | `arc-*.zip` |

#### macOS / Linux

```sh
curl -sSfL https://github.com/ayushmorbar/arc-vcs/releases/latest/download/installer.sh | sh
```

#### Windows (PowerShell)

```powershell
irm https://github.com/ayushmorbar/arc-vcs/releases/latest/download/installer.ps1 | iex
```

### From source

```sh
cargo install --path crates/arc-cli
```

Requires Rust 1.85+ (edition 2024).

## Crate Architecture

arc is structured as focused micro-crates with explicit purity and I/O
boundaries.

| Concern | Primary crates |
|---|---|
| Domain types and IDs | `arc-algebra-types`, `arc-store-types`, `arc-change` |
| Algebra and patch semantics | `arc-algebra`, `arc-engine`, `arc-revset` |
| Storage and graph state | `arc-store-cas`, `arc-store-graph`, `arc-store-view` |
| Transport and protocol | `arc-network`, `arc-net` |
| Identity and AI orchestration | `arc-ai`, type-level author contracts in `arc-store-types` |
| User and integration surfaces | `arc-cli`, `arc-daemon`, `arc-git-bridge` |

> `arc-core` is a compatibility facade during migration, not the long-term
> engine of record. New code belongs in the vertical-slice crates above.

## Axiom of Purity and Wasm Portability

arc enforces a strict engineering rule across all crates:

- Pure algebra/domain crates must not perform filesystem, network, process,
  clock, or terminal I/O.
- Side effects are isolated to explicit boundary crates (CLI, daemon, network
  adapters, storage adapters).
- Core computation must remain wasm-safe unless explicitly constrained behind
  native-only feature flags.

This is what keeps arc replayable, testable across native + wasm targets, and
portable to deterministic execution environments.

## Crash-Consistency Guarantees

arc persistence paths are designed for crash safety:

- **Atomic rename** write patterns for view and metadata updates.
- **Append-only** operation logging for durable intent sequencing.
- **Explicit sync barriers** where durability is required before returning to
  the caller.

Crash consistency is a protocol guarantee, not a best-effort implementation
detail.

## Telemetry

| Environment variable | Effect |
|---|---|
| *(unset)* | no subscriber installed |
| `ARC_TRACE=1` | compact human-readable trace output |
| `ARC_TRACE_EVENT=<path>` | append JSON event stream to file |

## Documentation

| Topic | Link |
|---|---|
| Tutorial | [docs/src/getting-started/tutorial.md](docs/src/getting-started/tutorial.md) |
| CLI reference | [docs/src/reference/cli-reference.md](docs/src/reference/cli-reference.md) |
| Architecture | [docs/src/architecture/overview.md](docs/src/architecture/overview.md) |
| Patch theory | [docs/src/design/patch_theory.md](docs/src/design/patch_theory.md) |
| Network transport | [docs/src/design/network_transport.md](docs/src/design/network_transport.md) |

## Contributing

Read [CONTRIBUTING.md](CONTRIBUTING.md) for workflow, crate boundaries, and
guardrails.

## License

Licensed under either [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at
your option.