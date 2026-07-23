use std::{
    collections::{HashMap, HashSet},
    fmt,
};

use arc_change::Change;
use arc_store_cas::ObjectStore;
use arc_store_types::newtypes::ChangeId;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tracing::instrument;

/// Native transport errors emitted by the sync protocol surface.
#[derive(Debug, Error)]
pub enum NetError {
    /// I/O operation failed.
    #[error("I/O failure: {0}")]
    Io(#[from] std::io::Error),
    /// Underlying CAS operation failed.
    #[error("CAS operation failed: {0}")]
    Cas(#[from] arc_store_cas::cas::CasError),
    /// Codec/serialization operation failed.
    #[error("serialization failure: {0}")]
    Serialization(String),
    /// A requested hash did not match downloaded payload bytes.
    #[error("hash verification failed for {0}")]
    HashVerification(String),
    /// Peer protocol contract was violated.
    #[error("protocol violation: {0}")]
    Protocol(String),
}

impl From<anyhow::Error> for NetError {
    fn from(value: anyhow::Error) -> Self {
        NetError::Protocol(value.to_string())
    }
}

/// Wire representation of one CAS block transferred during sync.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CasWireBlock {
    /// Content-addressed hash for `bytes`.
    pub hash: [u8; 32],
    /// Serialized immutable CAS bytes.
    pub bytes: Vec<u8>,
}

/// Native sync protocol abstraction used by CLI sync orchestration.
#[async_trait]
pub trait SyncProtocol {
    /// Exchange caller frontier with peer and return peer frontier.
    async fn exchange_frontiers(
        &self,
        local_frontier: Vec<blake3::Hash>,
    ) -> Result<Vec<blake3::Hash>, NetError>;

    /// Fetch immutable CAS blocks for the requested hashes.
    async fn fetch_cas_blocks(&self, missing_hashes: &[blake3::Hash]) -> Result<Vec<u8>, NetError>;
}

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
pub const SERVER_CAPABILITIES: &[SyncCapability] =
    &[SyncCapability::PayloadStreamV1, SyncCapability::KeepAlive, SyncCapability::TypedChangeId];

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
    /// Frontier vector supplied by the caller.
    #[serde(default)]
    pub frontier: Vec<[u8; 32]>,
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
    /// Frontier vector exposed by the responder.
    #[serde(default)]
    pub remote_frontier: Vec<[u8; 32]>,
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

/// Compute hashes reachable from `remote_frontier` but absent from
/// `local_frontier` by traversing the local DAG closure.
#[instrument(skip_all)]
pub fn compute_missing_hashes(
    store: &ObjectStore,
    local_frontier: &[blake3::Hash],
    remote_frontier: &[blake3::Hash],
) -> Result<Vec<blake3::Hash>, NetError> {
    let local = reachable_set(store, local_frontier)?;
    let mut missing = Vec::new();
    let mut queued: HashSet<[u8; 32]> = HashSet::new();
    let mut stack: Vec<[u8; 32]> = remote_frontier.iter().map(|h| *h.as_bytes()).collect();

    while let Some(current) = stack.pop() {
        if !queued.insert(current) {
            continue;
        }
        if local.contains(&current) {
            continue;
        }

        let id = ChangeId::from(current);
        let raw = match store.read_change_bytes(id) {
            Ok(bytes) => bytes,
            Err(_) => {
                missing.push(blake3::Hash::from(current));
                continue;
            }
        };
        let change: Change = bincode::deserialize(raw.as_ref())
            .map_err(|e| NetError::Serialization(e.to_string()))?;
        if change.id != current {
            return Err(NetError::Protocol(format!(
                "change id mismatch while traversing missing closure: expected {}, found {}",
                id.to_hex(),
                ChangeId::from(change.id).to_hex()
            )));
        }

        missing.push(blake3::Hash::from(current));
        stack.extend(change.deps.iter().copied());
    }

    missing.reverse();
    Ok(missing)
}

fn reachable_set(
    store: &ObjectStore,
    frontier: &[blake3::Hash],
) -> Result<HashSet<[u8; 32]>, NetError> {
    let mut seen = HashSet::new();
    let mut stack: Vec<[u8; 32]> = frontier.iter().map(|h| *h.as_bytes()).collect();

    while let Some(current) = stack.pop() {
        if !seen.insert(current) {
            continue;
        }

        let id = ChangeId::from(current);
        let raw = match store.read_change_bytes(id) {
            Ok(bytes) => bytes,
            Err(_) => continue,
        };
        let change: Change = bincode::deserialize(raw.as_ref())
            .map_err(|e| NetError::Serialization(e.to_string()))?;
        if change.id != current {
            return Err(NetError::Protocol(format!(
                "change id mismatch while traversing frontier closure: expected {}, found {}",
                id.to_hex(),
                ChangeId::from(change.id).to_hex()
            )));
        }
        stack.extend(change.deps.iter().copied());
    }

    Ok(seen)
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
            frontier: Vec::new(),
        };

