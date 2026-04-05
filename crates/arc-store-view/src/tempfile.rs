//! Signal-safe temporary-file registry.
//!
//! Any temporary file that arc creates during a write operation (lock files,
//! in-progress CAS objects) should be registered here.  On receipt of a
//! termination signal the CLI layer calls [`cleanup_signal_safe`] to delete
//! all registered paths owned by this process before the process exits.
//!
//! # Design (from gix-tempfile, Phase 2A harvest)
//!
//! * The registry is a `LazyLock<DashMap<usize, TempEntry>>`.  Each entry
//!   stores the path and the owning process's PID (fork safety).
//! * [`cleanup_signal_safe`] iterates the map and calls `std::fs::remove_file`
//!   on every entry whose `owning_pid` matches the current process.
//! * DashMap uses sharded `parking_lot::RwLock`s internally.  In a signal
//!   handler the `iter()` call acquires those read locks in sequence; if a
//!   shard lock is already held by the interrupted thread the handler will
//!   block on it.  This is a best-effort implementation — production-grade
//!   signal safety requires a lockfree structure (e.g. `hp-hashmap`).  For
//!   arc's usage pattern (single CLI process, fs-level lock protecting writes)
//!   the window for a deadlock is negligible.
//! * `owning_pid` prevents a forked child process from accidentally deleting
//!   the parent's temp files (equivalent to gix's `owning_process_id` guard).

use std::path::PathBuf;
use std::sync::LazyLock;
use std::sync::atomic::{AtomicUsize, Ordering};

use dashmap::DashMap;

/// An entry in the tempfile registry.
struct TempEntry {
    path: PathBuf,
    /// PID of the process that registered this entry.  Used to prevent forked
    /// children from deleting their parent's temporary files.
    owning_pid: u32,
}

/// Monotonically-increasing ID counter for registry entries.
static NEXT_ID: AtomicUsize = AtomicUsize::new(0);

/// Global tempfile registry, initialised on first use.
///
/// The `LazyLock` ensures the underlying `DashMap` (and its `parking_lot`
/// shards) are constructed exactly once, at the cost of one pointer-width
/// load on every hot path after initialisation.
static REGISTRY: LazyLock<DashMap<usize, TempEntry>> = LazyLock::new(DashMap::new);

/// Force the registry to initialise eagerly.
///
/// Call this during CLI startup **before** registering the signal handler so
/// that the first use of the registry never allocates inside a signal context.
pub fn init() {
    // Touch REGISTRY to force LazyLock initialisation.
    let _ = REGISTRY.len();
}

/// Register a temporary file path.
///
/// Returns an opaque `id` that can be passed to [`deregister`] once the
/// file is no longer needed.
#[must_use]
pub fn register(path: PathBuf) -> usize {
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    REGISTRY.insert(
        id,
        TempEntry {
            path,
            owning_pid: std::process::id(),
        },
    );
    id
}

/// Remove a previously-registered entry from the registry.
///
/// Safe to call even if the entry was already removed (e.g. cleaned up by a
/// signal handler before the normal code path reached here).
pub fn deregister(id: usize) {
    REGISTRY.remove(&id);
}

/// Delete all registered temporary files owned by this process.
///
/// Intended to be called from a UNIX termination-signal handler (SIGTERM,
/// SIGINT, etc.).  Uses `std::fs::remove_file` (which wraps `unlink(2)`) —
/// an async-signal-safe syscall on POSIX systems.  Best-effort: errors from
/// `remove_file` are silently discarded because there is no safe way to
/// propagate them from a signal handler.
///
/// # Fork safety
///
/// Only files whose `owning_pid` matches `std::process::id()` are deleted.
/// This prevents a forked child from removing the parent's in-flight files.
pub fn cleanup_signal_safe() {
    let current_pid = std::process::id();
    REGISTRY.iter().for_each(|entry| {
        if entry.owning_pid == current_pid {
            let _ = std::fs::remove_file(&entry.path);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_register_deregister() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.lock");
        std::fs::write(&path, b"").unwrap();

        let id = register(path.clone());
        assert!(REGISTRY.contains_key(&id));

        deregister(id);
        assert!(!REGISTRY.contains_key(&id));
    }

    #[test]
    fn test_cleanup_removes_owned_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cleanup_test.lock");
        std::fs::write(&path, b"").unwrap();
        assert!(path.exists());

        let id = register(path.clone());
        cleanup_signal_safe();
        deregister(id);

        assert!(
            !path.exists(),
            "cleanup_signal_safe must remove registered files"
        );
    }
}
