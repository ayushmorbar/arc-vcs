use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use bytes::BytesMut;
use futures_util::{SinkExt, StreamExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Semaphore;
use tokio::time::timeout;
use tokio_util::codec::Framed;

use super::codec::{ArcSyncCodec, MessageType, SyncFrame};
use super::protocol::{HandshakeRequest, HandshakeResponse};

const MAX_CONCURRENT_CONNECTIONS: usize = 256;
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

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
        let (socket, _) = listener.accept().await.context("accept failed")?;
        let permit = limiter
            .clone()
            .acquire_owned()
            .await
            .context("connection limiter closed")?;
        let task_repo_path = repo_path.clone();
        tokio::spawn(async move {
            let _permit = permit;
            if let Err(err) = handle_connection(socket, task_repo_path).await {
                let _ = err;
            }
        });
    }
}

async fn handle_connection(socket: TcpStream, _repo_path: PathBuf) -> Result<()> {
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

    let response = HandshakeResponse {
        status: if request.version == 1 { 0 } else { 1 },
        required_hashes: Vec::new(),
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

    Ok(())
}
