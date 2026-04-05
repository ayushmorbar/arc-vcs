//! Spacetime engine: algebraic history rewriting (squash, reorder).
//!
//! This module exposes the higher-level operations that compose inversion
//! ([`crate::algebra::inverse`]) and commutation ([`crate::algebra::commute`])
//! into user-visible commands:
//!
//! - [`spacetime::squash_into`] — fuse a contiguous linear spine into a target change.
//! - [`mutator`] — rewrite-safe squash/reorder primitives with typed rewrite maps.
pub use arc_engine::mutator;
pub use arc_engine::spacetime;
