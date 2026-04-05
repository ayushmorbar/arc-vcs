use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use arc_algebra_types::Blake3Hash;
use memmap2::Mmap;
use thiserror::Error;

/// Errors returned by the standalone CAS engine.
#[derive(Debug, Error)]
pub enum CasError {
    /// Filesystem I/O failure.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}

/// Files smaller than one OS page (4 KiB) are read into a heap buffer so the
/// kernel does not need to manage a very short-lived page-table entry.
/// Larger blobs are returned as a memory-mapped slice, avoiding a copy
/// into the process heap entirely.
///
/// Callers access the bytes via `Deref<Target = [u8]>`; the distinction is
/// fully transparent to them.
pub enum CasBytes {
    /// Heap-allocated bytes for small blobs (< 4 096 bytes).
    Owned(Vec<u8>),
    /// Memory-mapped file for large blobs (≥ 4 096 bytes).
    Mapped(Mmap),
}

impl std::ops::Deref for CasBytes {
    type Target = [u8];
    #[inline]
    fn deref(&self) -> &[u8] {
        match self {
            CasBytes::Owned(v) => v,
            CasBytes::Mapped(m) => m,
        }
    }
}

impl Default for CasBytes {
    #[inline]
    fn default() -> Self {
        CasBytes::Owned(Vec::new())
    }
}

impl AsRef<[u8]> for CasBytes {
    #[inline]
    fn as_ref(&self) -> &[u8] {
        self
    }
}

/// Content-addressable object store backed by BLAKE3 hashes and `bincode`
/// serialization.
///
/// Objects are persisted at `.arc/store/{hex[0..2]}/{hex[2..]}` ensuring
/// automatic deduplication — identical content always maps to the same path.
pub struct ObjectStore {
    root: PathBuf,
}

impl ObjectStore {
    /// Create a new `ObjectStore` rooted at `root/.arc`.
    pub fn new(root: impl AsRef<Path>) -> Self {
        Self {
            root: root.as_ref().join(".arc"),
        }
    }

    /// Derive the on-disk path for a given BLAKE3 hash.
    ///
    /// Layout: `<root>/.arc/store/{first_2_hex_chars}/{remaining_hex_chars}`
    fn object_path(&self, hash: &Blake3Hash) -> PathBuf {
        let hex = hex_encode(hash);
        self.root.join("store").join(&hex[..2]).join(&hex[2..])
    }

    /// Persist an object under its precomputed BLAKE3 key.
    ///
    /// Semantic guarantee: storage is content-addressed by caller-supplied
    /// BLAKE3 keys, and duplicate writes under an existing path are skipped.
    ///
    /// This is a local best-effort dedup behavior, not a crash-atomic
    /// multi-writer protocol.
    ///
    /// Durability note: this method performs a direct file write and does not
    /// issue an explicit fsync. It guarantees deterministic path placement and
    /// dedup semantics, but power-loss durability must be provided by a higher
    /// durability layer if required.
    pub fn write_object(&self, hash: &Blake3Hash, bytes: &[u8]) -> Result<Blake3Hash, CasError> {
        let path = self.object_path(hash);
        if path.exists() {
            return Ok(*hash);
        }
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&path, bytes)?;
        Ok(*hash)
    }

    /// Read an object back from CAS by BLAKE3 key.
    ///
    /// Files < 4 096 bytes are read into a `Vec<u8>`; larger objects are
    /// memory-mapped to avoid a heap copy.
    pub fn read_object(&self, hash: &Blake3Hash) -> Result<CasBytes, CasError> {
        let path = self.object_path(hash);
        let mut file = fs::File::open(&path)?;
        let len = file.metadata()?.len();
        if len < 4096 {
            let mut buf = Vec::with_capacity(len as usize);
            file.read_to_end(&mut buf)?;
            Ok(CasBytes::Owned(buf))
        } else {
            // SAFETY: CAS objects are immutable after write.
            Ok(CasBytes::Mapped(unsafe { Mmap::map(&file)? }))
        }
    }

