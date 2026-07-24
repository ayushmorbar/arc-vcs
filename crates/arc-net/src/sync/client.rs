use std::{collections::HashMap, path::Path, time::Duration};

use anyhow::{Context, Result, bail};
use arc_change::Change;
use arc_store_cas::ObjectStore;
use arc_store_types::newtypes::ChangeId;
use bytes::Bytes;
use futures_util::{SinkExt, StreamExt};
use tokio::{
    net::TcpStream,
    time::{sleep, timeout},
};
use tokio_util::codec::Framed;
use tracing::instrument;

use super::{
    backoff::QuadraticBackoff,
    codec::{ArcSyncCodec, MessageType, SyncFrame},
    endpoint::SyncEndpoint,
    protocol::{
        CasWireBlock,
        HandshakeRequest,
        HandshakeResponse,
        NetError,
        SyncCapability,
        SyncProtocol,
        negotiate_capabilities,
    },
};

const HANDSHAKE_RESPONSE_TIMEOUT: Duration = Duration::from_secs(10);
const PAYLOAD_FRAME_TIMEOUT: Duration = Duration::from_secs(10);
const CONNECT_ATTEMPTS: usize = 4;

/// Native sync client used by CLI orchestration.
#[derive(Debug, Clone)]
pub struct NativeSyncClient {
    endpoint: String,
    auth_token: Option<String>,
}

impl NativeSyncClient {
    /// Create a new native sync client targeting `endpoint`.
    pub fn new(endpoint: String, auth_token: Option<String>) -> Self {
        Self { endpoint, auth_token }
    }

    async fn open_framed(&self) -> Result<Framed<TcpStream, ArcSyncCodec>, NetError> {
        let endpoint = SyncEndpoint::parse(&self.endpoint)
            .map_err(|e| NetError::Protocol(format!("invalid native sync endpoint: {e}")))?;
        let socket =
            connect_with_retry(&endpoint).await.map_err(|e| NetError::Protocol(e.to_string()))?;
        Ok(Framed::new(socket, ArcSyncCodec::new()))
    }

    async fn handshake(
        &self,
        framed: &mut Framed<TcpStream, ArcSyncCodec>,
        frontier: Vec<[u8; 32]>,
    ) -> Result<HandshakeResponse, NetError> {
        let request = HandshakeRequest {
            version: 1,
            min_version: 1,
            auth_token: choose_auth_token(self.auth_token.clone()),
            view_heads: HashMap::new(),
            required_capabilities: vec![
                SyncCapability::PayloadStreamV1,
                SyncCapability::TypedChangeId,
            ],
            optional_capabilities: vec![SyncCapability::KeepAlive],
            frontier,
        };

        let payload = Bytes::from(
            bincode::serialize(&request).map_err(|e| NetError::Serialization(e.to_string()))?,
        );
        framed
            .send(SyncFrame::new(MessageType::Handshake, payload))
            .await
            .map_err(NetError::from)?;

        let response_frame = timeout(HANDSHAKE_RESPONSE_TIMEOUT, framed.next())
            .await
            .map_err(|_| {
                NetError::Protocol("timed out waiting for handshake response".to_string())
            })?
            .ok_or_else(|| {
                NetError::Protocol("connection closed before handshake response".to_string())
            })?
            .map_err(NetError::from)?;

        if response_frame.message_type != MessageType::Handshake {
            return Err(NetError::Protocol("expected handshake response frame".to_string()));
        }

        let response: HandshakeResponse = bincode::deserialize(&response_frame.payload)
            .map_err(|e| NetError::Serialization(e.to_string()))?;
        if response.status != 0 {
            return Err(NetError::Protocol(format!(
                "native sync handshake failed with status {}",
                response.status
            )));
        }

        Ok(response)
    }
}

#[async_trait::async_trait]
impl SyncProtocol for NativeSyncClient {
    async fn exchange_frontiers(
        &self,
        local_frontier: Vec<blake3::Hash>,
    ) -> Result<Vec<blake3::Hash>, NetError> {
        let mut framed = self.open_framed().await?;
        let response = self
            .handshake(&mut framed, local_frontier.iter().map(|h| *h.as_bytes()).collect())
            .await?;
        Ok(response.remote_frontier.into_iter().map(blake3::Hash::from).collect())
    }

