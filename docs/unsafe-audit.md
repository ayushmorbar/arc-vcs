# Unsafe Code Audit Registry

All `unsafe` blocks in arc-vcs are documented here. Every block must have a
`// SAFETY:` comment in source and a corresponding entry in this registry.

## Policy

- New unsafe blocks require a PR review comment explaining why safe abstractions
  are insufficient.
- Every unsafe block must have a `// SAFETY:` comment on the preceding line.
- Clippy lint `clippy::undocumented_unsafe_blocks` is enforced in CI.
- Miri runs on targeted crates to catch UB in unsafe code paths.

## Inventory

| # | Crate | File:Line | Kind | Rationale |
|---|-------|-----------|------|-----------|
| 1 | `arc-store-cas` | `crates/arc-store-cas/src/cas.rs:668` | `MmapOptions::map` | CAS files are immutable after publish; mmap is safe on immutable backing store |
| 2 | `arc-store-policy` | `crates/arc-store-policy/src/lib.rs:299` | `libc::geteuid()` | POSIX FFI; always safe on supported Unix targets |
| 3 | `arc-git` | `crates/arc-git/src/ingress.rs:333` | `MmapOptions::map_copy_read_only` | Git pack/index files are immutable snapshots created atomically |
| 4 | `arc-store` | `src/store/cas.rs:63` | `Mmap::map` | CAS objects are immutable once stored; file named by BLAKE3 hash |
| 5 | `arc-cli` | `crates/arc-cli/src/repo/core.rs:2665` | `memmap2::Mmap::map` | CAS blob store is append-only; files named by BLAKE3 hash, immutable |
| 6 | `arc-cli` | `crates/arc-cli/src/repo/core.rs:5361` | `memmap2::Mmap::map` | Same as #5 — blob restore path |
| 7 | `arc-cli` (test) | `crates/arc-cli/tests/devtools_orchestration.rs:41` | `std::env::set_var` | Test-only; single-threaded with env_lock() guard |
| 8 | `arc-cli` (test) | `crates/arc-cli/tests/devtools_orchestration.rs:51` | `std::env::remove_var` | Test-only; single-threaded with env_lock() guard |
| 9 | `arc-testtools` | `crates/arc-testtools/src/env.rs:42` | `std::env::set_var` | EnvGuard holds process-wide mutex; serialized across threads |
| 10 | `arc-testtools` | `crates/arc-testtools/src/env.rs:52` | `std::env::set_var` | Drop restores previous value under same mutex contract |
| 11 | `arc-testtools` | `crates/arc-testtools/src/env.rs:55` | `std::env::remove_var` | Drop removes var under same mutex contract |
| 12 | `arc-testtools` (test) | `crates/arc-testtools/src/env.rs:75` | `std::env::remove_var` | Test-only; env_lock() guard protects mutation |
| 13 | `arc-testtools` (test) | `crates/arc-testtools/src/env.rs:90` | `std::env::set_var` | Test-only; env_lock() guard protects mutation |
| 14 | `arc-testtools` (test) | `crates/arc-testtools/src/env.rs:100` | `std::env::remove_var` | Test-only; env_lock() guard protects mutation |
| 15 | `arc-testtools` (test) | `crates/arc-testtools/src/env.rs:108` | `std::env::set_var` | Test-only; env_lock() guard protects mutation |
| 16 | `arc-testtools` (test) | `crates/arc-testtools/src/env.rs:122` | `std::env::remove_var` | Test-only; env_lock() guard protects mutation |

## Categories

- **CAS mmap (1–4, 6)**: Content-addressed storage files are immutable by
  design. The BLAKE3 hash in the filename guarantees no writer will modify a
  mapped file. All blocks follow the same pattern: open file → mmap → read.
- **POSIX FFI (2)**: `libc::geteuid()` is a trivial, infallible syscall.
- **Env mutation (7–16)**: All `set_var`/`remove_var` calls are test helpers
  protected by a process-wide `Mutex<()>` (`env_lock()`). The mutex is held
  for the entire duration of env mutation, preventing data races.

## Miri Scope

### What Miri checks

Miri detects undefined behavior at the Rust abstract machine level:

- **Unsafe code paths**: invalid pointer arithmetic, use-after-free, double-free,
  data races, misaligned references
- **Integer overflow**: in debug builds (the default for `cargo miri test`)
- **Uninitialized memory**: reading uninitialized `u8`, `bool`, or enum
  discriminants
- **Validity invariants**: passing invalid values to `std` FFI boundaries
  (e.g., a dangling pointer to `CString::new`)
- **Stacked borrows**: violation of Rust's aliasing rules through raw pointers

### What Miri does NOT check

- **System calls**: Miri intercepts a subset of libc. `mmap`, `fork`,
  `ioctl`, and most OS-level syscalls are not modeled.
- **FFI safety**: `extern "C"` blocks execute host code; Miri cannot verify
  their correctness.
- **Performance**: Miri runs 10–100× slower than native; it is not a
  benchmarking tool.
- **No `unsafe` code without UB**: Miri cannot prove absence of UB — it
  only detects it when triggered by test inputs.

### Project configuration

- **Flag**: `MIRIFLAGS="-Zmiri-disable-isolation"` — allows access to the
  filesystem and environment variables from within Miri.
- **Target crate**: `arc-algebra` — the core algebraic operations crate
  contains the most concentrated unsafe code outside of mmap.
- **CI timeout**: 30 minutes (Miri is significantly slower than native tests).
- **Failure mode**: Miri failures block CI (non-`continue-on-error`).

### Limitations for this project

| Block | Why Miri can't verify it |
|-------|--------------------------|
| `memmap2::Mmap::map` (#1, 3–5) | `mmap` is a syscall outside Miri's model |
| `libc::geteuid()` (#2) | Raw FFI call to libc |
| `std::env::set_var` (#7–16) | Miri models env vars but the mutex pattern is sound |

Miri adds value by catching UB in pure-Rust unsafe code (e.g., pointer
derivation, slice indexing, union reads) even though it cannot verify the
syscall-based blocks.
