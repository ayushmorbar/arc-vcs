//! IDE daemon backend for arc.
//!
//! Provides a lightweight JSON-RPC server over stdin/stdout so editors can
//! query repository state without repeatedly spawning CLI commands.

#![warn(missing_docs)]

/// JSON-RPC protocol types.
pub mod protocol;
/// Async server loop and method handlers.
pub mod server;
