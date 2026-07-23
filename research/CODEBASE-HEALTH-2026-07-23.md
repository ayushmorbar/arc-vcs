# Codebase Health Report — 2026-07-23

Full audit of arc-vcs codebase. Generated from tarpaulin coverage runs, clippy analysis, and manual review.

---

## Coverage Summary

| Metric | Value |
|--------|-------|
| Full codebase coverage | **62.35%** (9148/14673 lines) |
| arc-cli coverage | 42.35% (5625/13281 lines) |
| Coverage gate (CI) | 50% (recommended target: 70%) |
| Total tests | 1,434+ across 32 crates |

---

## TODO/FIXME Inventory

Only 2 tracked tech-debt items in the entire codebase. Both are purity violations in crates that should be pure math/graph.

### 1. arc-algebra/src/apply.rs:6 — Purity Fix (ignore crate)

```
// TODO(v0.2): Purity Fix — `ignore` crate is a filesystem walker (pulls walkdir,
// globset, crossbeam-channel).  This crate must stay pure math/algebra.
// Replace with a local glob AST matcher or accept glob patterns as pre-parsed
// predicate closures injected by the caller.
use ignore::gitignore::Gitignore;
```

**Problem**: `arc-algebra` is supposed to be a pure math/algebra crate (deterministic, wasm-friendly). The `ignore` crate pulls in `walkdir`, `globset`, `crossbeam-channel` — all filesystem I/O.

**Fix options**:
- Accept pre-parsed predicate closures from the caller instead of walking the filesystem
- Write a local glob AST matcher that doesn't touch the filesystem
- Move the filesystem-dependent logic to a boundary crate (e.g., `arc-algebra-fs`)

### 2. arc-store-graph/src/bisect.rs:1 — Purity Fix (std::fs/io)

```
// TODO(v0.2): Purity Fix — `std::fs` and `std::io` are heavy filesystem I/O in a
// "graph" crate.  Extract bisect state persistence into a dedicated `arc-bisect-persist`
// boundary crate and keep this crate's graph algorithms pure.
use std::fs::{self, File};
use std::io::Write;
```

**Problem**: `arc-store-graph` should contain pure graph algorithms. Instead, `bisect.rs` uses `std::fs`/`std::io` for state persistence (writing bisection state to disk).

**Fix options**:
- Extract persistence into `arc-bisect-persist` boundary crate
- Use a `BisectStore` trait injected by the caller
- Accept a `Write` trait object instead of hardcoding `File`

---

## Dead Code: Root `src/` Directory

The entire root `src/` directory is **legacy monolith code** from before the workspace restructuring.

**Evidence**:
- Root `Cargo.toml` has no `[package]` section — these files are never compiled
- No workspace crate depends on them via Cargo.toml path dependencies
- Frozen at the workspace split commit (`3c813a7`); zero commits touched `src/` after
- `Architecture.md` line 55 explicitly disclaims it

**Files**: `src/lib.rs`, `src/author.rs`, `src/cas.rs`, `src/change.rs`, `src/commit.rs`, `src/repo.rs`, `src/view.rs`, `src/interop/git.rs`, plus subdirectories `src/store/`, `src/interop/`, `src/algebra/`, `src/network/`, `src/ai/`

**Recommendation**: Delete. It inflates "0% coverage" metrics and confuses contributors.

---

## Unsafe Code Audit

35 `unsafe` occurrences across the codebase. **All properly documented with `// SAFETY:` comments**.

| Location | Usage | Risk |
|----------|-------|------|
| `arc-store-cas/src/cas.rs` | `memmap2::Mmap` (immutable snapshots) | Low — read-only mapping |
| `arc-cli/src/` (multiple) | `memmap2::Mmap` (same pattern) | Low — read-only mapping |
| `arc-git/src/ingress.rs:338` | `memmap2::Mmap` (git packfiles) | Low — read-only mapping |
| `arc-store-policy/src/lib.rs:302` | `libc::geteuid()` | Low — single syscall |
| `arc-testtools/src/env.rs` | `env::set_var/remove_var` (test-only) | Low — guarded by `env_lock()` |
| `arc-cli/src/sync.rs` | Hex decoding | Low — bounds-checked |

**Crates with `#![forbid(unsafe_code)]`**: arc-ux, arc-diff, arc-tui, arc-keyring

---

## Test Distribution by Crate

| Crate | Tests | Source Files | Lines | Density |
|-------|:-----:|:-----------:|:-----:|:-------:|
| arc-cli | 500 | 58 | 13,281 | 0.038 |
| arc-algebra | 124 | 11 | 3,332 | 0.037 |
| arc-revset | 119 | 3 | 3,048 | 0.039 |
| arc-change | 85 | 1 | 1,308 | 0.065 |
| arc-store-types | 82 | 1 | 1,739 | 0.047 |
| arc-store-view | 64 | 3 | 1,672 | 0.038 |
| arc-store-cas | 64 | 1 | 1,933 | 0.033 |
| arc-core | 57 | 4 | 1,904 | 0.030 |
| arc-diff | 51 | 1 | 80 | 0.638 |
| arc-store-graph | 44 | 2 | 968 | 0.045 |

**Under-tested crates**:
- **arc-lsp**: 0 tests (194 lines, 2 files) — 🚨 zero coverage
- **arc-tui**: 6 tests (1,582 lines, 16 files) — very low
- **arc-git**: 9 tests (1,627 lines, 3 files) — low for core functionality
- **arc-diagnostics**: 8 tests (466 lines, 2 files)
- **arc-keyring**: 8 tests (469 lines, 1 file) — no inline tests

---

## Tarpaulin Instrumentation Gaps

Several crates show 0% in tarpaulin despite having passing tests. This is a systemic tarpaulin issue, not missing tests.

| Crate | Tests | Status |
|-------|:-----:|--------|
| arc-diff | 47 tests pass | 0% tarpaulin |
| arc-core/store/oplog | 4 tests pass | 0% tarpaulin |
| arc-store-graph/traversal_state | 3 tests pass | 0% tarpaulin |
| arc-store-view/checkpoint | 1 test passes | 0% tarpaulin |

**Cause**: Tarpaulin doesn't instrument inline `#[cfg(test)]` modules in certain configurations.

---

## Compilation Health

- `cargo check --workspace`: **Clean** — zero warnings
- `cargo clippy --workspace --all-targets`: **Clean** — zero warnings
- All 1,434+ tests passing

---

## Recommended Actions (Priority Order)

1. **Delete root `src/` directory** — dead monolith code, inflates metrics
2. **Add tests for arc-lsp** — 194 lines, zero tests, it's an LSP server
3. **Add tests for arc-tui** — 16 files, only 6 tests
4. **Improve arc-git test coverage** — core git bridge, 9 tests for 1,627 lines
5. **Address v0.2 purity fixes** — `arc-algebra/src/apply.rs` and `arc-store-graph/src/bisect.rs`
6. **Investigate tarpaulin instrumentation gaps** — 4 crates with passing tests showing 0%