        let rendered = format!("{request:?}");
        assert!(rendered.contains("<redacted>"));
        assert!(!rendered.contains("secret-token"));
    }

    #[test]
    fn negotiate_capabilities_passes_matching() {
        let request = HandshakeRequest {
            version: 1,
            min_version: 1,
            auth_token: None,
            view_heads: HashMap::new(),
            required_capabilities: vec![
                SyncCapability::PayloadStreamV1,
                SyncCapability::TypedChangeId,
            ],
            optional_capabilities: vec![SyncCapability::KeepAlive],
            frontier: Vec::new(),
        };
        let (negotiated, rejected) = negotiate_capabilities(&request, SERVER_CAPABILITIES);
        assert!(negotiated.contains(&SyncCapability::PayloadStreamV1));
        assert!(negotiated.contains(&SyncCapability::TypedChangeId));
        assert!(negotiated.contains(&SyncCapability::KeepAlive));
        assert!(rejected.is_empty());
    }

    #[test]
    fn negotiate_capabilities_rejects_unsupported_required() {
        let request = HandshakeRequest {
            version: 1,
            min_version: 1,
            auth_token: None,
            view_heads: HashMap::new(),
            required_capabilities: vec![SyncCapability::ProgressSideband],
            optional_capabilities: vec![],
            frontier: Vec::new(),
        };
        let (negotiated, rejected) = negotiate_capabilities(&request, SERVER_CAPABILITIES);
        assert!(negotiated.is_empty());
        assert_eq!(rejected, vec![SyncCapability::ProgressSideband]);
    }

    #[test]
    fn negotiate_capabilities_deduplicates_negotiated() {
        let request = HandshakeRequest {
            version: 1,
            min_version: 1,
            auth_token: None,
            view_heads: HashMap::new(),
            required_capabilities: vec![SyncCapability::KeepAlive],
            optional_capabilities: vec![SyncCapability::KeepAlive, SyncCapability::KeepAlive],
            frontier: Vec::new(),
        };
        let (negotiated, _) = negotiate_capabilities(&request, SERVER_CAPABILITIES);
        let keepalive_count =
            negotiated.iter().filter(|c| **c == SyncCapability::KeepAlive).count();
        assert_eq!(keepalive_count, 1);
    }

    #[test]
    fn default_versions_are_bounded() {
        assert!(default_min_version() >= 1);
        assert!(default_negotiated_version() >= default_min_version());
    }

    #[test]
    fn cas_wire_block_roundtrip() {
        let block = CasWireBlock { hash: [0xAA; 32], bytes: vec![1, 2, 3] };
        let json = serde_json::to_string(&block).unwrap();
        let decoded: CasWireBlock = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.hash, [0xAA; 32]);
        assert_eq!(decoded.bytes, vec![1, 2, 3]);
    }

    #[test]
    fn handshake_request_defaults() {
        let request = HandshakeRequest {
            version: 1,
            min_version: 1,
            auth_token: None,
            view_heads: HashMap::new(),
            required_capabilities: vec![],
            optional_capabilities: vec![],
            frontier: vec![[0xAB; 32]],
        };
        assert_eq!(request.frontier.len(), 1);
        assert!(request.auth_token.is_none());
        assert!(request.required_capabilities.is_empty());
    }

    #[test]
    fn handshake_response_defaults() {
        let response = HandshakeResponse {
            status: 0,
            negotiated_version: 1,
            negotiated_capabilities: vec![],
            rejected_required_capabilities: vec![],
            required_changes: vec![],
            remote_frontier: vec![],
        };
        let json = serde_json::to_string(&response).unwrap();
        let decoded: HandshakeResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.status, 0);
        assert!(decoded.negotiated_capabilities.is_empty());
    }

    #[test]
    fn net_error_from_io() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "file missing");
        let net_err: NetError = io_err.into();
        assert!(matches!(net_err, NetError::Io(_)));
        assert!(net_err.to_string().contains("I/O failure"));
    }
}
