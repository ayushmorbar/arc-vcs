//! Crash-consistent persistence for arc mutable state.
//!
//! This crate owns on-disk persistence for mutable pointers (`View`) and the
//! append-only operation log (`OpLog`). It provides crash-consistent
//! read/write paths for view frontiers, transactional operation history,
//! synthesis snapshots, and signal-safe tempfile tracking used by write
//! workflows.
//!
//! Disk payloads are serialized with `bincode`. Persistence paths use
//! atomic-rename write patterns (`*.tmp` then `rename`), with explicit
//! file/directory sync barriers where implemented (`OpLog`, synthesis).

#![warn(missing_docs)]

/// Crash-consistent JSON checkpoint persistence helpers.
pub mod checkpoint;
/// Lock-file primitives for crash-consistent mutable pointer publication.
pub mod lock;
/// Append-only transaction log with optimistic head publication.
pub mod oplog;
/// Content-addressed synthesis snapshot capture and storage.
pub mod synthesis;
/// Process-scoped tempfile registry for signal-time cleanup.
pub mod tempfile;
/// Virtual filesystem boundary for CAS-backed projections.
pub mod vfs;
/// Crash-consistent persistence of mutable `View` pointers.
pub mod view;

pub use checkpoint::*;
pub use lock::*;
pub use oplog::*;
pub use synthesis::*;
pub use tempfile::*;
pub use vfs::*;
pub use view::*;

/// Errors produced by persistent view serialization and filesystem access.
#[derive(Debug)]
pub enum StoreError {
    /// An I/O error from the filesystem.
    Io(std::io::Error),

    /// A bincode serialization or deserialization error.
    Serialization(Box<bincode::ErrorKind>),
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(err) => write!(f, "I/O error: {err}"),
            Self::Serialization(err) => write!(f, "serialization error: {err}"),
        }
    }
}

impl std::error::Error for StoreError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(err) => Some(err),
            Self::Serialization(err) => Some(err),
        }
    }
}

impl From<std::io::Error> for StoreError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

impl From<Box<bincode::ErrorKind>> for StoreError {
    fn from(value: Box<bincode::ErrorKind>) -> Self {
        Self::Serialization(value)
    }
}
