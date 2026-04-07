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
//! | [`ai`] (feature = `native`) | [`ai::AiResolver`] trait, [`ai::MockResolver`], and [`ai::generate_message`] for AST-aware commit generation |
//! | [`engine`] | [`engine::spacetime`] — algebraic history rewriting: `squash_into`, diffedit |
//! | [`store`] | CAS, [`store::change::Change`], [`store::graph::ChangeGraph`], [`store::view::View`], author identity |
//! | [`store::oplog`] | [`store::oplog::Operation`], [`store::oplog::OpLog`] — append-only spacetime ledger with O(1) undo |

#![warn(missing_docs)]

#[cfg(all(feature = "native", not(target_arch = "wasm32")))]
pub use arc_ai as ai;
/// Core algebraic types: atoms, hashes, commutativity, and change application.
pub mod algebra;
/// Spacetime engine: algebraic history rewriting (squash, diffedit).
pub mod engine;
/// Error types for this crate.
pub use arc_error as error;
/// Pure-Rust Git interoperability bridge for reading legacy repositories.
pub use arc_git as git_bridge;
/// Top-level repository facade with state-split handles and open options.
pub mod repository;
/// Async CRDT network transport (push/pull via HTTP + rustls TLS) when `native` is enabled.
#[cfg(all(feature = "native", not(target_arch = "wasm32")))]
pub use arc_network as network;
/// Revset grammar and parser for DAG query expressions.
pub mod revset;
/// Generic operation contexts with dual output channels.
pub mod operation_context;
/// Shared taxonomy for staged operations and tracing semantics.
pub mod ops;
pub mod store;
/// Virtual filesystem abstraction for CAS-backed materialized views.
pub mod vfs;
