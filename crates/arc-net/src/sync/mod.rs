//! Native arc-to-arc sync protocol primitives.
//!
//! This module defines a compact framed transport and handshake payloads for
//! direct TCP synchronization between arc repositories, bypassing the Git bridge.

/// Native TCP sync client.
pub mod client;
/// Length-prefixed binary frame codec used over TCP streams.
pub mod codec;
/// Protocol handshake payloads and status envelopes.
pub mod protocol;
/// Native TCP sync server.
pub mod server;

/// 4-byte stream prelude identifying the native arc sync protocol.
pub const MAGIC_BYTES: &[u8; 4] = b"ARC\x01";
