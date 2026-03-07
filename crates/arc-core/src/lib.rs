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
//! | [`algebra`] | Core types: [`algebra::Atom`], [`algebra::Blake3Hash`], commutativity, change application |
//! | [`ai`] | [`ai::AiResolver`] trait, [`ai::MockResolver`], and [`ai::generate_message`] for AST-aware commit generation |
//! | [`store`] | CAS, [`store::change::Change`], [`store::graph::ChangeGraph`], [`store::view::View`], author identity |

#![warn(missing_docs)]

pub mod ai;
/// Core algebraic types: atoms, hashes, commutativity, and change application.
pub mod algebra;
/// Pure‑Rust Git interoperability bridge for reading legacy repositories.
pub mod git_bridge;
/// Async CRDT network transport (push/pull via HTTP + rustls TLS).
pub mod network;
pub mod store;
