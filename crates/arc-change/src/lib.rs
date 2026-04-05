//! BLUF: `arc-change` defines signed change envelopes and deterministic
//! content hashing for `arc`'s Spacetime DAG.
//!
//! It combines AST atoms, dependency edges, intent text, and Ed25519
//! provenance into replayable `Change` records whose identity is a stable
//! BLAKE3 digest.
//!
//! ## Purity and I/O boundary
//!
//! This crate is pure compute and cryptographic verification logic:
//! - No filesystem I/O
//! - No network I/O
//! - Deterministic hashing and signature validation only
//!
//! ## Why this crate exists
//!
//! `arc` treats change identity and provenance as first-class invariants.
//! Isolating this logic keeps CRDT replay, deduplication, and auditability
//! consistent across CLI, daemon, and storage layers.
//!
//! ## Example
//!
//! ```
//! use std::collections::HashSet;
//!
//! use arc_algebra_types::Atom;
//! use arc_change::Change;
//! use arc_store_types::author::test_keypair;
//!
//! let (author, signing_key) = test_keypair();
//! let change = Change::new(
//!     HashSet::new(),
//!     vec![Atom::Insert { at: vec!["main".into()], content_hash: [0u8; 32] }],
//!     "add main",
//!     author,
//!     &signing_key,
//! );
//! assert!(change.verify_signature());
//! ```

extern crate self as arc_core;

/// Signed immutable change model.
pub mod change;
/// Deterministic field-level hashing trait and derive support.
pub mod content_hash;

pub use change::*;
pub use content_hash::*;

/// Compatibility namespace for proc-macro paths used during crate extraction.
pub mod store {
    /// Compatibility re-export of content-hash symbols.
    pub mod content_hash {
        pub use crate::content_hash::*;
    }
}
