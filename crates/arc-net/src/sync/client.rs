use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use arc_change::Change;
use arc_store_cas::ObjectStore;
use arc_store_types::newtypes::ChangeId;
use bytes::Bytes;
use futures_util::{SinkExt, StreamExt};
use tokio::net::TcpStream;
use tokio::time::timeout;
use tokio_util::codec::Framed;
use tracing::instrument;

use super::codec::{ArcSyncCodec, MessageType, SyncFrame};
use super::protocol::{
    HandshakeRequest, HandshakeResponse, SyncCapability, negotiate_capabilities,
};

const HANDSHAKE_RESPONSE_TIMEOUT: Duration = Duration::from_secs(10);

/// Perform a native handshake against a remote arc sync endpoint.
#[instrument(skip_all)]
pub async fn sync_remote(
    addr: &str,
    view_heads: HashMap<String, ChangeId>,
) -> Result<HandshakeResponse> {
    let repo_path = std::env::current_dir().context("failed to resolve current directory")?;
    sync_remote_from_repo(addr, view_heads, &repo_path).await
}

#[instrument(skip_all)]
pub(crate) async fn sync_remote_from_repo(
    addr: &str,
    view_heads: HashMap<String, ChangeId>,
    repo_path: &Path,
) -> Result<HandshakeResponse> {
    let socket = TcpStream::connect(addr)
        .await
        .with_context(|| format!("failed to connect to native sync remote at {addr}"))?;
    let mut framed = Framed::new(socket, ArcSyncCodec::new());

    let request = HandshakeRequest {
        version: 1,
        min_version: 1,
        auth_token: std::env::var("ARC_SYNC_TOKEN").ok(),
        view_heads,
        required_capabilities: vec![
            SyncCapability::PayloadStreamV1,
            SyncCapability::TypedChangeId,
        ],
        optional_capabilities: vec![SyncCapability::KeepAlive, SyncCapability::ProgressSideband],
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
        bail!(
            "native sync handshake failed with status {}",
            response.status
        );
    }

    if response.negotiated_version < request.min_version
        || response.negotiated_version > request.version
    {
        bail!(
            "server negotiated unsupported protocol version {}",
            response.negotiated_version
        );
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
            bail!(
                "local CAS object failed signature verification for {}",
                id.to_hex()
            );
        }

        framed
            .send(SyncFrame::new(
                MessageType::PayloadStream,
                Bytes::copy_from_slice(raw.as_ref()),
            ))
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
    use std::collections::{HashMap, HashSet};
    use std::path::PathBuf;
    use std::time::Duration;

    use arc_algebra_types::Atom;
    use arc_change::Change;
    use arc_store_cas::ObjectStore;
    use arc_store_types::author::test_keypair;
    use arc_store_types::newtypes::ChangeId;
    use arc_store_view::View;
    use tokio::net::TcpListener;
    use tokio::time::sleep;

    use crate::sync::client::sync_remote;
    use crate::sync::client::sync_remote_from_repo;
    use crate::sync::server;

    #[tokio::test]
    async fn sync_remote_roundtrips_handshake_with_server() {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("test listener bind should succeed");
        let addr = listener
            .local_addr()
            .expect("listener should expose local address");

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
            .write_object(&change.id, &change_bytes)
            .expect("client CAS write should succeed");
        View::new("main", HashSet::from([change.id]))
            .save(client_root.path())
            .expect("client view save should succeed");

        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("test listener bind should succeed");
        let addr = listener
            .local_addr()
            .expect("listener should expose local address");

        let server_repo = server_root.path().to_path_buf();
        let server_handle = tokio::spawn(async move {
            let _ = server::serve_with_listener(listener, server_repo).await;
        });

        let mut view_heads = HashMap::new();
        view_heads.insert("main".to_string(), ChangeId::from(change.id));

        let response = sync_remote_from_repo(&addr.to_string(), view_heads, client_root.path())
            .await
            .expect("native sync should succeed");
        assert_eq!(response.status, 0);
        assert_eq!(response.required_changes, vec![ChangeId::from(change.id)]);

        let server_store = ObjectStore::new(server_root.path());
        let mut loaded = None;
        for _ in 0..50 {
            if let Ok(change_bytes) = server_store.read_object(&change.id)
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
