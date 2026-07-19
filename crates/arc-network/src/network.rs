//! Async CRDT network transport — Phase 39: Distributed Scale.
//!
//! Provides [`NetworkClient`] and the [`DeltaPayload`] / [`SyncResponse`]
//! wire types that carry the exact missing slice of a mathematical change
//! graph between arc peers.
//!
//! # Protocol summary (Phase 39)
//!
//! * **Blob upload** — Before the sync POST, the caller streams each CAS blob
//!   to `PUT {remote}/blobs/{hash}`.  The server verifies the BLAKE3 hash,
//!   writes to a temp file, and atomically renames it into `.arc/blobs/`.
//!   This decouples the data plane (blobs) from the control plane (DAG
//!   metadata), preventing OOM on large binary-asset pushes.
//!
//! * **Push** — The caller then POSTs a [`DeltaPayload`] (Changes + view
//!   heads, **no** inline blobs) to `POST {remote}/sync/{view_name}`.
//!   The server calls [`verify_payload`] *before* any CAS write (zero-trust
//!   ingress), runs Identity Collapsing if needed, advances its view, and
//!   returns a [`SyncResponse`] containing the canonical view heads and a
//!   `rewritten_map` of any collapsed Changes.
//!
//! * **Fetch blob** — `GET {remote}/blobs/{hex}` returns raw bytes for a
//!   single CAS blob.
//!
//! # Zero-trust ingress
//!
//! [`verify_payload`] checks every Ed25519 signature before any write.  An
//! attacker who tampers with a blob changes its `content_hash` in the atom,
//! which changes the `Change` id, which breaks the signature — making
//! supply-chain poisoning detectable at the cryptographic layer (SLSA L4).
//!
//! # Async boundary
//!
//! The `tokio` runtime is **never started inside this module** — callers boot
//! the runtime at the CLI edge.  `arc-cli/sync.rs` uses `reqwest::blocking`
//! for its synchronous push/pull helpers.
//!
//! # TLS
//!
//! Built with `reqwest`'s default TLS backend; no manual OpenSSL headers
//! required.

use std::collections::{HashMap, HashSet};

use anyhow::{Context, Result, anyhow};
use arc_keyring::ArcIdentity;
use arc_store_cas::cas::CasStorage;
use arc_store_types::Signature as ArcSignature;
use chrono::{DateTime, Duration, Utc};
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use arc_algebra_types::Blake3Hash;
use arc_change::Change;

const SIGNING_DOMAIN: &[u8] = b"arc-network:signed-sync:v1:";

// ── Wire type ──────────────────────────────────────────────────────────────

/// The wire type for a coordination-free push/sync operation.
///
/// A `DeltaPayload` carries exactly the subset of the local DAG that the
/// remote is missing.  CAS blobs are **not** included here — they are
/// uploaded out-of-band via `PUT /blobs/:hash` before this payload is
/// POSTed, keeping the JSON envelope small and memory usage flat even
/// for repositories with large binary assets.
///
/// The receiver can apply it with no back-and-forth negotiation because:
///
/// 1. **CAS writes are idempotent** — duplicate Changes are silently skipped.
/// 2. **View merge is a set union** — `new_heads = remote ∪ payload.view_heads`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeltaPayload {
    /// Slice of the DAG the remote is missing (BFS order from local heads).
    pub changes: Vec<Change>,
    /// The sender's current view heads.
    ///
    /// The receiver computes `new_heads = remote_heads ∪ view_heads` — a
    /// pure set union that requires zero coordination.
    pub view_heads: HashSet<Blake3Hash>,
}

/// Sender frontier summary used during discovery.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FrontierSummary {
    /// Frontier heads known by the caller.
    pub frontier: HashSet<Blake3Hash>,
}

/// Signed network envelope carrying serialized payload bytes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignedRequest {
    /// Serialized payload bytes (JSON for wire structs).
    pub payload: Vec<u8>,
    /// Request timestamp (UTC) used for anti-replay drift checks.
    pub timestamp: DateTime<Utc>,
    /// Sender verifying key bytes.
    pub author_public_key: [u8; 32],
    /// Ed25519 signature over payload + timestamp.
    pub signature: ArcSignature,
}

impl SignedRequest {
    /// Sign an arbitrary payload with an explicit keypair and timestamp.
    pub fn sign(payload: Vec<u8>, timestamp: DateTime<Utc>, signing_key: &SigningKey) -> Self {
        let signing_bytes = signing_bytes(&payload, &timestamp);
        let signature = signing_key.sign(&signing_bytes);
        Self {
            payload,
            timestamp,
            author_public_key: signing_key.verifying_key().to_bytes(),
            signature: ArcSignature(signature.to_bytes()),
        }
    }

