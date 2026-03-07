//! CLI library for arc — Atomic Replayable Changes.
//!
//! This crate ties together [`arc_core`], [`arc_lang`], and [`arc_net`] into
//! a complete version-control tool.  The public surface is intentionally
//! small: the `[[bin]]` target (`arc`) drives everything through
//! [`clap`]-powered subcommands.
//!
//! # Internal modules
//!
//! - [`repo`] — Top-level repository handle and all VCS operations (snap, merge, undo, op_log, …).
//! - [`sync`] — Fetch / pull primitives for peer-to-peer sync.
//! - [`interop`] — Importers from other VCS systems.
//! - [`semantic_diff`] — Sesame-aligned semantic text diff rendering engine.

#![warn(missing_docs)]

/// Git repository interoperability tools.
pub mod interop;
/// Top-level repository operations and VCS commands.
pub mod repo;
/// Semantic text diff rendering: Sesame alignment, intent annotation, and
/// BDiff-inspired inline sub-expression highlighting.
pub mod semantic_diff;
/// Fetch and pull synchronization primitives.
pub mod sync;
