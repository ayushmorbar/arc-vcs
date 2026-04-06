//! Platform filesystem foundations for arc.
//!
//! This crate centralizes lockfile publication and tempfile lifecycle
//! primitives so higher-level crates can depend on a single, governed
//! foundation surface.

#![warn(missing_docs)]

/// Lock-file primitives for crash-consistent mutable pointer publication.
pub mod lock;
/// Process-scoped tempfile registry for signal-time cleanup.
pub mod tempfile;

pub use lock::*;
pub use tempfile::*;
