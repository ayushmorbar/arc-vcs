//! Spacetime engine: algebraic history rewriting (squash, diffedit).
//!
//! This module exposes the higher-level operations that compose inversion
//! ([`crate::algebra::inverse`]) and commutation ([`crate::algebra::commute`])
//! into user-visible commands:
//!
//! - [`spacetime::squash_into`] — fuse a contiguous linear spine into a target change.
pub mod spacetime;
