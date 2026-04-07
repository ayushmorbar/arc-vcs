//! Git object hashing primitives.

use std::fmt;

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
            out[i * 2 + 1] = HEX[(b & 0x0f) as usize];
        }

        // Git OIDs are always lowercase ASCII hex.
        let s = std::str::from_utf8(&out).map_err(|_| fmt::Error)?;
        f.write_str(s)
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
    use super::{GitObjectKind, git_hash};

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
}