    /// Derive the on-disk path for a raw blob in `.arc/blobs/{hex(hash)}`.
    fn blob_path(&self, hash: &Blake3Hash) -> PathBuf {
        self.root.join("blobs").join(hex_encode(hash))
    }

    /// Persist raw bytes as a content-addressed blob.
    ///
    /// Returns the BLAKE3 hash of the content (the blob's storage key).
    /// If the blob already exists the write is skipped.
    pub fn write_blob(&self, bytes: &[u8]) -> Result<Blake3Hash, CasError> {
        let hash: Blake3Hash = *blake3::hash(bytes).as_bytes();
        let path = self.blob_path(&hash);
        if path.exists() {
            return Ok(hash);
        }
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&path, bytes)?;
        Ok(hash)
    }

    /// Read raw bytes for a blob by its BLAKE3 hash.
    ///
    /// Files < 4 096 bytes are read into a `Vec<u8>`; larger blobs are
    /// memory-mapped for zero-copy access.  Callers dereference the result
    /// to obtain a `&[u8]` slice.
    pub fn read_blob(&self, hash: &Blake3Hash) -> Result<CasBytes, CasError> {
        let path = self.blob_path(hash);
        let mut file = fs::File::open(&path)?;
        let len = file.metadata()?.len();
        if len < 4096 {
            let mut buf = Vec::with_capacity(len as usize);
            file.read_to_end(&mut buf)?;
            Ok(CasBytes::Owned(buf))
        } else {
            // SAFETY: CAS blobs are immutable once written — no writer holds
            // a reference after `write_blob` returns.
            Ok(CasBytes::Mapped(unsafe { Mmap::map(&file)? }))
        }
    }

    /// Return `true` when the blob exists in `.arc/blobs/`.
    pub fn contains_blob(&self, hash: &Blake3Hash) -> bool {
        self.blob_path(hash).exists()
    }

    /// Return the filesystem path where the given blob is stored.
    ///
    /// Useful for callers that need to stream the blob directly from disk
    /// rather than loading it into RAM (e.g. the HTTP push path in
    /// `arc-cli` streams via `PUT /blobs/:hash` without buffering).
    pub fn blob_file_path(&self, hash: &Blake3Hash) -> PathBuf {
        self.blob_path(hash)
    }
}

/// Encode a 32-byte hash as a lowercase hex string (64 chars).
fn hex_encode(hash: &Blake3Hash) -> String {
    hash.iter().fold(String::with_capacity(64), |mut acc, b| {
        use std::fmt::Write;
        let _ = write!(acc, "{b:02x}");
        acc
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_object() -> (Blake3Hash, Vec<u8>) {
        let bytes = b"hello-cas".to_vec();
        let hash: Blake3Hash = *blake3::hash(&bytes).as_bytes();
        (hash, bytes)
    }

    #[test]
    fn test_cas_object_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let store = ObjectStore::new(dir.path());

        let (hash, original) = sample_object();
        let written = store.write_object(&hash, &original).unwrap();

        assert_eq!(written, hash, "write must return the object's id");

        let loaded = store.read_object(&hash).unwrap();
        assert_eq!(
            &*loaded,
            original.as_slice(),
            "bytes must roundtrip via CAS"
        );
    }

    #[test]
    fn test_cas_deduplication() {
        let dir = tempfile::tempdir().unwrap();
        let store = ObjectStore::new(dir.path());

        let (hash, bytes) = sample_object();
        let h1 = store.write_object(&hash, &bytes).unwrap();
        let h2 = store.write_object(&hash, &bytes).unwrap();

        assert_eq!(
            h1, h2,
            "writing the same object twice must return the same hash"
        );

        let path = store.object_path(&hash);
        assert!(path.exists());
    }

    #[test]
    fn test_object_path_layout() {
        let store = ObjectStore::new("/repo");
        let mut hash = [0u8; 32];
        hash[0] = 0xab;
        hash[1] = 0xcd;

        let path = store.object_path(&hash);
        let path_str = path.to_string_lossy().replace('\\', "/");
        assert!(
            path_str.contains("/store/ab/"),
            "path must follow {{first_2_hex}}/{{remaining_hex}} layout, got: {path_str}"
        );
    }
}
