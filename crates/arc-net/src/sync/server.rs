use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use arc_algebra_types::Blake3Hash;
use arc_change::Change;
use arc_store_view::View;
use bytes::BytesMut;
use futures_util::{SinkExt, StreamExt};
use std::net::SocketAddr;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Semaphore;
use tokio::time::timeout;
use tokio_util::codec::Framed;

use super::codec::{ArcSyncCodec, MessageType, SyncFrame};
use super::protocol::{HandshakeRequest, HandshakeResponse};

const MAX_CONCURRENT_CONNECTIONS: usize = 256;
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
const PAYLOAD_FRAME_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_PAYLOAD_FRAMES: usize = 4096;
const MAX_TOTAL_PAYLOAD_BYTES: usize = 256 * 1024 * 1024;

/// Start the native TCP sync server.
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
    let required_hashes = if request.version == 1 && authorized {
        compute_required_hashes(&repo_path, &request.view_heads)
    } else {
        Vec::new()
    };
    let response = HandshakeResponse {
        status: if request.version != 1 {
            1
        } else if !authorized {
            2
        } else {
            0
        },
        required_hashes: required_hashes.clone(),
    };
    let payload = BytesMut::from(
        bincode::serialize(&response)
            .context("failed to encode handshake response payload")?
            .as_slice(),
    );

    framed
        .send(SyncFrame::new(MessageType::Handshake, payload))
        .await
        .context("failed to send handshake response")?;

    if response.status == 0 && !required_hashes.is_empty() {
        ingest_payload_stream(&mut framed, &repo_path, &required_hashes).await?;
    }

    if response.status == 0 {
        ensure_dependency_closure_present(&repo_path, request.view_heads.values().copied())?;
        persist_view_heads(&repo_path, &request.view_heads)?;
    }

    Ok(())
}

fn compute_required_hashes(
    repo_path: &std::path::Path,
    requested_heads: &std::collections::HashMap<String, Blake3Hash>,
) -> Vec<Blake3Hash> {
    let mut needed = Vec::new();
    for hash in requested_heads.values() {
        if !change_exists(repo_path, hash) {
            needed.push(*hash);
        }
    }
    needed.sort();
    needed.dedup();
    needed
}

async fn ingest_payload_stream(
    framed: &mut Framed<TcpStream, ArcSyncCodec>,
    repo_path: &std::path::Path,
    required_hashes: &[Blake3Hash],
) -> Result<()> {
    let mut saw_eof = false;
    let mut frame_count: usize = 0;
    let mut total_bytes: usize = 0;
    while let Some(frame_result) = timeout(PAYLOAD_FRAME_TIMEOUT, framed.next())
        .await
        .context("timed out waiting for payload frame")?
    {
        let frame = frame_result.context("failed to decode payload frame")?;
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

        write_change_raw(repo_path, &change.id, &frame.payload)?;
    }

    if !saw_eof {
        bail!("payload stream ended without EOF frame");
    }

    for hash in required_hashes {
        if !change_exists(repo_path, hash) {
            bail!("required change missing after ingest");
        }
    }

    Ok(())
}

fn ensure_dependency_closure_present(
    repo_path: &std::path::Path,
    heads: impl IntoIterator<Item = Blake3Hash>,
) -> Result<()> {
    let mut stack: Vec<Blake3Hash> = heads.into_iter().collect();
    let mut seen = std::collections::HashSet::new();
    while let Some(hash) = stack.pop() {
        if !seen.insert(hash) {
            continue;
        }

        let raw = read_change_raw(repo_path, &hash)?;
        let change: Change = bincode::deserialize(&raw)
            .context("failed to decode change while verifying dependency closure")?;
        if change.id != hash {
            bail!("dependency closure contains mismatched change id");
        }
        if !change.verify_signature() {
            bail!("dependency closure verification failed");
        }
        stack.extend(change.deps.iter().copied());
    }
    Ok(())
}

fn persist_view_heads(
    repo_path: &std::path::Path,
    view_heads: &std::collections::HashMap<String, Blake3Hash>,
) -> Result<()> {
    for (view_name, head) in view_heads {
        if !is_valid_view_name(view_name) {
            bail!("invalid view name in handshake payload");
        }

        let view = match View::load(repo_path, view_name) {
            Ok(existing) => View::new(existing.name, std::collections::HashSet::from([*head])),
            Err(_) => View::new(view_name.clone(), std::collections::HashSet::from([*head])),
        };
        view.save(repo_path)
            .map_err(|e| anyhow::anyhow!("failed to persist view '{view_name}': {e}"))?;
    }
    Ok(())
}

fn write_change_raw(repo_path: &std::path::Path, hash: &Blake3Hash, bytes: &[u8]) -> Result<()> {
    let path = change_object_path(repo_path, hash);
    if path.exists() {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create CAS directory {}", parent.display()))?;
    }
    std::fs::write(&path, bytes)
        .with_context(|| format!("failed to write change object {}", path.display()))?;
    Ok(())
}

fn read_change_raw(repo_path: &std::path::Path, hash: &Blake3Hash) -> Result<Vec<u8>> {
    let path = change_object_path(repo_path, hash);
    std::fs::read(&path).with_context(|| format!("failed to read change object {}", path.display()))
}

fn change_exists(repo_path: &std::path::Path, hash: &Blake3Hash) -> bool {
    change_object_path(repo_path, hash).exists()
}

fn change_object_path(repo_path: &std::path::Path, hash: &Blake3Hash) -> PathBuf {
    let hex = hash_hex(hash);
    repo_path
        .join(".arc")
        .join("store")
        .join(&hex[..2])
        .join(&hex[2..])
}

fn hash_hex(hash: &Blake3Hash) -> String {
    hash.iter().fold(String::with_capacity(64), |mut acc, b| {
        use std::fmt::Write as _;
        let _ = write!(acc, "{b:02x}");
        acc
    })
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

