//! Zero-copy packet-line framing primitives for transport-layer streams.
//!
//! This module adapts the high-throughput packet-line strategy used in modern
//! VCS transports while staying domain-neutral for `arc`: it only frames bytes
//! and control packets. Semantic decoding (Change, View, CRDT state) remains in
//! higher layers.
//!
//! I/O boundary: pure byte framing only. No network sockets, no filesystem, no
//! signatures, and no CAS writes occur in this module.

use std::io;

use bytes::BytesMut;
use tracing::instrument;

/// Packet-line header size in bytes (`len` encoded as 4 ASCII hex digits).
pub const PKT_LINE_HEADER_LEN: usize = 4;

/// Maximum payload bytes for one packet-line data frame.
///
/// Git-style pkt-line encodes total length in 16 bits, so max data is
/// `0xFFFF - 4`.
pub const MAX_PKT_LINE_DATA_LEN: usize = 65_531;

/// Control packet markers encoded as 4-byte hex lengths.
const FLUSH_MARKER: &[u8; 4] = b"0000";
const DELIMITER_MARKER: &[u8; 4] = b"0001";
const RESPONSE_END_MARKER: &[u8; 4] = b"0002";

/// Borrowed view of one packet-line frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PacketLineRef<'a> {
    /// `0000` packet (flush section boundary).
    Flush,
    /// `0001` packet (delimiter between protocol sections).
    Delimiter,
    /// `0002` packet (logical response termination marker).
    ResponseEnd,
    /// Data packet carrying a borrowed payload slice.
    Data(&'a [u8]),
}

/// Parse exactly one packet-line from `input` without copying payload bytes.
///
/// Returns `Ok(None)` if `input` does not yet contain a full frame.
#[instrument(skip_all)]
pub fn decode_packet_line(input: &[u8]) -> io::Result<Option<(PacketLineRef<'_>, usize)>> {
    if input.len() < PKT_LINE_HEADER_LEN {
        return Ok(None);
    }

    let raw_len = parse_hex_len(&input[..PKT_LINE_HEADER_LEN])?;
    match raw_len {
        0 => Ok(Some((PacketLineRef::Flush, PKT_LINE_HEADER_LEN))),
        1 => Ok(Some((PacketLineRef::Delimiter, PKT_LINE_HEADER_LEN))),
        2 => Ok(Some((PacketLineRef::ResponseEnd, PKT_LINE_HEADER_LEN))),
        len if len < PKT_LINE_HEADER_LEN => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("invalid pkt-line length {len}: must be 0/1/2 or >= 4"),
        )),
        len => {
            if len > PKT_LINE_HEADER_LEN + MAX_PKT_LINE_DATA_LEN {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!(
                        "pkt-line payload length {} exceeds max {}",
                        len - PKT_LINE_HEADER_LEN,
                        MAX_PKT_LINE_DATA_LEN
                    ),
                ));
            }
            if input.len() < len {
                return Ok(None);
            }
            Ok(Some((
                PacketLineRef::Data(&input[PKT_LINE_HEADER_LEN..len]),
                len,
            )))
        }
    }
}

/// Encode one control packet into `dst`.
#[instrument(skip_all)]
pub fn encode_control_packet(kind: PacketLineRef<'_>, dst: &mut BytesMut) -> io::Result<()> {
    match kind {
        PacketLineRef::Flush => dst.extend_from_slice(FLUSH_MARKER),
        PacketLineRef::Delimiter => dst.extend_from_slice(DELIMITER_MARKER),
        PacketLineRef::ResponseEnd => dst.extend_from_slice(RESPONSE_END_MARKER),
        PacketLineRef::Data(_) => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "encode_control_packet() received data packet",
            ));
        }
    }
    Ok(())
}

/// Encode one borrowed payload as pkt-line data into `dst`.
#[instrument(skip_all)]
pub fn encode_data_packet(payload: &[u8], dst: &mut BytesMut) -> io::Result<()> {
    if payload.len() > MAX_PKT_LINE_DATA_LEN {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "pkt-line payload {} exceeds max {}",
                payload.len(),
                MAX_PKT_LINE_DATA_LEN
            ),
        ));
    }

    let frame_len = payload.len() + PKT_LINE_HEADER_LEN;
    let mut header = [0u8; PKT_LINE_HEADER_LEN];
    write_hex_len(frame_len, &mut header)?;

    dst.reserve(frame_len);
    dst.extend_from_slice(&header);
    dst.extend_from_slice(payload);
    Ok(())
}

fn parse_hex_len(raw: &[u8]) -> io::Result<usize> {
    if raw.len() != PKT_LINE_HEADER_LEN {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "pkt-line length header must be exactly 4 bytes",
        ));
    }

    let text = std::str::from_utf8(raw).map_err(|err| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("pkt-line header must be ascii hex: {err}"),
        )
    })?;

    usize::from_str_radix(text, 16).map_err(|err| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("invalid pkt-line hex length '{text}': {err}"),
        )
    })
}

fn write_hex_len(len: usize, out: &mut [u8; PKT_LINE_HEADER_LEN]) -> io::Result<()> {
    if len > 0xFFFF {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "pkt-line frame length exceeds 16-bit maximum",
        ));
    }

    let s = format!("{len:04x}");
    out.copy_from_slice(s.as_bytes());
    Ok(())
}

#[cfg(test)]
mod tests {
    use bytes::BytesMut;

    use super::{
        MAX_PKT_LINE_DATA_LEN, PacketLineRef, decode_packet_line, encode_control_packet,
        encode_data_packet,
    };

    #[test]
    fn data_roundtrip_is_zero_copy_view() {
        let mut buf = BytesMut::new();
        encode_data_packet(b"arc", &mut buf).expect("encode data should succeed");

        let decoded = decode_packet_line(&buf)
            .expect("decode should succeed")
            .expect("full frame expected");
        assert_eq!(decoded.1, buf.len());
        assert_eq!(decoded.0, PacketLineRef::Data(b"arc"));
    }

    #[test]
    fn control_packets_roundtrip() {
        let mut buf = BytesMut::new();
        encode_control_packet(PacketLineRef::Flush, &mut buf).expect("encode flush");
        assert_eq!(buf.as_ref(), b"0000");

        let decoded = decode_packet_line(&buf)
            .expect("decode flush")
            .expect("flush frame expected");
        assert_eq!(decoded.0, PacketLineRef::Flush);
    }

    #[test]
    fn rejects_oversized_data_packet() {
        let payload = vec![0u8; MAX_PKT_LINE_DATA_LEN + 1];
        let mut buf = BytesMut::new();
        let err = encode_data_packet(&payload, &mut buf).expect_err("oversized payload must fail");
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
    }
}
