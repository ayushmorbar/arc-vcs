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
/// Interactive onboarding commands.
pub mod commands;
/// Internal runtime diagnostics and command orchestration helpers.
pub mod devtools;
/// `arc generate` — agentic code generation with semantic context.
pub mod generate;
/// GitHub governance and CI policy auditing utilities.
pub mod governance;
/// ASCII DAG renderer for `arc log` output.
pub mod graph_render;
/// Git repository interoperability tools.
pub mod interop;
/// Progress UI primitives for spinners and staged sync pipelines.
pub mod progress;
/// Top-level repository operations and VCS commands.
pub mod repo;
/// Semantic text diff rendering: Sesame alignment, intent annotation, and
/// BDiff-inspired inline sub-expression highlighting.
pub mod semantic_diff;
mod store_compat;
/// Fetch and pull synchronization primitives.
pub mod sync;
/// Typed workspace tooling policy audit utilities.
pub mod tooling;
/// Root workspace policy audit utilities.
pub mod workspace_policy;
