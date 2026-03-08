//! Async CRDT network transport — Phase 38: Coordination-Free Network Transport.
//!
//! Provides [`NetworkClient`] and the [`DeltaPayload`] wire type that carry
//! the exact missing slice of a mathematical change graph — Changes plus their
//! referenced CAS blobs — between arc peers in a single JSON envelope.
//!
//! # Protocol summary
//!
//! * **Push** — The caller computes the DAG delta (Changes the remote is
//!   missing), bundles all referenced blobs into a [`DeltaPayload`], and
//!   POSTs it to `POST {remote}/sync/{view_name}`.  The server calls
//!   [`verify_payload`] *before* any CAS write (zero-trust ingress), then
//!   advances its view with `new_heads = remote ∪ payload.view_heads`.
//!   This eliminates Git's multi-round-trip `Have/Want` negotiation entirely.
//!
//! * **Fetch blob** — `GET {remote}/blobs/{hex}` returns raw bytes for a
//!   single CAS blob.  Used by `fetch_http` in `arc-cli` to hydrate the
//!   blob sidecar for each fetched Change without a second round-trip.
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
use reqwest::Client;
use serde::{Deserialize, Serialize};

use crate::algebra::Blake3Hash;
use crate::store::change::Change;

// ── Wire type ──────────────────────────────────────────────────────────────

/// The wire type for a coordination-free push/sync operation.
///
/// A `DeltaPayload` carries exactly the subset of the local DAG that the
/// remote is missing, together with every CAS blob those Changes reference,
/// in a single JSON envelope.  The receiver can apply it with no back-and-
/// forth negotiation because:
///
/// 1. **CAS writes are idempotent** — duplicate Changes are silently skipped.
/// 2. **View merge is a set union** — `new_heads = remote ∪ payload.view_heads`.
///
/// # Memory note
///
/// Blobs are held in RAM as raw `Vec<u8>` bytes.  This is acceptable for
/// most source-code repositories.
/// TODO: Phase 39 — implement chunked multipart streaming for large blobs to
/// keep memory usage flat when pushing binary assets above a size threshold.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeltaPayload {
    /// Slice of the DAG the remote is missing (BFS order from local heads).
    pub changes: Vec<Change>,
    /// Raw CAS blob bytes keyed by lowercase hex-encoded BLAKE3 hash.
    ///
    /// Every blob referenced by an `Insert { content_hash }` or
    /// `Delete { prior_hash }` atom in `changes` is included here so the
    /// receiver can materialise the working directory without a second
    /// round-trip.
    pub blobs: HashMap<String, Vec<u8>>,
    /// The sender's current view heads.
    ///
    /// The receiver computes `new_heads = remote_heads ∪ view_heads` — a
    /// pure set union that requires zero coordination.
    pub view_heads: HashSet<Blake3Hash>,
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
    /// The server is expected to:
    /// 1. Call [`verify_payload`] and reject with 400 on failure.
    /// 2. Write all changes and blobs to its CAS (idempotent).
    /// 3. Advance its view: `new_heads = remote_heads ∪ payload.view_heads`.
    pub async fn push_payload(
        &self,
        remote_url: &str,
        view_name: &str,
        payload: &DeltaPayload,
    ) -> Result<()> {
        let url = format!("{remote_url}/sync/{view_name}");
        self.client
            .post(&url)
            .json(payload)
            .send()
            .await
            .with_context(|| format!("POST {url} failed"))?
            .error_for_status()
            .with_context(|| format!("server rejected DeltaPayload at {url}"))?;
        Ok(())
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

impl Default for NetworkClient {
    fn default() -> Self {
        Self::new().unwrap_or_else(|_| Self {
            client: Client::new(),
        })
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;
    use crate::store::author::test_keypair;
    use crate::store::change::Change;

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
        let payload = DeltaPayload {
            changes: vec![change],
            blobs: HashMap::new(),
            view_heads: HashSet::new(),
        };
        assert!(verify_payload(&payload).is_ok());
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
        let payload = DeltaPayload {
            changes: vec![change],
            blobs: HashMap::new(),
            view_heads: HashSet::new(),
        };
        let err = verify_payload(&payload);
        assert!(err.is_err(), "tampered change id must fail verify_payload");
        assert!(
            err.unwrap_err().to_string().contains("signature verification failed"),
            "error message must mention signature verification"
        );
    }
}