    /// Sign with an ArcIdentity loaded from keyring.
    pub fn sign_with_identity(
        payload: Vec<u8>,
        timestamp: DateTime<Utc>,
        identity: &ArcIdentity,
    ) -> Self {
        let signing_key = SigningKey::from_bytes(&identity.signing_key);
        Self::sign(payload, timestamp, &signing_key)
    }

    /// Verify signature and replay window on incoming signed envelope.
    pub fn verify_incoming(&self, now: DateTime<Utc>) -> std::result::Result<(), NetworkError> {
        let drift = now.signed_duration_since(self.timestamp).num_seconds().abs();
        if drift > Duration::minutes(5).num_seconds() {
            return Err(NetworkError::Unauthorized);
        }

        let verifying_key = VerifyingKey::from_bytes(&self.author_public_key)
            .map_err(|_| NetworkError::Unauthorized)?;
        let signature = Signature::from_bytes(&self.signature.0);
        let signing_bytes = signing_bytes(&self.payload, &self.timestamp);

        verifying_key.verify(&signing_bytes, &signature).map_err(|_| NetworkError::Unauthorized)
    }

    /// Verify against a trusted expected author key.
    pub fn verify_incoming_for_author(
        &self,
        now: DateTime<Utc>,
        expected_author_public_key: &[u8; 32],
    ) -> std::result::Result<(), NetworkError> {
        if &self.author_public_key != expected_author_public_key {
            return Err(NetworkError::Unauthorized);
        }
        self.verify_incoming(now)
    }
}

// ── Sync response ──────────────────────────────────────────────────────────────────────────────

/// Server response to a successful `POST /sync/:view_name`.
///
/// Carries the server's new canonical view heads plus a map of any Changes
/// that were collapsed under Dual-Provenance Identity Collapsing (Phase 39).
///
/// # Identity Collapsing
///
/// When the server receives a payload containing transient-author Changes (or
/// Changes whose dependencies were collapsed and therefore have a
/// Cryptographic Cascade rewrite trigger), it re-signs them under
/// `Author::Server`, writes the canonical Change alongside the original in
/// CAS (`original.id` → `collapsed_from` pointer), and returns the mapping
/// here.  Clients update their local view to point at canonical heads.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncResponse {
    /// Server's view heads after applying the payload (canonical IDs).
    pub view_heads: HashSet<Blake3Hash>,
    /// Collapsed Changes: `hex(original_id)` → `hex(canonical_id)`.
    ///
    /// Empty when no identity collapsing occurred (the common case).
    /// JSON map keys are 64-character lowercase hex strings.
    pub rewritten_map: HashMap<String, String>,
}

/// Errors emitted by signed sync protocol operations.
#[derive(Debug, Error)]
pub enum NetworkError {
    /// Incoming request failed signature or replay-window checks.
    #[error("unauthorized request")]
    Unauthorized,
    /// Incoming payload failed structural or cryptographic validation.
    #[error("invalid payload: {0}")]
    InvalidPayload(String),
    /// Serialization or deserialization failed.
    #[error("serialization error: {0}")]
    Serialization(String),
    /// Persisting verified payload into CAS failed.
    #[error("cas write failed: {0}")]
    Storage(String),
    /// HTTP transport failed or returned non-success.
    #[error("network error: {0}")]
    Transport(String),
}

// ── Zero-trust ingress ─────────────────────────────────────────────────────

/// Verify every [`Change`] in `payload` carries a valid Ed25519 signature
/// and that its content address is self-consistent.
///
/// This is the **zero-trust ingress boundary**: the server must call this
/// *before* any write to its CAS.  An attacker who tampers with a blob
/// changes the `content_hash` in the atom, which changes the `Change` id,
/// which breaks the Ed25519 signature — making supply-chain attacks
/// detectable at the cryptographic layer.
///
/// # Errors
///
/// Returns an error naming the hex prefix of the first offending change.
pub fn verify_payload(payload: &DeltaPayload) -> Result<()> {
    for change in &payload.changes {
        if !change.verify_signature() {
            let hex: String = change.id.iter().map(|b| format!("{b:02x}")).collect();
            return Err(anyhow!(
                "zero-trust ingress: signature verification failed for change {}",
                &hex[..16]
            ));
        }
    }
    Ok(())
}

// ── HTTP client ────────────────────────────────────────────────────────────

