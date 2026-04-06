//! BLUF: This crate orchestrates high-level history rewriting (squashing,
//! reorder mutation) by combining the pure CRDT math of `arc-algebra`
//! with the DAG topology of `arc-store-graph`.
//!
//! `spacetime` provides safe spine-level rewrite operations that preserve
//! causal dependencies while rebuilding content-addressed change identities.
//! Rewrites are computed from explicit graph ancestry and commutativity checks
//! to avoid data loss during squash/reorder workflows.

#![warn(missing_docs)]
#![allow(ambiguous_glob_reexports)]

pub mod mutator;
pub mod spacetime;
pub mod task_harness;

pub use mutator::*;
pub use spacetime::*;
pub use task_harness::*;
