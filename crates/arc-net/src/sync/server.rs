use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use arc_change::Change;
use arc_store_cas::ObjectStore;
use arc_store_types::newtypes::ChangeId;
use arc_store_view::View;
use bytes::Bytes;
use futures_util::{SinkExt, StreamExt};
use std::net::SocketAddr;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Semaphore;
use tokio::time::timeout;
use tokio_util::codec::Framed;
use tracing::instrument;

use super::codec::{ArcSyncCodec, MessageType, SyncFrame};
use super::protocol::{
    HandshakeRequest, HandshakeResponse, SERVER_CAPABILITIES, SyncCapability,
    negotiate_capabilities,
};

const MAX_CONCURRENT_CONNECTIONS: usize = 256;
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
const PAYLOAD_FRAME_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_PAYLOAD_FRAMES: usize = 4096;
const MAX_TOTAL_PAYLOAD_BYTES: usize = 256 * 1024 * 1024;

/// Start the native TCP sync server.
#[instrument(skip_all)]
pub async fn serve(port: u16, repo_path: PathBuf) -> Result<()> {
    let bind_addr = format!("0.0.0.0:{port}");
    let listener = TcpListener::bind(&bind_addr)
        .await
        .with_context(|| format!("failed to bind native sync server on {bind_addr}"))?;
    serve_with_listener(listener, repo_path).await
}

pub(crate) async fn serve_with_listener(listener: TcpListener, repo_path: PathBuf) -> Result<()> {
    let limiter = Arc::new(Semaphore::new(MAX_CONCURRENT_CONNECTIONS));

    loop {
        let (socket, peer_addr) = listener.accept().await.context("accept failed")?;
        let permit = limiter
            .clone()
            .acquire_owned()
            .await
            .context("connection limiter closed")?;
        let task_repo_path = repo_path.clone();
        tokio::spawn(async move {
            let _permit = permit;
            if let Err(err) = handle_connection(socket, peer_addr, task_repo_path).await {
                let _ = err;
            }
        });
    }
}

async fn handle_connection(
    socket: TcpStream,
    peer_addr: SocketAddr,
    repo_path: PathBuf,
) -> Result<()> {
    let mut framed = Framed::new(socket, ArcSyncCodec::new());

    let first = timeout(HANDSHAKE_TIMEOUT, framed.next())
        .await
        .context("timed out waiting for handshake frame")?
        .ok_or_else(|| anyhow::anyhow!("connection closed before handshake"))??;

    if first.message_type != MessageType::Handshake {
        bail!("expected handshake frame as first message");
    }

    let request: HandshakeRequest = bincode::deserialize(&first.payload)
        .context("failed to decode handshake request payload")?;

    let authorized = is_authorized_peer(peer_addr, request.auth_token.as_deref());
    let (negotiated_capabilities, rejected_required_capabilities) =
        negotiate_capabilities(&request, SERVER_CAPABILITIES);
    let version_supported = request.min_version <= 1 && request.version >= 1;

    let required_changes =
        if version_supported && authorized && rejected_required_capabilities.is_empty() {
            compute_required_hashes(&repo_path, &request.view_heads)
        } else {
            Vec::new()
        };

    let status = if !version_supported {
        1
    } else if !authorized {
        2
    } else if !rejected_required_capabilities.is_empty() {
        3
    } else {
        0
    };

    let response = HandshakeResponse {
        status,
        negotiated_version: 1,
        negotiated_capabilities,
        rejected_required_capabilities,
        required_changes: required_changes.clone(),
    };
    let payload = Bytes::from(
        bincode::serialize(&response).context("failed to encode handshake response payload")?,
    );

    framed
        .send(SyncFrame::new(MessageType::Handshake, payload))
        .await
        .context("failed to send handshake response")?;

    if response.status == 0 && !required_changes.is_empty() {
        let allow_keepalive = response
            .negotiated_capabilities
            .contains(&SyncCapability::KeepAlive);
        ingest_payload_stream(&mut framed, &repo_path, &required_changes, allow_keepalive).await?;
    }

    if response.status == 0 {
        ensure_dependency_closure_present(&repo_path, request.view_heads.values().copied())?;
        persist_view_heads(&repo_path, &request.view_heads)?;
    }

    Ok(())
}

fn compute_required_hashes(
    repo_path: &std::path::Path,
    requested_heads: &std::collections::HashMap<String, ChangeId>,
) -> Vec<ChangeId> {
    let store = ObjectStore::new(repo_path);
    let mut needed = Vec::new();
    for id in requested_heads.values() {
        if store.read_change_bytes(*id).is_err() {
            needed.push(*id);
        }
    }
    needed.sort();
    needed.dedup();
    needed
}

