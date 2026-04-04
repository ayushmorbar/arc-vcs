use std::io;

use bytes::{Buf, BufMut, BytesMut};
use tokio_util::codec::{Decoder, Encoder};

/// Maximum accepted payload size for one sync frame (16 MiB).
pub const MAX_FRAME_LEN: usize = 16 * 1024 * 1024;

/// One-byte message family for framed sync messages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum MessageType {
    /// Initial handshake exchange.
    Handshake = 0x01,
    /// Have/want negotiation payload.
    HaveWant = 0x02,
    /// Serialized object payload stream.
    PayloadStream = 0x03,
}

impl TryFrom<u8> for MessageType {
    type Error = io::Error;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0x01 => Ok(Self::Handshake),
            0x02 => Ok(Self::HaveWant),
            0x03 => Ok(Self::PayloadStream),
            _ => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("unknown message type: 0x{value:02x}"),
            )),
        }
    }
}

/// Decoded sync frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncFrame {
    /// Logical frame type.
    pub message_type: MessageType,
    /// Opaque binary payload.
    pub payload: BytesMut,
}

impl SyncFrame {
    /// Construct a frame from type and payload.
    pub fn new(message_type: MessageType, payload: BytesMut) -> Self {
        Self {
            message_type,
            payload,
        }
    }
}

/// Length-prefixed codec for native arc sync streams.
#[derive(Debug, Default, Clone, Copy)]
pub struct ArcSyncCodec;

impl Decoder for ArcSyncCodec {
    type Item = SyncFrame;
    type Error = io::Error;

    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        // Header: 1 byte type + 4 bytes payload length.
        if src.len() < 5 {
            return Ok(None);
        }

        let message_type = MessageType::try_from(src[0])?;
        let length = u32::from_be_bytes([src[1], src[2], src[3], src[4]]) as usize;
        if length > MAX_FRAME_LEN {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("frame length {length} exceeds max {MAX_FRAME_LEN}"),
            ));
        }

        let frame_len = 5usize.checked_add(length).ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidData, "frame length overflow")
        })?;

        if src.len() < frame_len {
            return Ok(None);
        }

        src.advance(5);
        let payload = src.split_to(length);

        Ok(Some(SyncFrame {
            message_type,
            payload,
        }))
    }
}

impl Encoder<SyncFrame> for ArcSyncCodec {
    type Error = io::Error;

    fn encode(&mut self, item: SyncFrame, dst: &mut BytesMut) -> Result<(), Self::Error> {
        let payload_len = item.payload.len();
        if payload_len > u32::MAX as usize {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "payload exceeds u32 framing limit",
            ));
        }
        if payload_len > MAX_FRAME_LEN {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("payload exceeds max frame length: {payload_len}"),
            ));
        }

        dst.reserve(5 + payload_len);
        dst.put_u8(item.message_type as u8);
        dst.put_u32(payload_len as u32);
        dst.extend_from_slice(&item.payload);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use bytes::BytesMut;
    use futures_util::{SinkExt, StreamExt};
    use tokio_test::io::Builder;
    use tokio_util::codec::{Decoder, Framed};

    use super::{ArcSyncCodec, MAX_FRAME_LEN, MessageType, SyncFrame};

    #[tokio::test]
    async fn framed_codec_roundtrip_with_header_prefix() {
        let payload = BytesMut::from(&b"hello-sync"[..]);

        let mut expected = Vec::new();
        expected.push(MessageType::PayloadStream as u8);
        expected.extend_from_slice(&(payload.len() as u32).to_be_bytes());
        expected.extend_from_slice(&payload);

        let io = Builder::new().write(&expected).read(&expected).build();
        let mut framed = Framed::new(io, ArcSyncCodec);

        framed
            .send(SyncFrame::new(MessageType::PayloadStream, payload.clone()))
            .await
            .expect("encoding through Framed should succeed");

        let decoded = framed
            .next()
            .await
            .expect("stream should yield one frame")
            .expect("decode should succeed");

        assert_eq!(decoded.message_type, MessageType::PayloadStream);
        assert_eq!(decoded.payload, payload);
    }

    #[test]
    fn decoder_rejects_unknown_message_type() {
        let mut codec = ArcSyncCodec;
        let mut src = BytesMut::from(&[0x7f, 0, 0, 0, 0][..]);
        let err = codec
            .decode(&mut src)
            .expect_err("unknown type must error");
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
    }

    #[test]
    fn decoder_rejects_oversized_frame() {
        let mut codec = ArcSyncCodec;
        let oversized = (MAX_FRAME_LEN as u32).saturating_add(1);
        let mut src = BytesMut::from(&[MessageType::PayloadStream as u8][..]);
        src.extend_from_slice(&oversized.to_be_bytes());
        let err = codec
            .decode(&mut src)
            .expect_err("oversized frame must error");
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
    }
}
