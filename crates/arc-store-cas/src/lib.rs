//! BLUF: `arc-store-cas` encapsulates local content-addressed storage I/O.
//!
//! It is the boundary where `arc` persists and retrieves BLAKE3-addressed
//! objects and blobs, and where hash-map performance primitives are tuned for
//! digest keys used by Spacetime-DAG traversal.
//!
//! ## Purity and I/O boundary
//!
//! This crate is an explicit disk-I/O boundary:
//! - Reads/writes CAS objects and blobs under `.arc/`.
//! - Uses memory-mapped reads for larger immutable blobs.
//! - Contains no network I/O and no Ed25519 key operations.
//!
//! ## Why this crate exists
//!
//! Separating CAS I/O from algebra and provenance logic keeps replay math and
//! CRDT semantics pure while letting storage implementation details evolve
//! independently (layout, mmap policy, durability strategy).
//!
//! ## Example
//!
//! ```no_run
//! let store = arc_store_cas::ObjectStore::new(".");
//! let hash = store.write_blob(b"hello")?;
//! let bytes = store.read_blob(&hash)?;
//! assert_eq!(&*bytes, b"hello");
//! # Ok::<(), arc_store_cas::CasError>(())
//! ```

/// Hash-map and hasher types optimized for BLAKE3 digest keys.
pub mod blake3_hasher;
/// Content-addressable storage engine and byte container types.
pub mod cas;

pub use blake3_hasher::*;
pub use cas::*;
