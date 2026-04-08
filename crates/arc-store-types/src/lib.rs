//! BLUF: `arc-store-types` is the shared type spine for storage and identity.
//!
//! It defines strongly-typed IDs, author/provenance identities, signed tags,
//! and reference-reading helpers that connect persisted state to the Spacetime
//! DAG.
//!
//! ## Purity and I/O boundary
//!
//! This crate is mostly pure types plus light local I/O helpers:
//! - Pure data model: IDs, author enums, signatures, tags.
//! - Local I/O only: identity and ref file loading helpers.
//! - No network I/O.
//!
//! ## Why this crate exists
//!
//! `arc` needs one canonical place for Ed25519 provenance types and durable
//! identifier wrappers so all crates agree on cryptographic and storage
//! semantics without importing full CAS or DAG engines.
//!
//! ## Example
//!
//! ```
//! use arc_store_types::newtypes::ChangeId;
//!
//! let id = ChangeId::from_hex(
//!     "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
//! ).expect("valid hex id");
//! assert_eq!(id.to_hex().len(), 64);
//! ```

#![no_std]

extern crate alloc;
#[cfg(feature = "std")]
extern crate std;

/// Author identity types and Ed25519 signature wrappers.
pub mod author;
/// Newtype IDs for changes, blobs, snapshots, and mutations.
pub mod newtypes;
/// Reference readers for tags, bookmarks, and remote branches.
#[cfg(feature = "std")]
pub mod refs;
/// Immutable cryptographically signed tag model.
#[cfg(feature = "std")]
pub mod tag;

/// Local canonical 32-byte BLAKE3 hash type used by store primitives.
pub type Blake3Hash = [u8; 32];

pub use author::{Author, PublicKeyBytes, Signature};
pub use newtypes::ChangeId;
