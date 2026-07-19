#[cfg(feature = "std")]
use alloc::string::ToString;
use alloc::{format, string::String};
use core::fmt;

use serde::{Deserialize, Serialize};

use crate::Blake3Hash;

/// Strongly-typed Change identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ChangeId(pub Blake3Hash);

/// Strongly-typed Blob identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct BlobId(pub Blake3Hash);

/// Strongly-typed synthesis snapshot identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct SnapshotId(pub Blake3Hash);

/// Strongly-typed history mutation transaction identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct MutationId(pub Blake3Hash);

/// Hex parsing errors for strongly-typed identifiers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseHexError {
    /// The string length was not exactly 64 hex characters.
    InvalidLength {
        /// Actual input length.
        got: usize,
    },
    /// An invalid non-hex character was found.
    InvalidCharacter {
        /// The invalid character.
        ch: char,
    },
}

impl fmt::Display for ParseHexError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidLength { got } => write!(f, "expected 64 hex chars, got {got}"),
            Self::InvalidCharacter { ch } => {
                write!(f, "invalid hex character '{ch}': expected [0-9a-fA-F]")
            }
        }
    }
}

#[cfg(feature = "std")]
impl std::error::Error for ParseHexError {}

#[cfg(feature = "std")]
type ParseResult<T> = anyhow::Result<T>;

#[cfg(not(feature = "std"))]
type ParseResult<T> = Result<T, ParseHexError>;

fn lift_parse_result<T>(result: Result<T, ParseHexError>) -> ParseResult<T> {
    #[cfg(feature = "std")]
    {
        result.map_err(|err| anyhow::anyhow!(err.to_string()))
    }

    #[cfg(not(feature = "std"))]
    {
        result
    }
}

impl ChangeId {
    /// Return lowercase hex representation.
    pub fn to_hex(self) -> String {
        hash_to_hex(&self.0)
    }

    /// Parse a 64-char lowercase/uppercase hex string.
    pub fn from_hex(input: &str) -> ParseResult<Self> {
        lift_parse_result(decode_hex_32(input).map(Self))
    }
}

impl BlobId {
    /// Return lowercase hex representation.
    pub fn to_hex(self) -> String {
        hash_to_hex(&self.0)
    }

    /// Parse a 64-char lowercase/uppercase hex string.
    pub fn from_hex(input: &str) -> ParseResult<Self> {
        lift_parse_result(decode_hex_32(input).map(Self))
    }
}

impl SnapshotId {
    /// Return lowercase hex representation.
    pub fn to_hex(self) -> String {
        hash_to_hex(&self.0)
    }

    /// Parse a 64-char lowercase/uppercase hex string.
    pub fn from_hex(input: &str) -> ParseResult<Self> {
        lift_parse_result(decode_hex_32(input).map(Self))
    }
}

impl MutationId {
    /// Return lowercase hex representation.
    pub fn to_hex(self) -> String {
        hash_to_hex(&self.0)
    }

    /// Parse a 64-char lowercase/uppercase hex string.
    pub fn from_hex(input: &str) -> ParseResult<Self> {
        lift_parse_result(decode_hex_32(input).map(Self))
    }
}

impl fmt::Display for SnapshotId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.to_hex())
    }
}

impl fmt::Display for ChangeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.to_hex())
    }
}

impl fmt::Display for BlobId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.to_hex())
    }
}

impl fmt::Display for MutationId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.to_hex())
    }
}

impl From<Blake3Hash> for ChangeId {
    fn from(value: Blake3Hash) -> Self {
        Self(value)
    }
}

impl From<ChangeId> for Blake3Hash {
    fn from(value: ChangeId) -> Self {
        value.0
    }
}

impl From<Blake3Hash> for BlobId {
    fn from(value: Blake3Hash) -> Self {
        Self(value)
    }
}

impl From<BlobId> for Blake3Hash {
    fn from(value: BlobId) -> Self {
        value.0
    }
}

impl From<Blake3Hash> for SnapshotId {
    fn from(value: Blake3Hash) -> Self {
        Self(value)
    }
}

impl From<Blake3Hash> for MutationId {
    fn from(value: Blake3Hash) -> Self {
        Self(value)
    }
}

impl From<MutationId> for Blake3Hash {
    fn from(value: MutationId) -> Self {
        value.0
    }
}

impl From<SnapshotId> for Blake3Hash {
    fn from(value: SnapshotId) -> Self {
        value.0
    }
}

fn hash_to_hex(hash: &Blake3Hash) -> String {
    hash.iter().map(|b| format!("{b:02x}")).collect()
}

fn decode_hex_32(input: &str) -> Result<Blake3Hash, ParseHexError> {
    if input.len() != 64 {
        return Err(ParseHexError::InvalidLength { got: input.len() });
    }
    let mut out = [0u8; 32];
    let bytes = input.as_bytes();
    for i in 0..32 {
        let hi = nybble(bytes[i * 2])?;
        let lo = nybble(bytes[i * 2 + 1])?;
        out[i] = (hi << 4) | lo;
    }
    Ok(out)
}

fn nybble(c: u8) -> Result<u8, ParseHexError> {
    match c {
        b'0'..=b'9' => Ok(c - b'0'),
        b'a'..=b'f' => Ok(c - b'a' + 10),
        b'A'..=b'F' => Ok(c - b'A' + 10),
        _ => Err(ParseHexError::InvalidCharacter { ch: c as char }),
    }
}

#[cfg(test)]
mod tests {
    use alloc::string::ToString;

    use super::SnapshotId;

    #[test]
    fn snapshot_id_parses_valid_hex() {
        let id = SnapshotId::from_hex(
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
        )
        .expect("valid hex must parse");
        assert_eq!(id.to_hex(), "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef");
    }

    #[test]
    fn snapshot_id_rejects_invalid_hex() {
        let err = SnapshotId::from_hex("not-hex").expect_err("invalid input must fail");
        assert!(err.to_string().contains("expected 64 hex chars"));
    }

    #[test]
    fn change_id_roundtrip_hex() {
        let id = super::ChangeId::from_hex(
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        )
        .expect("valid hex must parse");
        assert_eq!(id.to_hex(), "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");
    }

    #[test]
    fn mutation_id_roundtrip_hex() {
        let id = super::MutationId::from_hex(
            "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
        )
        .expect("valid hex must parse");
        assert_eq!(id.to_hex(), "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb");
    }
}
