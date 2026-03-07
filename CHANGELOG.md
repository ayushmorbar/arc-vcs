# Changelog

All notable changes to arc are documented here.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).
arc uses [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

---

## [0.1.0-beta.1] — 2026-03-08

First versioned beta release covering Phases 26–33.
**Zero C dependencies** — `git2`/libgit2 excised, `reqwest` built with
`rustls-tls`. `arc` builds to a single static binary on Linux, macOS, and
Windows with no cmake, no OpenSSL headers, and no C compiler required.
WASM compilation is now structurally unblocked.

### Added — Phase 33 (Async Network Engine)
- **`arc-core::network`** — new module implementing [`NetworkClient`]: a
  pure-Rust async HTTP client built on `reqwest` + `rustls-tls` (zero OpenSSL).
  `push` and `pull` are async, skeleton-complete, connectivity-verifying.
  Full incremental CRDT delta upload/download lands in Phase 34.
- **`arc push [remote]`** upgraded from hint to live skeleton: resolves the
  named remote from `.arc/config.json`, boots a `tokio::runtime` at the CLI
  edge (keeping all other commands zero-latency synchronous), and calls
  `NetworkClient::push`. Actionable error when remote is unconfigured.
- **`reqwest = { version = "0.12", default-features = false, features = ["json", "rustls-tls"] }`**
  added to `arc-core` — the only new dependency; no C libs required.
- All four crates bumped to **`0.1.0-beta.1`**.

### Added — Phase 32 (The Great Amputation — plan ready, applying next)
- Pure-Rust `interop/git.rs` rewrite using `git_bridge` pipeline
  (scheduled for Phase 32 apply commit).

### Added — Phase 31 (Tree & Blob Extraction Layer)
- `TreeEntry`, `GitTree`, `parse_tree()` — binary Git tree format decoder with
  safe NUL/SP state machine; gracefully handles truncated trailing entries.
- `read_blob()` — bridge between the Git DAG and the Tree-sitter AST engine.
- `read_tree_for_commit()`, `extract_tree_to_memory()` — full recursive tree
  walk returning `HashMap<path, bytes>` for every blob in a commit.
- `head_branch_name()` — reads `.git/HEAD`, portable across `master`/`main`.
- `resolve_git_dir` made `pub` for downstream crate access.

### Added — Phase 30 (Pure-Rust Git Bridge)
- `crates/arc-core/src/git_bridge.rs` (~500 lines) — bespoke Git kernel:
  ref resolution, loose object decompression (zlib/flate2), pack index v2,
  `OFS_DELTA` + `REF_DELTA` reconstruction, commit parsing, BFS DAG walk.
  Only new dependency: `flate2 = "1"`.
- `analyze_git_repo(path) → GitAnalysis` public API; returns commits
  oldest-first with full author, timestamp, and parent metadata.

### Added — Phase 29 (Remote Suite Completion)
- `arc remote remove <name>` with actionable error pointing to `arc remote list`.

### Added — Phase 28 (Empathy & Context Layer)
- `On view: <name>` cyan-bold header in `arc status` and `arc diff`.
- `arc identity` now accepts `--name` / `--email` long flags.
- `arc snap` bails with a configuration hint when no identity exists.
- Empty `arc log` message: `"Use 'arc snap' to create your first change."`.

### Added — Phase 27 (DX Polish)
- `arc identity --name <n> --email <e>` first-run wizard.
- `arc diff` with coloured ANSI output and `atom_diff_line()` helper.
- Proactive `arc push` hint appended after every successful `arc snap`.

### Added — Phase 26 (Power-User Parity)
- `arc amend` — rewrites the latest change (re-signs, re-hashes).
- `resolve_rev` — resolves short hashes, `HEAD`, `HEAD~N`, named tags.
- fs2 re-entrancy guard on `.arc/lock` to prevent deadlocks in nested calls.
- Fixed `&hash[..8]` panic for CherryPick / Tag / Revert operations.

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
