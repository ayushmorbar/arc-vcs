//! CLI library for arc — Atomic Replayable Changes.
//!
//! This crate ties together [`arc_core`], [`arc_lang`], and [`arc_net`] into
//! a complete version-control tool.  The public surface is intentionally
//! small: the `[[bin]]` target (`arc`) drives everything through
//! [`clap`]-powered subcommands.
//!
//! # Internal modules
//!
//! - [`repo`] — Top-level repository handle and all VCS operations.
//! - [`sync`] — Fetch / pull primitives for peer-to-peer sync.
//! - [`interop`] — Importers from other VCS systems.

#![warn(missing_docs)]

/// Git repository interoperability tools.
pub mod interop;
/// Top-level repository operations and VCS commands.
pub mod repo;
/// Fetch and pull synchronization primitives.
pub mod sync;
