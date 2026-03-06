# arc — Atomic Replayable Changes

**arc** is a next-generation version-control system designed around three core ideas:

1. **Atom-level granularity** — changes are recorded as typed AST mutations (Insert, Delete, Move, SemanticsPreserving) rather than text line diffs.
2. **Formal commutativity** — any two changes that do not structurally conflict can be freely reordered, replayed, or cherry-picked without manual merge.
3. **Cryptographic provenance** — every change is signed with an Ed25519 key, and the entire graph is content-addressed via Blake3.

## Workspace crates

| Crate | Role |
|-------|------|
| [`arc-core`](../crates/arc-core/README.md) | Algebra, CAS store, change graph |
| [`arc-lang`](../crates/arc-lang/README.md) | Language plug-ins (tree-sitter) |
| [`arc-net`](../crates/arc-net/README.md) | HTTP distribution server |
| [`arc-cli`](../crates/arc-cli/README.md) | CLI binary and repository orchestration |

## Quick start

```sh
arc auth login --name "Ada Lovelace" --email "ada@example.com"
cd my-project && arc init
# edit some Rust files …
arc snap -m "feat: add widget"
arc log
```