    async fn fetch_cas_blocks(&self, missing_hashes: &[blake3::Hash]) -> Result<Vec<u8>, NetError> {
        let mut framed = self.open_framed().await?;
        let _ = self.handshake(&mut framed, Vec::new()).await?;

        let request: Vec<[u8; 32]> = missing_hashes.iter().map(|h| *h.as_bytes()).collect();
        let payload = Bytes::from(
            bincode::serialize(&request).map_err(|e| NetError::Serialization(e.to_string()))?,
        );
        framed
            .send(SyncFrame::new(MessageType::HaveWant, payload))
            .await
            .map_err(NetError::from)?;

        let requested: std::collections::HashSet<[u8; 32]> = request.iter().copied().collect();
        let mut remaining = requested.clone();
        let mut blocks = Vec::new();

        while let Some(frame_result) =
            timeout(PAYLOAD_FRAME_TIMEOUT, framed.next()).await.map_err(|_| {
                NetError::Protocol("timed out waiting for CAS payload frame".to_string())
            })?
        {
            let frame = frame_result.map_err(NetError::from)?;
            if frame.message_type == MessageType::KeepAlive {
                continue;
            }
            if frame.message_type != MessageType::PayloadStream {
                return Err(NetError::Protocol(
                    "unexpected frame while reading CAS blocks".to_string(),
                ));
            }
            if frame.payload.is_empty() {
                break;
            }

            let block: CasWireBlock = bincode::deserialize(&frame.payload)
                .map_err(|e| NetError::Serialization(e.to_string()))?;
            if !requested.contains(&block.hash) {
                return Err(NetError::Protocol(
                    "peer returned CAS block not present in request".to_string(),
                ));
            }
            let computed = blake3::hash(&block.bytes);
            if computed.as_bytes() != &block.hash {
                return Err(NetError::HashVerification(ChangeId::from(block.hash).to_hex()));
            }
            remaining.remove(&block.hash);
            blocks.push(block);
        }

        if !remaining.is_empty() {
            let mut missing: Vec<String> =
                remaining.into_iter().map(|hash| ChangeId::from(hash).to_hex()).collect();
            missing.sort();
            return Err(NetError::Protocol(format!(
                "peer omitted {} requested CAS block(s): {}",
                missing.len(),
                missing.join(", ")
            )));
        }

        bincode::serialize(&blocks).map_err(|e| NetError::Serialization(e.to_string()))
    }
}

/// Perform a native handshake against a remote arc sync endpoint.
#[instrument(skip_all)]
pub async fn sync_remote(
    addr: &str,
    view_heads: HashMap<String, ChangeId>,
) -> Result<HandshakeResponse> {
    sync_remote_with_token(addr, view_heads, None).await
}

/// Perform a native handshake with optional explicit auth token.
#[instrument(skip_all)]
pub async fn sync_remote_with_token(
    addr: &str,
    view_heads: HashMap<String, ChangeId>,
    auth_token: Option<String>,
) -> Result<HandshakeResponse> {
    let repo_path = std::env::current_dir().context("failed to resolve current directory")?;
    sync_remote_from_repo(addr, view_heads, &repo_path, auth_token).await
}

#[instrument(skip_all)]
pub(crate) async fn sync_remote_from_repo(
    addr: &str,
    view_heads: HashMap<String, ChangeId>,
    repo_path: &Path,
    auth_token: Option<String>,
) -> Result<HandshakeResponse> {
    let endpoint = SyncEndpoint::parse(addr).context("invalid native sync endpoint")?;
    let socket = connect_with_retry(&endpoint).await?;
    let mut framed = Framed::new(socket, ArcSyncCodec::new());

    let request = HandshakeRequest {
        version: 1,
        min_version: 1,
        auth_token: choose_auth_token(auth_token),
        view_heads,
        required_capabilities: vec![SyncCapability::PayloadStreamV1, SyncCapability::TypedChangeId],
        optional_capabilities: vec![SyncCapability::KeepAlive, SyncCapability::ProgressSideband],
        frontier: Vec::new(),
    };
    let payload = Bytes::from(
        bincode::serialize(&request).context("failed to encode handshake request payload")?,
    );
    framed
        .send(SyncFrame::new(MessageType::Handshake, payload))
        .await
        .context("failed to send handshake request")?;

    let response_frame = timeout(HANDSHAKE_RESPONSE_TIMEOUT, framed.next())
        .await
        .context("timed out waiting for handshake response")?
        .ok_or_else(|| anyhow::anyhow!("connection closed before handshake response"))??;

    if response_frame.message_type != MessageType::Handshake {
        bail!("expected handshake response frame");
    }

    let response: HandshakeResponse = bincode::deserialize(&response_frame.payload)
        .context("failed to decode handshake response payload")?;

    if response.status != 0 {
        bail!("native sync handshake failed with status {}", response.status);
    }

    if response.negotiated_version < request.min_version
        || response.negotiated_version > request.version
    {
        bail!("server negotiated unsupported protocol version {}", response.negotiated_version);
    }

    let (_accepted, rejected) = negotiate_capabilities(&request, &response.negotiated_capabilities);
    if !rejected.is_empty() || !response.rejected_required_capabilities.is_empty() {
        bail!("native sync handshake rejected required capabilities");
    }

    if response.required_changes.is_empty() {
        return Ok(response);
    }

    stream_required_changes(&mut framed, repo_path, &response.required_changes).await?;
    Ok(response)
}

