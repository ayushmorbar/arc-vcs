# Known Shortcomings

arc is a mathematically rigorous system. We believe strongly in being honest about where the implementation has not yet caught up to the theory. This document is updated continuously. Contributions that address any of these items are especially welcome.

---

## 1. Rust-Only AST Diffing

**Status:** Active limitation.

`arc-lang` currently ships a single language plugin: `RustPlugin` (via `tree-sitter-rust`). The algebraic patch theory is language-agnostic, but semantic AST diff is not available for Python, TypeScript, Go, C, or any language other than Rust.

**Impact:** Non-Rust files are tracked at the `Atom::Blob` level (whole-file replacement). You lose semantic conflict detection for those files; they are still content-addressed and signed correctly.

**Roadmap:** Additional tree-sitter grammars can be added in `arc-lang` with minimal effort once the plugin interface is stabilised (currently Tier 3). Contributions welcome — see [ADR 002](docs/src/architecture/ADRs/002-ast-over-text.md).

---

## 2. AI Resolution Requires an External API Key

**Status:** By design, but documented here for transparency.

`arc resolve` requires an external LLM API. The production resolver is not bundled; you must provide credentials. Tests use `MockResolver` which is deterministic and credential-free.

**Impact:** Semantic conflict resolution is gated behind an LLM subscription. Without one, conflicts can still be resolved manually: edit the affected files and run `arc snap`.

**Roadmap:** A local GGUF-based resolver plugin is planned for arc 1.1.

---

## 3. Network Sync Is Bidirectional (Resolved in 2026)

**Status:** Resolved milestone.

arc now supports bidirectional sync through two production paths:

- **Git Smart HTTP Bridge** for compatibility with legacy Git remotes.
- **Native TCP Sync Protocol** for direct arc-to-arc synchronization.

Remote peers can both send and receive changesets, including view updates, over the native protocol.

**Roadmap:** Next networking work focuses on higher-level replication topologies (for example, gossip-assisted distribution), not basic push/pull capability.

---

## 4. Windows Shell Built-in Hook Commands

**Status:** Platform limitation, documented throughout.

Shell built-ins (`echo`, `dir`, `type`) are not standalone PATH executables on Windows and cannot be used directly in the hook engine.

**Workaround:** Use `cmd /C echo message` or replace the built-in with a real script or binary.

---

## 5. Large Binary Files Are Not Block-Deduplicated

**Status:** Active limitation.

Binary files are stored as `Atom::Blob` — the entire file is hashed and stored as a single CAS object. There is no block-level deduplication (compare `git-lfs` chunking or `bup` rolling hash).

**Impact:** Frequently-changing large binaries (compiled assets, videos) grow `.arc/blobs/` proportionally. The zero-copy `memmap2` path makes hashing fast, but storage is neither compressed nor deduplicated.

**Roadmap:** Content-defined chunking (CDC) for `Atom::Blob` is planned for arc 1.1.

---

## 6. No Interactive Rebase / Cherry-Pick Equivalent

**Status:** Design gap.

arc has `arc undo` (pop last OpLog entry) and `arc restore` (revert a file), but there is no command equivalent to `git rebase -i` or `git cherry-pick`. Reordering arbitrary subsets of the change graph is theoretically sound (that is exactly what commutativity buys), but the interactive UI is not yet exposed.

**Roadmap:** `arc reorder` is planned for arc 1.1.

---

## 7. No `.gitattributes` Equivalent

**Status:** Design gap.

arc does not yet have a per-file attribute system (end-of-line normalization, binary classification, diff driver overrides). The `Atom::Blob` path is used automatically for files with no recognized language plugin.

---

## 8. `arc-net` Mutation Auth Is Token-Gated (Resolved in 2026)

**Status:** Resolved milestone.

Remote mutation paths now enforce an authentication token guard (`ARC_SYNC_TOKEN`) for non-loopback peers. This gate is validated before accepting remote state mutations.

**Roadmap:** TLS transport hardening and richer request-level cryptographic attestation remain planned for the 1.0 stable line.

---

_If you encounter a limitation not listed here, please open a GitHub Issue or submit a PR adding it to this file._
