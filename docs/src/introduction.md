# arc — Atomic Replayable Changes

**arc eliminates textual merge conflicts — mathematically.** Rather than comparing text line-by-line, arc records every code mutation as a typed algebraic *Atom* and proves via a formal commutativity predicate whether two changes can coexist or structurally conflict. If they commute, the merge is automatic. If they don’t, the conflict is precise, semantic, and AI-resolvable.

## Core Principles

1. **Atom-level granularity** — changes are typed AST mutations (`Insert`, `Delete`, `SemanticsPreserving`, `Blob`, `Mount`) rather than line diffs.
2. **Formal commutativity** — any two changes that do not structurally conflict can be freely reordered, replayed, or cherry-picked without manual intervention.
3. **Cryptographic provenance** — every `Change` is signed with an Ed25519 key and content-addressed via BLAKE3.
4. **Zero-copy I/O** — large binary files are hashed directly from the OS page cache via `memmap2`; they are never copied into memory.

## What Was Built (Phases 1–24)

Phase 1–10 established the algebraic foundation: `Atom`, `Change`, `ChangeGraph`, `commutes()`, the BLAKE3 CAS, tree-sitter Rust AST diffing, `arc init`/`snap`/`log`, and the HTTP transport layer.

Phases 11–20 added cryptographic identity (Ed25519), tags, interactive staging, Git interop (`arc git-import`), semantic conflict detection, AI-assisted resolution (`arc resolve`), and network sync (`arc fetch`/`pull`/`push`).

Phases 21–23 introduced semantic sparse checkouts (`Atom::Mount`), split-root workspaces, hierarchical configuration, command aliases, and causal-stability GC.

Phase 24 shipped the hook engine, Trace2-style telemetry, dual MIT/Apache-2.0 licensing, and this full documentation hierarchy.

## Workspace Crates

| Crate | Role |
|-------|------|
| `arc-core` | Algebra, CAS store, change graph, cryptographic identity |
| `arc-lang` | Language plug-ins: Tree-sitter AST diffing, `RustPlugin` |
| `arc-net` | Network services: HTTP endpoints, sync protocol, AI provider integration |
| `arc-git-bridge` | Git Smart HTTP boundary bridge and Git object translation |
| `arc-cli` | CLI binary and repository orchestration layer |
| `arc-daemon` | JSON-RPC daemon backend for IDE integrations |

## Quick Start

```sh
arc auth login --name "Ada Lovelace" --email "ada@example.com"
cd my-project && arc init
# edit some Rust files…
arc snap -m "feat: add widget"
arc log
```

Continue with the [Tutorial](getting-started/tutorial.md) or jump directly to the [CLI Reference](reference/cli-reference.md).
