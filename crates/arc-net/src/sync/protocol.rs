use std::collections::HashMap;
use std::fmt;

use arc_store_types::newtypes::ChangeId;
use serde::{Deserialize, Serialize};
use tracing::instrument;

/// Negotiable transport capabilities for native arc sync.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, PartialOrd, Ord)]
#[repr(u8)]
pub enum SyncCapability {
    /// Framed payload stream exchange for change transfer.
    PayloadStreamV1 = 0x01,
    /// Keep-alive frames to avoid long-transfer idle disconnects.
    KeepAlive = 0x02,
    /// Out-of-band progress frame channel.
    ProgressSideband = 0x03,
    /// Typed change identifiers (`ChangeId`) on wire metadata.
    TypedChangeId = 0x04,
}

/// Server capabilities currently implemented by this binary.
pub const SERVER_CAPABILITIES: &[SyncCapability] = &[
    SyncCapability::PayloadStreamV1,
    SyncCapability::KeepAlive,
    SyncCapability::TypedChangeId,
];

/// Initial client hello for native arc sync.
#[derive(Clone, Serialize, Deserialize)]
pub struct HandshakeRequest {
    /// Protocol version requested by the client.
    pub version: u32,
    /// Lowest protocol version accepted by the client.
    #[serde(default = "default_min_version")]
    pub min_version: u32,
    /// Optional shared-secret token for remote sync authorization.
    #[serde(default)]
    pub auth_token: Option<String>,
    /// Caller view frontier by view name.
    pub view_heads: HashMap<String, ChangeId>,
    /// Capabilities required for this connection to proceed.
    #[serde(default)]
    pub required_capabilities: Vec<SyncCapability>,
    /// Capabilities preferred by the caller but not mandatory.
    #[serde(default)]
    pub optional_capabilities: Vec<SyncCapability>,
}

impl fmt::Debug for HandshakeRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let redacted_token = self.auth_token.as_ref().map(|_| "<redacted>");
        f.debug_struct("HandshakeRequest")
            .field("version", &self.version)
            .field("min_version", &self.min_version)
            .field("auth_token", &redacted_token)
            .field("view_heads", &self.view_heads)
            .field("required_capabilities", &self.required_capabilities)
            .field("optional_capabilities", &self.optional_capabilities)
            .finish()
    }
}

/// Server response to a [`HandshakeRequest`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HandshakeResponse {
    /// Handshake status code (`0` = OK, `1` = unsupported version,
    /// `2` = unauthorized, `3` = required capability missing).
    pub status: u8,
    /// Negotiated protocol version.
    #[serde(default = "default_negotiated_version")]
    pub negotiated_version: u32,
    /// Capabilities accepted for this session.
    #[serde(default)]
    pub negotiated_capabilities: Vec<SyncCapability>,
    /// Required capabilities rejected by the server.
    #[serde(default)]
    pub rejected_required_capabilities: Vec<SyncCapability>,
    /// Change ids the server needs from the client.
    #[serde(default, alias = "required_hashes")]
    pub required_changes: Vec<ChangeId>,
}

const fn default_min_version() -> u32 {
    1
}

const fn default_negotiated_version() -> u32 {
    1
}

/// Negotiate version/capabilities deterministically between client and server.
#[instrument(skip_all)]
pub fn negotiate_capabilities(
    request: &HandshakeRequest,
    server_capabilities: &[SyncCapability],
) -> (Vec<SyncCapability>, Vec<SyncCapability>) {
    let mut negotiated: Vec<SyncCapability> = request
        .required_capabilities
        .iter()
        .chain(request.optional_capabilities.iter())
        .copied()
        .filter(|cap| server_capabilities.contains(cap))
        .collect();
    negotiated.sort_unstable();
    negotiated.dedup();

    let mut rejected_required: Vec<SyncCapability> = request
        .required_capabilities
        .iter()
        .copied()
        .filter(|cap| !server_capabilities.contains(cap))
        .collect();
    rejected_required.sort_unstable();
    rejected_required.dedup();

    (negotiated, rejected_required)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handshake_request_debug_redacts_auth_token() {
        let request = HandshakeRequest {
            version: 1,
            min_version: 1,
            auth_token: Some("secret-token".to_string()),
            view_heads: HashMap::new(),
            required_capabilities: vec![SyncCapability::PayloadStreamV1],
            optional_capabilities: vec![],
        };

        let rendered = format!("{request:?}");
        assert!(rendered.contains("<redacted>"));
        assert!(!rendered.contains("secret-token"));
    }
}
