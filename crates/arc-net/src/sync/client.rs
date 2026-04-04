use std::collections::HashMap;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use arc_core::algebra::Blake3Hash;
use bytes::BytesMut;
use futures_util::{SinkExt, StreamExt};
use tokio::net::TcpStream;
use tokio::time::timeout;
use tokio_util::codec::Framed;

use super::codec::{ArcSyncCodec, MessageType, SyncFrame};
use super::protocol::{HandshakeRequest, HandshakeResponse};

const HANDSHAKE_RESPONSE_TIMEOUT: Duration = Duration::from_secs(10);

/// Perform a native handshake against a remote arc sync endpoint.
pub async fn sync_remote(
    addr: &str,
    view_heads: HashMap<String, Blake3Hash>,
) -> Result<HandshakeResponse> {
    let socket = TcpStream::connect(addr)
        .await
        .with_context(|| format!("failed to connect to native sync remote at {addr}"))?;
    let mut framed = Framed::new(socket, ArcSyncCodec::new());

    let request = HandshakeRequest {
        version: 1,
        view_heads,
    };
    let payload = BytesMut::from(
        bincode::serialize(&request)
            .context("failed to encode handshake request payload")?
            .as_slice(),
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
    Ok(response)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::path::PathBuf;

    use tokio::net::TcpListener;

    use crate::sync::client::sync_remote;
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
        assert!(response.required_hashes.is_empty());

        server_handle.abort();
    }
}