/// Async CRDT-aware HTTP client for arc peer synchronisation.
///
/// Constructed once per network operation and discarded.  The underlying
/// `reqwest::Client` reuses keep-alive connections for the duration of a
/// single push/pull batch, then is dropped.
pub struct NetworkClient {
    client: Client,
}

impl NetworkClient {
    /// Build a new [`NetworkClient`].
    ///
    /// Configures the `User-Agent` header identifying the arc version.
    pub fn new() -> Result<Self> {
        let client = Client::builder()
            .user_agent(concat!("arc-vcs/", env!("CARGO_PKG_VERSION")))
            .build()
            .context("failed to build HTTP client")?;
        Ok(Self { client })
    }

    /// POST a [`DeltaPayload`] to `{remote_url}/sync/{view_name}`.
    ///
    /// Blobs must be uploaded out-of-band (via `PUT /blobs/:hash`) before
    /// calling this.  The server enforces this with a 409 response listing
    /// any missing blobs.
    ///
    /// The server is expected to:
    /// 1. Call [`verify_payload`] and reject with 400 on failure.
    /// 2. Run Dual-Provenance Identity Collapsing for transient-author Changes.
    /// 3. Write all changes to its CAS (idempotent).
    /// 4. Advance its view: `new_heads = remote_heads ∪ payload.view_heads`.
    /// 5. Return a [`SyncResponse`] with canonical view heads and any
    ///    `rewritten_map` entries.
    pub async fn push_payload(
        &self,
        remote_url: &str,
        view_name: &str,
        payload: &DeltaPayload,
    ) -> Result<SyncResponse> {
        let url = format!("{remote_url}/sync/{view_name}");
        let resp = self
            .client
            .post(&url)
            .json(payload)
            .send()
            .await
            .with_context(|| format!("POST {url} failed"))?
            .error_for_status()
            .with_context(|| format!("server rejected DeltaPayload at {url}"))?;
        resp.json::<SyncResponse>()
            .await
            .with_context(|| format!("failed to deserialize SyncResponse from {url}"))
    }

    /// Push local frontier delta to remote using signed envelope transport.
    pub async fn push_changes(
        &self,
        remote_url: &str,
        view_name: &str,
        local_frontier: &HashSet<Blake3Hash>,
        remote_frontier: &HashSet<Blake3Hash>,
        local_changes: &HashMap<Blake3Hash, Change>,
        identity: &ArcIdentity,
    ) -> std::result::Result<SyncResponse, NetworkError> {
        let missing = compute_missing_changes(local_frontier, remote_frontier, local_changes);
        let payload = DeltaPayload { changes: missing, view_heads: local_frontier.clone() };
        let payload_bytes =
            serde_json::to_vec(&payload).map_err(|e| NetworkError::Serialization(e.to_string()))?;
        let signed = SignedRequest::sign_with_identity(payload_bytes, Utc::now(), identity);

        let url = format!("{remote_url}/sync/{view_name}");
        let resp = self
            .client
            .post(&url)
            .json(&signed)
            .send()
            .await
            .map_err(|e| NetworkError::Transport(e.to_string()))?
            .error_for_status()
            .map_err(|e| NetworkError::Transport(e.to_string()))?;

        resp.json::<SyncResponse>().await.map_err(|e| NetworkError::Transport(e.to_string()))
    }

    /// Pull missing changes from remote with zero-trust verification before CAS writes.
    pub async fn pull_changes<S: CasStorage>(
        &self,
        remote_url: &str,
        view_name: &str,
        local_frontier: &HashSet<Blake3Hash>,
        identity: &ArcIdentity,
        expected_remote_author_public_key: [u8; 32],
        cas: &S,
    ) -> std::result::Result<DeltaPayload, NetworkError> {
        let summary = FrontierSummary { frontier: local_frontier.clone() };
        let payload_bytes =
            serde_json::to_vec(&summary).map_err(|e| NetworkError::Serialization(e.to_string()))?;
        let signed_summary = SignedRequest::sign_with_identity(payload_bytes, Utc::now(), identity);

        let url = format!("{remote_url}/sync/{view_name}");
        let resp = self
            .client
            .post(&url)
            .json(&signed_summary)
            .send()
            .await
            .map_err(|e| NetworkError::Transport(e.to_string()))?
            .error_for_status()
            .map_err(|e| NetworkError::Transport(e.to_string()))?;

        let incoming = resp
            .json::<SignedRequest>()
            .await
            .map_err(|e| NetworkError::Transport(e.to_string()))?;
        process_incoming_signed_delta(
            &incoming,
            cas,
            Utc::now(),
            &expected_remote_author_public_key,
        )
    }

