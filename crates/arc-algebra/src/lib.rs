//! BLUF: `arc-algebra` is the pure patch-theory core for `arc`.
//!
//! This crate contains the CRDT mathematics for change application,
//! commutativity checks, inversion, and sparse-boundary matching over AST
//! operations.
//!
//! ## Axiom of Purity
//!
//! This crate contains the pure CRDT patch-theory math (commutativity,
//! inversion, application). It does not perform filesystem or network I/O,
//! and it does not depend on any concrete storage implementation.
//!
//! ## Why this crate exists
//!
//! Isolating patch-theory math from storage and transport keeps replay laws,
//! algebraic guarantees, and conflict reasoning deterministic and reusable
//! across CLI, daemon, and network layers.
//!
//! ## Example
//!
//! ```
//! use arc_algebra::sparse::SparseMatcher;
//!
//! let matcher = SparseMatcher::from_patterns(&["src".to_string()]);
//! assert!(matcher.matches_file_path("src/lib.rs"));
//! ```

use arc_algebra_types::Blake3Hash;

/// Pure algebra-facing blob reader abstraction.
///
/// This trait is the storage boundary for `arc-algebra`: callers provide blob
/// bytes and existence checks, while patch-theory operations remain free of any
/// filesystem/CAS implementation dependency.
pub trait BlobStore {
    /// Read raw blob bytes by BLAKE3 hash.
    fn read_blob(&self, hash: &Blake3Hash) -> Result<Vec<u8>, String>;

    /// Return whether a blob hash exists.
    fn contains_blob(&self, hash: &Blake3Hash) -> bool;
}

pub mod apply;
pub mod commute;
pub mod inverse;
pub mod sparse;
