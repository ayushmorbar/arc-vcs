//! IDE daemon backend for arc.
//!
//! Provides a lightweight JSON-RPC server over stdin/stdout so editors can
//! query repository state without repeatedly spawning CLI commands.

#![warn(missing_docs)]

/// Cross-platform filesystem watcher and debounced autosnapshot loop.
pub mod watcher;
pub use watcher::AutoSnapDaemon;

/// JSON-RPC protocol types.
#[cfg(feature = "rpc-server")]
pub mod protocol;
/// Async server loop and method handlers.
#[cfg(feature = "rpc-server")]
pub mod server;