    /// GET a single CAS blob from `{remote_url}/blobs/{hex_hash}`.
    ///
    /// Returns the raw bytes.  The caller should verify `blake3(bytes) == hash`
    /// before storing (the CAS `write_blob` performs this automatically).
    pub async fn fetch_blob(&self, remote_url: &str, hash: &Blake3Hash) -> Result<Vec<u8>> {
        let hex: String = hash.iter().map(|b| format!("{b:02x}")).collect();
        let url = format!("{remote_url}/blobs/{hex}");
        let bytes = self
            .client
            .get(&url)
            .send()
            .await
            .with_context(|| format!("GET {url} failed"))?
            .error_for_status()
            .with_context(|| format!("server returned error for blob {hex}"))?
            .bytes()
            .await
            .with_context(|| format!("failed to read blob bytes from {url}"))?;
        Ok(bytes.to_vec())
    }
}

/// Verify and persist a signed incoming delta payload.
///
/// Verification is always run before any CAS write.
pub fn process_incoming_signed_delta<S: CasStorage>(
    incoming: &SignedRequest,
    cas: &S,
    now: DateTime<Utc>,
    expected_author_public_key: &[u8; 32],
) -> std::result::Result<DeltaPayload, NetworkError> {
    incoming.verify_incoming_for_author(now, expected_author_public_key)?;

    let payload: DeltaPayload = serde_json::from_slice(&incoming.payload)
        .map_err(|e| NetworkError::Serialization(e.to_string()))?;

    verify_payload(&payload).map_err(|e| NetworkError::InvalidPayload(e.to_string()))?;

    for change in &payload.changes {
        let bytes =
            bincode::serialize(change).map_err(|e| NetworkError::Serialization(e.to_string()))?;
        cas.write_object(&change.id, &bytes).map_err(|e| NetworkError::Storage(e.to_string()))?;
    }
    Ok(payload)
}

fn compute_missing_changes(
    local_frontier: &HashSet<Blake3Hash>,
    remote_frontier: &HashSet<Blake3Hash>,
    local_changes: &HashMap<Blake3Hash, Change>,
) -> Vec<Change> {
    let mut missing = Vec::new();
    let mut stack: Vec<Blake3Hash> = local_frontier.iter().copied().collect();
    let mut seen = HashSet::new();

    while let Some(id) = stack.pop() {
        if !seen.insert(id) {
            continue;
        }
        if remote_frontier.contains(&id) {
            continue;
        }
        let Some(change) = local_changes.get(&id) else {
            continue;
        };
        missing.push(change.clone());
        for dep in &change.deps {
            stack.push(*dep);
        }
    }

    missing
}

fn signing_bytes(payload: &[u8], timestamp: &DateTime<Utc>) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(SIGNING_DOMAIN.len() + payload.len() + 64);
    bytes.extend_from_slice(SIGNING_DOMAIN);
    bytes.extend_from_slice(payload);
    bytes.extend_from_slice(timestamp.to_rfc3339().as_bytes());
    bytes
}

impl Default for NetworkClient {
    fn default() -> Self {
        Self::new().unwrap_or_else(|_| Self { client: Client::new() })
    }
}

#[cfg(test)]
mod tests {
    use std::{
        collections::HashSet,
        io::Read,
        sync::atomic::{AtomicUsize, Ordering},
    };

    use super::*;
    use arc_algebra_types::Blake3Hash as CasHash;
    use arc_change::Change;
    use arc_store_cas::cas::{CasBytes, CasError};
    use arc_store_types::author::test_keypair;
    use rand_core::OsRng;

    fn make_change(intent: &str) -> Change {
        let (author, key) = test_keypair();
        Change::new(HashSet::new(), vec![], intent, author, &key)
    }

    /// `NetworkClient::new()` must always succeed on any platform.
    #[test]
    fn test_network_client_new() {
        let result = NetworkClient::new();
        assert!(result.is_ok(), "NetworkClient::new() must succeed: {:?}", result.err());
    }

    /// `verify_payload` accepts a correctly-signed [`DeltaPayload`].
    #[test]
    fn verify_payload_accepts_valid_change() {
        let change = make_change("add widget");
        let payload = DeltaPayload { changes: vec![change], view_heads: HashSet::new() };
        assert!(verify_payload(&payload).is_ok());
    }

