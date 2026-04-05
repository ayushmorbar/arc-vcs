//! CRDT sync transport protocol and blob transfer client for arc.
//!
//! This crate owns transport-layer wire types and HTTP client helpers used for
//! sync payload exchange and blob upload/fetch between peers.
//!
//! It handles protocol serialization, remote request execution, and payload
//! verification boundaries, but it does NOT start or manage a tokio runtime.
//! Runtime ownership remains at caller boundaries (CLI/daemon layers).

#![warn(missing_docs)]

pub mod network;
pub mod transport;
pub use network::*;
