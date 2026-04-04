/// Encode a payload as a Git pkt-line frame.
///
/// The first four bytes are lowercase hex length including the 4-byte prefix.
pub fn pkt_line(data: &[u8]) -> Vec<u8> {
    let len = data.len() + 4;
    assert!(len <= 0xFFFF, "pkt-line payload too large: {len}");

    let mut out = format!("{len:04x}").into_bytes();
    out.extend_from_slice(data);
    out
}

/// Return the Git pkt-line flush marker.
pub fn pkt_flush() -> &'static [u8] {
    b"0000"
}

#[cfg(test)]
mod tests {
    use super::{pkt_flush, pkt_line};

    #[test]
    fn pkt_line_matches_git_framing_examples() {
        assert_eq!(pkt_line(b""), b"0004");
        assert_eq!(pkt_line(b"a\n"), b"0006a\n");
        assert_eq!(pkt_flush(), b"0000");
    }
}
