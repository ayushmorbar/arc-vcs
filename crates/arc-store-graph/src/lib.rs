//! BLUF: `arc-store-graph` owns pure DAG topology logic for `arc`.
//!
//! This crate provides in-memory change-graph traversal, deterministic
//! topological ordering, merge-base calculation, and bisect state management
//! over the Spacetime DAG.
//!
//! ## Purity and I/O boundary
//!
//! `arc-store-graph` is intentionally compute-focused:
//! - DAG traversal and ancestry algorithms are pure in-memory operations.
//! - Bisect decision logic is deterministic over graph state.
//! - No CAS or network I/O is performed by graph traversal APIs.
//!
//! ## Why this crate exists
//!
//! Separating DAG topology from storage engines keeps CRDT replay and revset
//! semantics independent from disk persistence concerns while preserving clean
//! layering above `arc-change` and below higher execution engines.
//!
//! ## Example
//!
//! ```
//! use arc_store_graph::ChangeGraph;
//!
//! let g = ChangeGraph::new();
//! assert!(g.is_empty());
//! ```

/// Deterministic bisect state machine and persistence helpers.
pub mod bisect;
/// Digest-key optimized hasher utilities for DAG maps.
pub mod blake3_hasher;
/// In-memory change DAG traversal and topology algorithms.
pub mod graph;
/// Native-only scoped worker orchestration for parallel DAG passes.
#[cfg(all(not(target_arch = "wasm32"), feature = "native-workers"))]
pub mod scoped_workers;
/// Compact bitflag-based node traversal metadata.
pub mod traversal_state;

pub use bisect::*;
pub use blake3_hasher::{Blake3HashMap, Blake3Hasher};
pub use graph::*;
#[cfg(all(not(target_arch = "wasm32"), feature = "native-workers"))]
pub use scoped_workers::*;
pub use traversal_state::*;