async fn ingest_payload_stream(
    framed: &mut Framed<TcpStream, ArcSyncCodec>,
    repo_path: &std::path::Path,
    required_changes: &[ChangeId],
    allow_keepalive: bool,
) -> Result<()> {
    let store = ObjectStore::new(repo_path);
    let mut saw_eof = false;
    let mut frame_count: usize = 0;
    let mut total_bytes: usize = 0;
    while let Some(frame_result) = timeout(PAYLOAD_FRAME_TIMEOUT, framed.next())
        .await
        .context("timed out waiting for payload frame")?
    {
        let frame = frame_result.context("failed to decode payload frame")?;
        if frame.message_type == MessageType::KeepAlive && allow_keepalive {
            continue;
        }
        if frame.message_type != MessageType::PayloadStream {
            bail!("expected payload-stream frame while ingesting");
        }

        if frame.payload.is_empty() {
            saw_eof = true;
            break;
        }

        frame_count = frame_count.saturating_add(1);
        if frame_count > MAX_PAYLOAD_FRAMES {
            bail!("payload stream exceeded frame limit");
        }
        total_bytes = total_bytes.saturating_add(frame.payload.len());
        if total_bytes > MAX_TOTAL_PAYLOAD_BYTES {
            bail!("payload stream exceeded size limit");
        }

        let change: Change = bincode::deserialize(&frame.payload)
            .context("failed to decode streamed change payload")?;
        if !change.verify_signature() {
            bail!("received change failed cryptographic verification");
        }

        let id = ChangeId::from(change.id);
        store
            .write_change_bytes(id, frame.payload.as_ref())
            .with_context(|| format!("failed to persist streamed change {}", id.to_hex()))?;
    }

    if !saw_eof {
        bail!("payload stream ended without EOF frame");
    }

    for id in required_changes {
        if store.read_change_bytes(*id).is_err() {
            bail!("required change missing after ingest");
        }
    }

    Ok(())
}

fn ensure_dependency_closure_present(
    repo_path: &std::path::Path,
    heads: impl IntoIterator<Item = ChangeId>,
) -> Result<()> {
    let store = ObjectStore::new(repo_path);
    let mut stack: Vec<ChangeId> = heads.into_iter().collect();
    let mut seen = std::collections::HashSet::new();
    while let Some(id) = stack.pop() {
        if !seen.insert(id) {
            continue;
        }

        let raw = store.read_change_bytes(id).with_context(|| {
            format!(
                "failed to read change while verifying dependency closure {}",
                id.to_hex()
            )
        })?;
        let change: Change = bincode::deserialize(&raw)
            .context("failed to decode change while verifying dependency closure")?;
        if change.id != id.0 {
            bail!("dependency closure contains mismatched change id");
        }
        if !change.verify_signature() {
            bail!("dependency closure verification failed");
        }
        stack.extend(change.deps.iter().copied().map(ChangeId::from));
    }
    Ok(())
}

fn persist_view_heads(
    repo_path: &std::path::Path,
    view_heads: &std::collections::HashMap<String, ChangeId>,
) -> Result<()> {
    for (view_name, head) in view_heads {
        if !is_valid_view_name(view_name) {
            bail!("invalid view name in handshake payload");
        }

        let view = match View::load(repo_path, view_name) {
            Ok(existing) => View::new(existing.name, std::collections::HashSet::from([head.0])),
            Err(_) => View::new(view_name.clone(), std::collections::HashSet::from([head.0])),
        };
        view.save(repo_path)
            .map_err(|e| anyhow::anyhow!("failed to persist view '{view_name}': {e}"))?;
    }
    Ok(())
}

fn is_authorized_peer(peer_addr: SocketAddr, presented_token: Option<&str>) -> bool {
    if peer_addr.ip().is_loopback() {
        return true;
    }

    match std::env::var("ARC_SYNC_TOKEN") {
        Ok(expected) => presented_token.is_some_and(|provided| provided == expected),
        Err(_) => false,
    }
}

fn is_valid_view_name(name: &str) -> bool {
    if name.is_empty() || name.starts_with('/') || name.starts_with('\\') || name.contains("..") {
        return false;
    }

    for segment in name.split('/') {
        if segment.is_empty() || segment == "." || segment == ".." {
            return false;
        }
    }

    name.chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-' | '/'))
}
