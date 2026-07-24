//! Git object hashing primitives.

use std::{fmt, str::FromStr};

use sha1::{Digest, Sha1};

/// Kind tag for synthesized Git objects.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GitObjectKind {
    /// Raw file content object.
    Blob,
    /// Directory listing object.
    Tree,
    /// Commit metadata object.
    Commit,
    /// Annotated tag object.
    Tag,
}

impl GitObjectKind {
    /// Return Git's canonical object-kind bytes.
    #[inline]
    pub const fn as_bytes(self) -> &'static [u8] {
        match self {
            Self::Blob => b"blob",
            Self::Tree => b"tree",
            Self::Commit => b"commit",
            Self::Tag => b"tag",
        }
    }
}

/// A 20-byte SHA-1 object id used for Git object addressing.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct GitOid(pub [u8; 20]);

impl GitOid {
    /// Construct an object id from raw SHA-1 bytes.
    pub const fn from_bytes(bytes: [u8; 20]) -> Self {
        Self(bytes)
    }

    /// Access the raw 20-byte SHA-1 digest.
    pub const fn as_bytes(self) -> [u8; 20] {
        self.0
    }

    /// Parse a lowercase or uppercase 40-char hex object id.
    pub fn from_hex(hex: &str) -> Result<Self, &'static str> {
        if hex.len() != 40 {
            return Err("git oid hex must be exactly 40 characters");
        }

        let mut out = [0u8; 20];
        let bytes = hex.as_bytes();
        for i in 0..20 {
            let hi = decode_hex_nibble(bytes[i * 2]).ok_or("invalid hex digit in git oid")?;
            let lo = decode_hex_nibble(bytes[i * 2 + 1]).ok_or("invalid hex digit in git oid")?;
            out[i] = (hi << 4) | lo;
        }
        Ok(Self(out))
    }
}

fn decode_hex_nibble(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(10 + (b - b'a')),
        b'A'..=b'F' => Some(10 + (b - b'A')),
        _ => None,
    }
}

impl fmt::Debug for GitOid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self)
    }
}

impl fmt::Display for GitOid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        let mut out = [0u8; 40];
        for (i, b) in self.0.iter().enumerate() {
            out[i * 2] = HEX[(b >> 4) as usize];
            out[i * 2 + 1] = HEX[(b & 0x0F) as usize];
        }

        // Git OIDs are always lowercase ASCII hex.
        let s = std::str::from_utf8(&out).map_err(|_| fmt::Error)?;
        f.write_str(s)
    }
}

impl FromStr for GitOid {
    type Err = &'static str;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::from_hex(s)
    }
}

/// Hash content using Git's object framing:
/// `<kind> <len>\0<raw-content>`.
pub fn git_hash(kind: GitObjectKind, content: &[u8]) -> GitOid {
    let mut hasher = Sha1::new();
    hasher.update(kind.as_bytes());
    hasher.update(b" ");

    // Length is encoded in ASCII decimal in the object header.
    let len_ascii = content.len().to_string();
    hasher.update(len_ascii.as_bytes());
    hasher.update(b"\0");
    hasher.update(content);

    let digest = hasher.finalize();
    let mut oid = [0u8; 20];
    oid.copy_from_slice(&digest);
    GitOid::from_bytes(oid)
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use super::{GitObjectKind, GitOid, git_hash};

    #[test]
    fn empty_blob_matches_git_oracle() {
        let oid = git_hash(GitObjectKind::Blob, b"");
        assert_eq!(oid.to_string(), "e69de29bb2d1d6434b8b29ae775ad8c2e48c5391");
    }

    #[test]
    fn hello_world_blob_matches_git_oracle() {
        let oid = git_hash(GitObjectKind::Blob, b"Hello, World!\n");
        assert_eq!(oid.to_string(), "8ab686eafeb1f44702738c8b0f24f2567c36da6d");
    }

    #[test]
    fn git_object_kind_as_bytes_all_variants() {
        assert_eq!(GitObjectKind::Blob.as_bytes(), b"blob");
        assert_eq!(GitObjectKind::Tree.as_bytes(), b"tree");
        assert_eq!(GitObjectKind::Commit.as_bytes(), b"commit");
        assert_eq!(GitObjectKind::Tag.as_bytes(), b"tag");
    }

    #[test]
    fn git_oid_from_bytes_and_as_bytes_roundtrip() {
        let bytes: [u8; 20] = [
            0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0A, 0x0B, 0x0C, 0x0D, 0x0E,
            0x0F, 0x10, 0x11, 0x12, 0x13, 0x14,
        ];
        let oid = GitOid::from_bytes(bytes);
        assert_eq!(oid.as_bytes(), bytes);
    }

    #[test]
    fn git_oid_from_hex_valid_lowercase() {
        let hex = "abcdef0123456789abcdef0123456789abcdef01";
        let oid = GitOid::from_hex(hex).unwrap();
        assert_eq!(oid.to_string(), hex);
    }

    #[test]
    fn git_oid_from_hex_valid_uppercase() {
        let hex_upper = "ABCDEF0123456789ABCDEF0123456789ABCDEF01";
        let hex_lower = "abcdef0123456789abcdef0123456789abcdef01";
        let oid = GitOid::from_hex(hex_upper).unwrap();
        assert_eq!(oid.to_string(), hex_lower);
    }

    #[test]
    fn git_oid_from_hex_wrong_length_too_short() {
        assert!(GitOid::from_hex("abc").is_err());
    }

    #[test]
    fn git_oid_from_hex_wrong_length_too_long() {
        assert!(GitOid::from_hex("0".repeat(41).as_str()).is_err());
    }

    #[test]
    fn git_oid_from_hex_invalid_digit() {
        assert!(GitOid::from_hex("000000000000000000000000000000000000000g").is_err());
    }

    #[test]
    fn git_oid_from_str_delegates_to_from_hex() {
        let hex = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let oid = GitOid::from_str(hex).unwrap();
        assert_eq!(oid.to_string(), hex);
    }

    #[test]
    fn git_oid_debug_matches_display() {
        let oid = git_hash(GitObjectKind::Blob, b"test");
        let display = oid.to_string();
        let debug = format!("{:?}", oid);
        assert_eq!(display, debug);
    }

    #[test]
    fn git_oid_equality() {
        let oid1 = git_hash(GitObjectKind::Blob, b"same");
        let oid2 = git_hash(GitObjectKind::Blob, b"same");
        assert_eq!(oid1, oid2);
    }

    #[test]
    fn git_oid_inequality_different_content() {
        let oid1 = git_hash(GitObjectKind::Blob, b"one");
        let oid2 = git_hash(GitObjectKind::Blob, b"two");
        assert_ne!(oid1, oid2);
    }

    #[test]
    fn empty_tree_hash() {
        let oid = git_hash(GitObjectKind::Tree, b"");
        assert!(!oid.to_string().is_empty());
        assert_eq!(oid.to_string().len(), 40);
    }

    #[test]
    fn git_hash_deterministic() {
        let oid1 = git_hash(GitObjectKind::Commit, b"content");
        let oid2 = git_hash(GitObjectKind::Commit, b"content");
        assert_eq!(oid1, oid2);
    }
}
