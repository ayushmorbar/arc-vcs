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

/// Ghost Node state machine for pending AI-authored changes.
pub mod ai_pending;
/// Anonymized DAG telemetry packager for `arc bugreport`.
pub mod bugreport;
/// `arc generate` — agentic code generation with semantic context.
pub mod generate;
/// Git repository interoperability tools.
pub mod interop;
/// Top-level repository operations and VCS commands.
pub mod repo;
/// Semantic text diff rendering: Sesame alignment, intent annotation, and
/// BDiff-inspired inline sub-expression highlighting.
pub mod semantic_diff;
/// Fetch and pull synchronization primitives.
pub mod sync;
