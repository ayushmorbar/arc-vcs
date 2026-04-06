#![no_main]

use arc_net::sync::codec::{ArcSyncCodec, MessageType, SyncFrame};
use arc_net::sync::protocol::{HandshakeRequest, SERVER_CAPABILITIES, negotiate_capabilities};
use bytes::{Bytes, BytesMut};
use libfuzzer_sys::fuzz_target;
use tokio_util::codec::{Decoder, Encoder};

fuzz_target!(|data: &[u8]| {
    let mut codec = ArcSyncCodec::new();

    // Exercise frame decode on arbitrary byte streams.
    let mut src = BytesMut::from(data);
    let _ = codec.decode(&mut src);

    // Exercise negotiated handshake parsing paths.
    if let Ok(request) = bincode::deserialize::<HandshakeRequest>(data) {
        let _ = negotiate_capabilities(&request, SERVER_CAPABILITIES);
    }

    // Exercise encode/decode roundtrip for bounded synthetic payloads.
    let mut dst = BytesMut::new();
    let msg_ty = match data.first().copied().unwrap_or(0) % 5 {
        0 => MessageType::Handshake,
        1 => MessageType::HaveWant,
        2 => MessageType::PayloadStream,
        3 => MessageType::KeepAlive,
        _ => MessageType::Progress,
    };
    let payload = Bytes::copy_from_slice(&data[..data.len().min(1024)]);
    let frame = SyncFrame::new(msg_ty, payload);
    if codec.encode(frame, &mut dst).is_ok() {
        let _ = codec.decode(&mut dst);
    }
});