fn choose_auth_token(explicit_token: Option<String>) -> Option<String> {
    choose_auth_token_with_env(explicit_token, std::env::var("ARC_SYNC_TOKEN").ok())
}

fn choose_auth_token_with_env(
    explicit_token: Option<String>,
    env_token: Option<String>,
) -> Option<String> {
    explicit_token.or(env_token)
}

#[instrument(skip_all)]
async fn connect_with_retry(endpoint: &SyncEndpoint) -> Result<TcpStream> {
    let mut waits = QuadraticBackoff::default();
    let mut last_error: Option<anyhow::Error> = None;

    for attempt in 1..=CONNECT_ATTEMPTS {
        match TcpStream::connect(endpoint.socket_addr()).await {
            Ok(socket) => return Ok(socket),
            Err(error) if attempt < CONNECT_ATTEMPTS && is_retryable_connect_error(&error) => {
                last_error = Some(anyhow::anyhow!(
                    "connect attempt {attempt} to {endpoint} failed: {error}"
                ));
                if let Some(wait) = waits.next() {
                    sleep(wait).await;
                }
            }
            Err(error) => {
                return Err(anyhow::anyhow!(
                    "failed to connect to native sync remote at {endpoint}: {error}"
                ));
            }
        }
    }

    Err(last_error.unwrap_or_else(|| {
        anyhow::anyhow!("failed to connect to native sync remote at {endpoint}")
    }))
}

fn is_retryable_connect_error(error: &std::io::Error) -> bool {
    matches!(
        error.kind(),
        std::io::ErrorKind::ConnectionRefused
            | std::io::ErrorKind::ConnectionAborted
            | std::io::ErrorKind::ConnectionReset
            | std::io::ErrorKind::TimedOut
            | std::io::ErrorKind::Interrupted
            | std::io::ErrorKind::WouldBlock
            | std::io::ErrorKind::AddrNotAvailable
            | std::io::ErrorKind::NotConnected
    )
}

#[instrument(skip_all)]
async fn stream_required_changes(
    framed: &mut Framed<TcpStream, ArcSyncCodec>,
    repo_path: &Path,
    required_changes: &[ChangeId],
) -> Result<()> {
    let ids_to_stream = collect_required_closure(repo_path, required_changes)?;
    let store = ObjectStore::new(repo_path);

    for id in &ids_to_stream {
        let raw = store
            .read_change_bytes(*id)
            .with_context(|| format!("failed to read local change {}", id.to_hex()))?;

        // Verify local object integrity before transmission.
        let change: Change = bincode::deserialize(&raw)
            .with_context(|| format!("failed to decode local change {}", id.to_hex()))?;
        if change.id != id.0 {
            bail!("local CAS object id mismatch for {}", id.to_hex());
        }
        if !change.verify_signature() {
            bail!("local CAS object failed signature verification for {}", id.to_hex());
        }

        framed
            .send(SyncFrame::new(MessageType::PayloadStream, Bytes::copy_from_slice(raw.as_ref())))
            .await
            .context("failed to send payload stream frame")?;
    }

    framed
        .send(SyncFrame::new(MessageType::PayloadStream, Bytes::new()))
        .await
        .context("failed to send payload stream EOF frame")?;

    Ok(())
}

#[instrument(skip_all)]
fn collect_required_closure(
    repo_path: &Path,
    required_changes: &[ChangeId],
) -> Result<Vec<ChangeId>> {
    let store = ObjectStore::new(repo_path);
    let mut ordered = Vec::new();
    let mut stack: Vec<ChangeId> = required_changes.to_vec();
    let mut seen = std::collections::HashSet::new();

    while let Some(id) = stack.pop() {
        if !seen.insert(id) {
            continue;
        }

        ordered.push(id);
        let raw = store
            .read_change_bytes(id)
            .with_context(|| format!("failed to read local change {}", id.to_hex()))?;
        let change: Change = bincode::deserialize(&raw)
            .with_context(|| format!("failed to decode local change {}", id.to_hex()))?;
        if change.id != id.0 {
            bail!("local CAS object id mismatch for {}", id.to_hex());
        }
        stack.extend(change.deps.iter().copied().map(ChangeId::from));
    }

    Ok(ordered)
}

#[cfg(test)]
mod tests {
    use std::{
        collections::{HashMap, HashSet},
        path::PathBuf,
        time::Duration,
    };