    /// `verify_payload` accepts Changes signed by `Author::Server`.
    ///
    /// Server-canonical Changes (Phase 39 Identity Collapsing) must pass
    /// zero-trust ingress just like Human-signed changes.
    #[test]
    fn verify_payload_accepts_server_signed_change() {
        use arc_store_types::author::{Author, PublicKeyBytes};

        let server_key = ed25519_dalek::SigningKey::from_bytes(&[77u8; 32]);
        let server_pubkey: PublicKeyBytes = server_key.verifying_key().to_bytes();
        let server_author =
            Author::Server { canonical_id: "arc-server".to_string(), key: server_pubkey };
        let change =
            Change::new(HashSet::new(), vec![], "server change", server_author, &server_key);

        let payload = DeltaPayload { changes: vec![change], view_heads: HashSet::new() };
        assert!(
            verify_payload(&payload).is_ok(),
            "Author::Server-signed Change must pass verify_payload"
        );
    }

    /// `verify_payload` rejects a payload whose change id has been tampered with.
    ///
    /// This simulates a supply-chain attack: the change content is altered, which
    /// breaks the Ed25519 signature because the id no longer matches the signing payload.
    #[test]
    fn verify_payload_rejects_tampered_id() {
        let mut change = make_change("add widget");
        // Corrupt the content-addressed identity — signature becomes invalid.
        change.id = [0u8; 32];
        let payload = DeltaPayload { changes: vec![change], view_heads: HashSet::new() };
        let err = verify_payload(&payload);
        assert!(err.is_err(), "tampered change id must fail verify_payload");
        assert!(
            err.unwrap_err().to_string().contains("signature verification failed"),
            "error message must mention signature verification"
        );
    }

    #[test]
    fn verify_incoming_rejects_tampered_signature() {
        let key = SigningKey::generate(&mut OsRng);
        let mut signed = SignedRequest::sign(b"{}".to_vec(), Utc::now(), &key);
        signed.signature.0[0] ^= 0x01;
        let result = signed.verify_incoming(Utc::now());
        assert!(matches!(result, Err(NetworkError::Unauthorized)));
    }

    #[test]
    fn verify_incoming_rejects_expired_timestamp() {
        let key = SigningKey::generate(&mut OsRng);
        let old = Utc::now() - Duration::minutes(6);
        let signed = SignedRequest::sign(b"{}".to_vec(), old, &key);
        let result = signed.verify_incoming(Utc::now());
        assert!(matches!(result, Err(NetworkError::Unauthorized)));
    }

    #[derive(Default)]
    struct MockCasStorage {
        writes: AtomicUsize,
    }

    impl CasStorage for MockCasStorage {
        fn write_object(
            &self,
            _hash: &CasHash,
            _bytes: &[u8],
        ) -> std::result::Result<CasHash, CasError> {
            self.writes.fetch_add(1, Ordering::SeqCst);
            Ok(*_hash)
        }

        fn read_object(&self, _hash: &CasHash) -> std::result::Result<CasBytes, CasError> {
            Err(CasError::HashMismatch)
        }

        fn write_blob_stream(
            &self,
            _reader: &mut dyn Read,
        ) -> std::result::Result<(CasHash, u64), CasError> {
            Err(CasError::HashMismatch)
        }
    }

    #[test]
    fn process_incoming_rejects_unauthorized_before_cas_write() {
        let key = SigningKey::generate(&mut OsRng);
        let payload =
            DeltaPayload { changes: vec![make_change("incoming")], view_heads: HashSet::new() };
        let payload_bytes = serde_json::to_vec(&payload).expect("payload serialization");
        let mut signed = SignedRequest::sign(payload_bytes, Utc::now(), &key);
        signed.signature.0[1] ^= 0x01;

        let cas = MockCasStorage::default();
        let result = process_incoming_signed_delta(
            &signed,
            &cas,
            Utc::now(),
            &key.verifying_key().to_bytes(),
        );
        assert!(matches!(result, Err(NetworkError::Unauthorized)));
        assert_eq!(cas.writes.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn process_incoming_rejects_untrusted_author_key() {
        let key = SigningKey::generate(&mut OsRng);
        let trusted = SigningKey::generate(&mut OsRng).verifying_key().to_bytes();
        let payload =
            DeltaPayload { changes: vec![make_change("incoming")], view_heads: HashSet::new() };
        let payload_bytes = serde_json::to_vec(&payload).expect("payload serialization");
        let signed = SignedRequest::sign(payload_bytes, Utc::now(), &key);

        let cas = MockCasStorage::default();
        let result = process_incoming_signed_delta(&signed, &cas, Utc::now(), &trusted);
        assert!(matches!(result, Err(NetworkError::Unauthorized)));
        assert_eq!(cas.writes.load(Ordering::SeqCst), 0);
    }
}
