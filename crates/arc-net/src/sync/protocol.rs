use std::collections::HashMap;

use arc_core::algebra::Blake3Hash;
use serde::{Deserialize, Serialize};

/// Initial client hello for native arc sync.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HandshakeRequest {
    /// Protocol version requested by the client.
    pub version: u32,
    /// Caller view frontier by view name.
    pub view_heads: HashMap<String, Blake3Hash>,
}

/// Server response to a [`HandshakeRequest`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HandshakeResponse {
    /// Handshake status code (`0` = OK, `1` = unsupported version).
    pub status: u8,
    /// Object hashes the server needs from the client.
    pub required_hashes: Vec<Blake3Hash>,
}