    use arc_algebra_types::Atom;
    use arc_change::Change;
    use arc_store_cas::ObjectStore;
    use arc_store_types::{author::test_keypair, newtypes::ChangeId};
    use arc_store_view::View;
    use tokio::{net::TcpListener, time::sleep};

    use crate::sync::{
        client::{choose_auth_token_with_env, sync_remote, sync_remote_from_repo},
        endpoint::SyncEndpoint,
        server,
    };

    #[test]
    fn sync_endpoint_parser_rejects_invalid_input() {
        assert!(SyncEndpoint::parse("http://example.com:7777").is_err());
        assert!(SyncEndpoint::parse("tcp://user:secret@localhost:7777").is_err());
    }

    #[tokio::test]
    async fn sync_remote_roundtrips_handshake_with_server() {
        let listener =
            TcpListener::bind("127.0.0.1:0").await.expect("test listener bind should succeed");
        let addr = listener.local_addr().expect("listener should expose local address");

        let server_handle = tokio::spawn(async move {
            let _ = server::serve_with_listener(listener, PathBuf::from(".")).await;
        });

        let response = sync_remote(&addr.to_string(), HashMap::new())
            .await
            .expect("client handshake should succeed");
        assert_eq!(response.status, 0);
        assert!(response.required_changes.is_empty());

        server_handle.abort();
    }

    #[tokio::test]
    async fn sync_transfers_missing_change_into_server_cas() {
        let server_root = tempfile::tempdir().expect("server tempdir should be created");
        let client_root = tempfile::tempdir().expect("client tempdir should be created");

        init_empty_repo(server_root.path()).expect("server repo init should succeed");
        init_empty_repo(client_root.path()).expect("client repo init should succeed");

        let (author, signing_key) = test_keypair();
        let change = Change::new(
            HashSet::new(),
            vec![Atom::Insert {
                at: vec!["file".to_string(), "hello.rs".to_string()],
                content_hash: [7u8; 32],
            }],
            "client change",
            author,
            &signing_key,
        );

        let client_store = ObjectStore::new(client_root.path());
        let change_bytes = bincode::serialize(&change).expect("change serialize should succeed");
        client_store
            .write_change_bytes(arc_store_types::newtypes::ChangeId::from(change.id), &change_bytes)
            .expect("client CAS write should succeed");
        View::new("main", HashSet::from([change.id]))
            .save(client_root.path())
            .expect("client view save should succeed");

        let listener =
            TcpListener::bind("127.0.0.1:0").await.expect("test listener bind should succeed");
        let addr = listener.local_addr().expect("listener should expose local address");

        let server_repo = server_root.path().to_path_buf();
        let server_handle = tokio::spawn(async move {
            let _ = server::serve_with_listener(listener, server_repo).await;
        });

        let mut view_heads = HashMap::new();
        view_heads.insert("main".to_string(), ChangeId::from(change.id));

        let response =
            sync_remote_from_repo(&addr.to_string(), view_heads, client_root.path(), None)
                .await
                .expect("native sync should succeed");
        assert_eq!(response.status, 0);
        assert_eq!(response.required_changes, vec![ChangeId::from(change.id)]);

        let server_store = ObjectStore::new(server_root.path());
        let mut loaded = None;
        for _ in 0..50 {
            if let Ok(change_bytes) =
                server_store.read_change_bytes(arc_store_types::newtypes::ChangeId::from(change.id))
                && let Ok(change_on_server) = bincode::deserialize::<Change>(&change_bytes)
            {
                loaded = Some(change_on_server);
                break;
            }
            sleep(Duration::from_millis(20)).await;
        }
        let loaded = loaded.expect("server CAS should contain transferred change");
        assert_eq!(loaded.id, change.id);

        let server_main = View::load(server_root.path(), "main")
            .expect("server view should be updated after sync");
        assert!(server_main.heads.contains(&change.id));

        server_handle.abort();
    }

    #[tokio::test]
    async fn explicit_auth_token_overrides_environment() {
        assert_eq!(
            choose_auth_token_with_env(
                Some("explicit-token".to_string()),
                Some("env-token".to_string())
            ),
            Some("explicit-token".to_string())
        );
        assert_eq!(
            choose_auth_token_with_env(None, Some("env-token".to_string())),
            Some("env-token".to_string())
        );
        assert_eq!(choose_auth_token_with_env(None, None), None);
    }

    fn init_empty_repo(root: &std::path::Path) -> anyhow::Result<()> {
        std::fs::create_dir_all(root.join(".arc").join("store"))?;
        std::fs::create_dir_all(root.join(".arc").join("views"))?;
        std::fs::write(root.join(".arc").join("HEAD"), "main")?;
        View::new("main", HashSet::new())
            .save(root)
            .map_err(|e| anyhow::anyhow!("failed to save main view: {e}"))?;
        Ok(())
    }
}
