//! # arc-core
//!
//! Pure algebraic foundation for `arc — Atomic Replayable Changes`.
//!
//! This crate contains the physics of `arc`: the typed atom vocabulary,
//! the content-addressable store, the cryptographic change graph, and
//! author identity primitives.  It has **zero** dependency on language
//! plugins, network code, or CLI concerns — making it a stable, auditable
//! foundation that the other crates build upon.
//!
//! ## Crate layout
//!
//! | Module | Responsibility |
//! |---|---|
//! | [`algebra`] | Core types: [`Atom`], [`Blake3Hash`], commutativity, change application |
//! | [`ai`] | [`AiResolver`] trait and [`MockResolver`] for conflict resolution |
//! | [`store`] | CAS, [`Change`], [`ChangeGraph`], [`View`], author identity |

#![warn(missing_docs)]

pub mod ai;
pub mod algebra;
pub mod store;
