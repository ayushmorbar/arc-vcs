//! Network server for arc's content-addressable store.
//!
//! Exposes a minimal, read-only HTTP API over the local `.arc` store so that
//! remote peers can fetch [`arc_core::store::view::View`] states and raw
//! [`arc_core::store::change::Change`] objects.
//!
//! The server is intentionally stateless and read-only — writes always happen
//! through the local [`arc_core::store::cas::ObjectStore`] and are
//! cryptographically signed, so any tampering is detected by the client's
//! signature verification step.

#![warn(missing_docs)]

/// LLM provider abstractions and implementations used by `arc resolve`.
pub mod ai;

/// HTTP server exposing the arc CAS to remote peers for fetch and pull operations.
pub mod server;
