# Changelog

All notable changes to arc are documented here.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).
arc uses [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

---

## [1.0.0] — 2026-03-07

The Genesis Release. All 25 development phases complete. arc is production-ready.

### Added — Phase 25 (The Genesis Release)
- **DAG Compaction (`arc compact`)** — PO-Log Compaction via a single Genesis Change: collapses the entire causally-stable DAG history into one `Atom::Insert`/`Atom::Blob` snapshot, permanently solving CRDT tombstone growth. A 10-year-old repository with millions of changes can be truncated in O(|stable_set|) time.
- **Epoch Map (`.arc/epochs`)** — append-only JSON map of `compacted_id → genesis_id`. The `hydrate_heads()` BFS transparently intercepts compacted IDs and redirects to the Genesis node, preserving the BLAKE3 cryptographic identity of every live `Change` object. Fully compatible with SLSA L4 provenance and P2P CRDT sync.
- **`justfile` `compact` recipe** — `arc compact` available as `just compact`.
- **GitHub Actions CI pipeline** (`.github/workflows/ci.yml`) — triggers on push to `main` and pull requests: `cargo check`, `cargo fmt --check`, `cargo clippy -D warnings`, `cargo test`.
- **GitHub Actions docs pipeline** (`.github/workflows/docs.yml`) — builds mdBook on push to `main` and deploys to GitHub Pages via `actions/upload-pages-artifact` + `actions/deploy-pages`. Permissions: `pages: write`, `id-token: write`.
- **GitHub Actions release pipeline** (`.github/workflows/release.yml`) — triggers on `v*` tags; builds release binaries for `ubuntu-latest`, `macos-latest`, `windows-latest` and uploads them to the GitHub Release via `softprops/action-gh-release`.

### Changed — Phase 25
- `hydrate_heads()` now consults `.arc/epochs` before attempting a CAS read, enabling transparent history truncation without mutating any existing `Change`.
- `Repository::compact()` performs its own targeted CAS deletion (distinct from `gc()`) so tombstones are removed immediately without a separate GC pass.

---

## [1.0.0-rc.1] — 2026-03-07

This release candidate marks the completion of all 24 development phases and represents the full feature set targeted for the arc 1.0 stable release.

### Added — Phase 24 Part 2 (2026 OSS Blueprint)
- **Full mdBook docs hierarchy** — `docs/src/` expanded to Diátaxis structure: Getting Started, Reference, Design, How-To, Architecture/ADRs.
- **Architecture Decision Records** — ADR 001 (BLAKE3 CAS), ADR 002 (AST over text-diff), ADR 003 (CRDT over OT).
- **GitHub templates** — `ISSUE_TEMPLATE/bug_report.md`, `PULL_REQUEST_TEMPLATE.md`, `.github/FUNDING.yml`.
- **Governance & conduct** — `GOVERNANCE.md`, `CODE_OF_CONDUCT.md` (Contributor Covenant v2.1).
- **Engineering honesty** — `STABILITY.md` (4-tier API classification), `SHORTCOMINGS.md` (8 documented limits).
- **`justfile`** — task runner: `test`, `lint`, `fmt`, `docs`, `docs-serve`, `clean`, `ci`, `release`.
- **`DEVELOPMENT.md`** — comprehensive build, test, profile, and release guide.
- **`.mailmap`** — template for canonical arc identity mapping.

### Added — Phase 24 Part 1 (OSS Launchpad)
- **Hook engine** — declarative lifecycle hooks (`pre-snap`, `post-merge`) via `hooks` in `.arc/config.json`. `shlex`-parsed, `work_root` CWD, descriptive Windows PATH error.
- **Trace2-style telemetry** — `init_tracing()`: NopSubscriber default (zero overhead), `ARC_TRACE=1` compact stderr, `ARC_TRACE_EVENT=<path>` JSON append file.
- **`tracing::info!` / `debug!`** instrumentation in `snap()`, `merge_heads()`, `write_state_to_working_dir()`.
- **Dual license** — `MIT OR Apache-2.0` in all four crate `[package]` sections; `LICENSE-MIT`, `LICENSE-APACHE` in root.

### Changed — Phase 24
- `RepoConfig` adds `hooks: HashMap<String, Vec<String>>` with `#[serde(default)]`.
- **Clippy fixes** — `unnecessary-get-then-check` in `apply.rs` and `repo.rs` tests; `unnecessary-map-or` in `init_tracing`.

---

## [0.1.0-alpha] — Phases 1–23

### Added — Phase 23 (Workspace & Config)
- Split-root workspaces (`arc workspace add`, `arc workspace list`); `WorkspaceManifest` at `.arc-workspace`.
- Hierarchical config merge: global `~/.config/arc/config.json` + per-repo `.arc/config.json`.
- User-defined command aliases (`arc config alias`).
- Causal stability-aware garbage collection (`arc gc`); `GcResult` with retained/pruned counts.

### Added — Phase 22 (Semantic Sparse Checkouts)
- `Atom::Mount` — sparse checkout directives stored as change-graph atoms.
- `arc sparse set/add/remove/list`; patterns respected by `write_state_to_working_dir`.

### Added — Phase 21 (Network Sync)
- `arc fetch`, `arc pull`, `arc push`.
- Incremental CAS sync via `arc-net` HTTP server (`axum`).

### Added — Phase 20 (AI Conflict Resolution)
- `arc resolve` with pluggable `AiResolver` trait; `MockResolver` for tests.
- `PendingConflict` serialized to `.arc/conflict` for resumable resolution.

### Added — Phase 19 (Conflict Detection)
- Cross-product commutativity check in `merge_heads()`, LCA computation, hex IDs in error messages.

### Added — Phase 18 (Views & Merging)
- `View`: named set of DAG heads; `arc view create/list/delete`, `arc switch`, `arc merge`.
- `merge_heads()` with dirty working-directory guard.

### Added — Phase 17 (Interactive Staging)
- `arc snap --interactive` / `-i`: per-atom accept/reject; directory atoms always staged.

### Added — Phase 16 (Git Interop)
- `arc git-import` — import Git history into arc CAS via `git2`.

### Added — Phase 15 (Arcignore)
- `.arcignore` support via the `ignore` crate; excluded from `snap`, `status`, `diff`.

### Added — Phase 14 (Status & Diff)
- `arc status` — working-directory delta. `arc diff` — per-atom formatted output.

### Added — Phase 13 (Restore & Undo)
- `arc restore <path>`, `arc undo`; `OpLog` at `.arc/oplog`.

### Added — Phase 12 (Cryptographic Identity)
- `arc auth login` — ed25519-dalek keypair generation; every `Change` is signed.

### Added — Phase 11 (Tags)
- `arc tag create/list/delete`; `Tag` struct signed with author keypair.

### Added — Phases 1–10 (Foundation)
- `Atom`, `Change`, `ChangeGraph`, `commutes()`, `apply_change()`.
- BLAKE3 content-addressable `ObjectStore`.
- Tree-sitter Rust plugin: `diff()`, `unparse()`.
- `arc init`, `snap`, `log`, `status`, `diff`, `restore`, `undo`.
- `arc remote add/list`, `arc config set/get`.
- `arc-net` HTTP server; `arc-lang` Rust plugin.
- Zero-copy `memmap2` binary I/O.

---

## [Unreleased — Post 1.0.0-rc.1]

*No changes yet.*
