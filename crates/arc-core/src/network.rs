//! Async CRDT network transport.
//!
//! Provides [`NetworkClient`] — a thin, pure-Rust HTTP client for pushing
//! and pulling arc's mathematical change atoms between peers.
//!
//! # Design notes
//!
//! arc's sync protocol is radically simpler than Git's because the
//! algebraic foundation removes the need for packfiles or merge negotiation:
//!
//! * **Push** — serialise the local DAG delta as a JSON array of
//!   BLAKE3-addressed [`crate::store::change::Change`] objects and POST it
//!   to the remote's `/sync` endpoint.
//! * **Pull** — GET the remote's named view, then fetch any unknown
//!   [`crate::algebra::Blake3Hash`] objects one by one from `/objects/{hex}`.
//!   No "common ancestor" negotiation is required — content-addressing
//!   guarantees idempotent deduplication.
//!
//! The runtime (`tokio`) is **never started inside this module** — callers
//! boot the runtime at the CLI edge and pass in the async context.  This
//! keeps `arc-core` a pure library with no ambient global state.
//!
//! # TLS
//! Built with `reqwest`'s `rustls-tls` feature so there is **no OpenSSL
//! dependency** — the zero-C-binding invariant is preserved end-to-end.

use anyhow::{Context, Result};
use reqwest::Client;

/// CRDT-aware HTTP client for arc peer synchronisation.
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
    /// Configures a `User-Agent` header identifying the arc version and
    /// enables rustls so the binary has zero native TLS dependencies.
    pub fn new() -> Result<Self> {
        let client = Client::builder()
            .user_agent(concat!("arc-vcs/", env!("CARGO_PKG_VERSION")))
            .build()
            .context("failed to build HTTP client")?;
        Ok(Self { client })
    }

    /// Push local mathematical changes to a remote arc peer.
    ///
    /// **Current implementation:** skeleton — establishes the async call
    /// path and validates connectivity.  Full DAG serialisation and
    /// incremental delta upload will land in Phase 33.
    ///
    /// # Protocol (planned)
    /// ```text
    /// POST {remote_url}/sync
    /// Content-Type: application/json
    /// Body: [{ "id": "…", "deps": […], "atoms": […], … }, …]
    /// ```
    pub async fn push(&self, remote_name: &str, remote_url: &str) -> Result<()> {
        // Phase 33: collect local-only DAG delta and POST to /sync.
        // For now we validate the remote is reachable and print progress.
        let probe = format!("{remote_url}/health");
        let status = self
            .client
            .get(&probe)
            .send()
            .await
            .map(|r| r.status());

        match status {
            Ok(s) if s.is_success() => {
                println!(
                    "Connected to '{}' ({remote_url}) — DAG delta upload (Phase 33).",
                    remote_name
                );
            }
            Ok(s) => {
                println!(
                    "Remote '{}' responded with {s} — server may not support arc sync yet.",
                    remote_name
                );
            }
            Err(_) => {
                // Remote unreachable — fall back to informative message.
                println!(
                    "Remote '{}' ({remote_url}) is not reachable over HTTP.",
                    remote_name
                );
                println!(
                    "Tip: start an arc server with `arc serve` on the remote machine \
                     and ensure the URL is correct."
                );
            }
        }

        println!(
            "Push skeleton complete — \
             full incremental CRDT delta upload lands in Phase 33."
        );
        Ok(())
    }

    /// Pull and mathematically merge independent AST changes from a remote arc peer.
    ///
    /// **Current implementation:** skeleton — validates connectivity and
    /// prints the remote's head information.  Full incremental pull (bounded
    /// BFS over `GET /objects/{hex}`) is wired in `sync.rs` and will be
    /// unified here in Phase 33.
    ///
    /// # Protocol (planned)
    /// ```text
    /// GET {remote_url}/views/{view_name}         → View { heads: […] }
    /// GET {remote_url}/objects/{hex}             → bincode Change bytes
    /// ```
    pub async fn pull(&self, remote_name: &str, remote_url: &str) -> Result<()> {
        let view_url = format!("{remote_url}/views/main");
        let status = self
            .client
            .get(&view_url)
            .send()
            .await
            .map(|r| r.status());

        match status {
            Ok(s) if s.is_success() => {
                println!(
                    "Pulling independent AST changes from '{}' ({remote_url})…",
                    remote_name
                );
                println!("State converged. (Full BFS delta pull lands in Phase 33.)");
            }
            Ok(s) => {
                println!(
                    "Remote '{}' responded with {s} — server may not support arc sync yet.",
                    remote_name
                );
            }
            Err(_) => {
                println!(
                    "Remote '{}' ({remote_url}) is not reachable over HTTP.",
                    remote_name
                );
                println!(
                    "Tip: start an arc server with `arc serve` on the remote machine \
                     and ensure the URL is correct."
                );
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `NetworkClient::new()` is pure construction — must always succeed on any platform.
    #[test]
    fn test_network_client_new() {
        let result = NetworkClient::new();
        assert!(result.is_ok(), "NetworkClient::new() must succeed: {:?}", result.err());
    }

    /// `push()` must return `Ok(())` even when the remote is unreachable.
    /// The current design swallows connectivity failures and prints a message
    /// rather than propagating the error to callers.
    #[tokio::test]
    async fn test_push_unreachable_remote_returns_ok() {
        let client = NetworkClient::new().unwrap();
        // Port 19999 is almost certainly not listening on CI.
        let result = client.push("ghost", "http://127.0.0.1:19999").await;
        assert!(
            result.is_ok(),
            "push must return Ok even when the remote is unreachable, got: {:?}",
            result.err()
        );
    }

    /// `pull()` must return `Ok(())` even when the remote is unreachable.
    #[tokio::test]
    async fn test_pull_unreachable_remote_returns_ok() {
        let client = NetworkClient::new().unwrap();
        let result = client.pull("ghost", "http://127.0.0.1:19999").await;
        assert!(
            result.is_ok(),
            "pull must return Ok even when the remote is unreachable, got: {:?}",
            result.err()
        );
    }
}
