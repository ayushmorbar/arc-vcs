# arc - Atomic Replayable Changes

[![CI](https://img.shields.io/github/actions/workflow/status/ayushmorbar/arc-vcs/ci.yml?branch=main&label=CI)](https://github.com/ayushmorbar/arc-vcs/actions)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](LICENSE-MIT)
[![Crates.io](https://img.shields.io/crates/v/arc-cli.svg)](https://crates.io/crates/arc-cli)
[![Docs](https://img.shields.io/badge/docs-arc--book-orange)](https://ayushmorbar.github.io/arc-vcs/)

## Bottom Line Up Front

arc is a mathematically constrained VCS for large-scale, AI-assisted engineering.
It models history as typed semantic changes over a DAG, not text hunks over files.
After ADR-004, arc is structured as focused micro-crates with explicit purity and I/O boundaries: algebra and identity logic remain side-effect free, while disk and network effects are isolated to dedicated crates.

## Why This Beats Git In Complex Systems

Git optimizes for textual patch transport. arc optimizes for correctness under concurrent semantic change.

| Dimension                  | Git                      | arc                                                 |
| -------------------------- | ------------------------ | --------------------------------------------------- |
| Merge primitive            | line heuristics          | algebraic commutativity over typed atoms            |
| Conflict unit              | text region              | semantic operation with first-class conflict atoms  |
| Provenance                 | commit hash chain        | per-change Ed25519 signatures + BLAKE3 CAS          |
| Identity on sync ingress   | trust transport boundary | zero-trust signature verification before CAS writes |
| Architecture for refactors | broad core coupling      | micro-crate vertical slices with narrow contracts   |

## ADR-004 Slice Architecture

The old monolithic core has been decomposed into focused crates.

| Concern                       | Primary crates                                             |
| ----------------------------- | ---------------------------------------------------------- |
| Domain types and IDs          | `arc-algebra-types`, `arc-store-types`, `arc-change`       |
| Algebra and patch semantics   | `arc-algebra`, `arc-engine`, `arc-revset`                  |
| Storage and graph state       | `arc-store-cas`, `arc-store-graph`, `arc-store-view`       |
| Transport and protocol        | `arc-network`, `arc-net`                                   |
| Identity and AI orchestration | `arc-ai`, type-level author contracts in `arc-store-types` |
| User and integration surfaces | `arc-cli`, `arc-daemon`, `arc-git-bridge`                  |

`arc-core` remains as a compatibility facade during migration, not as the long-term engine of record.

## Axiom Of Purity And I/O Isolation

arc enforces a strict engineering rule:

- Pure domain crates must not perform filesystem or network side effects.
- Disk persistence is isolated to storage crates.
- Network ingress/egress is isolated to transport crates.

This separation is what makes large rewires testable, reviewable, and portable.

## Crash-Consistency Guarantees

arc persistence paths are designed for crash safety:

- Atomic rename write patterns for view and metadata updates.
- Append-only operation logging for durable intent sequencing.
- Explicit sync barriers where durability is required.

> **Note:** Crash consistency is treated as a protocol guarantee, not a best-effort implementation detail.

## Quick Start

```sh
# Set up identity once
arc auth login --name "Ada Lovelace" --email "ada@example.com"

# Initialize repository
arc init

# Record semantic change
arc snap -m "feat: add widget"

# Inspect history
arc log

# Rewrite history algebraically
arc squash --into HEAD~3

# Push through Git Smart HTTP translation bridge
arc push https://github.com/<org>/<repo>.git
```

## Install

```sh
cargo install --path crates/arc-cli
```

Requires Rust 1.85+ (edition 2024).

## Telemetry

| Environment variable     | Effect                              |
| ------------------------ | ----------------------------------- |
| unset                    | no subscriber installed             |
| `ARC_TRACE=1`            | compact human-readable trace output |
| `ARC_TRACE_EVENT=<path>` | append JSON event stream to file    |

## Documentation

| Topic             | Link                                                                         |
| ----------------- | ---------------------------------------------------------------------------- |
| Tutorial          | [docs/src/getting-started/tutorial.md](docs/src/getting-started/tutorial.md) |
| CLI reference     | [docs/src/reference/cli-reference.md](docs/src/reference/cli-reference.md)   |
| Architecture      | [docs/src/architecture/overview.md](docs/src/architecture/overview.md)       |
| Patch theory      | [docs/src/design/patch_theory.md](docs/src/design/patch_theory.md)           |
| Network transport | [docs/src/design/network_transport.md](docs/src/design/network_transport.md) |
| ADR index         | [docs/src/architecture/ADRs/](docs/src/architecture/ADRs/)                   |

Build docs locally with `just docs`.

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) for crate boundaries, workflow, and review requirements.

## License

Licensed under either [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at your option.
