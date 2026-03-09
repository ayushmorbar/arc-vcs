use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use memmap2::Mmap;

use crate::algebra::Blake3Hash;
use crate::store::StoreError;
use crate::error::{Exn, ResultExt};
use crate::store::change::Change;

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

    /// Persist a [`Change`] to the CAS.
    ///
    /// Returns the change's `id` (its BLAKE3 hash). If the object already
    /// exists on disk the write is skipped (deduplication).
    #[track_caller]
    pub fn write_change(&self, change: &Change) -> Result<Blake3Hash, Exn<StoreError>> {
        let path = self.object_path(&change.id);

        // Dedup: skip write when the object is already present.
            if path.exists() {
                return Ok(change.id);
            }

        if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).or_raise(|| StoreError::Io(std::io::Error::new(std::io::ErrorKind::Other, "create_dir_all failed")))?;
        }

            let bytes = bincode::serialize(change).or_raise(|| StoreError::Serialization(Box::new(bincode::ErrorKind::Custom("serialize failed".to_string()))))?;
            fs::write(&path, &bytes).or_raise(|| StoreError::Io(std::io::Error::new(std::io::ErrorKind::Other, "write failed")))?;

            Ok(change.id)
    }

    /// Read a [`Change`] back from the CAS.
    ///
    /// Files < 4 096 bytes are read into a `Vec<u8>`; larger objects are
    /// memory-mapped to avoid a heap copy.
    #[track_caller]
    pub fn read_change(&self, hash: &Blake3Hash) -> Result<Change, Exn<StoreError>> {
        let path = self.object_path(hash);
            let mut file = fs::File::open(&path).or_raise(|| StoreError::Io(std::io::Error::new(std::io::ErrorKind::Other, "file open failed")))?;
            let len = file.metadata().or_raise(|| StoreError::Io(std::io::Error::new(std::io::ErrorKind::Other, "metadata failed")))?.len();
            let bytes: CasBytes = if len < 4096 {
                let mut buf = Vec::with_capacity(len as usize);
                file.read_to_end(&mut buf).or_raise(|| StoreError::Io(std::io::Error::new(std::io::ErrorKind::Other, "read_to_end failed")))?;
                CasBytes::Owned(buf)
            } else {
                // SAFETY: the file is immutable once written (CAS guarantee).
                CasBytes::Mapped(unsafe { Mmap::map(&file).or_raise(|| StoreError::Io(std::io::Error::new(std::io::ErrorKind::Other, "mmap failed")))? })
            };
            let change: Change = bincode::deserialize(&bytes).or_raise(|| StoreError::Serialization(Box::new(bincode::ErrorKind::Custom("deserialize failed".to_string()))))?;
            Ok(change)
    }

    /// Derive the on-disk path for a raw blob in `.arc/blobs/{hex(hash)}`.
    fn blob_path(&self, hash: &Blake3Hash) -> PathBuf {
        self.root.join("blobs").join(hex_encode(hash))
    }

    /// Persist raw bytes as a content-addressed blob.
    ///
    /// Returns the BLAKE3 hash of the content (the blob's storage key).
    /// If the blob already exists the write is skipped.
    #[track_caller]
    pub fn write_blob(&self, bytes: &[u8]) -> Result<Blake3Hash, Exn<StoreError>> {
        let hash: Blake3Hash = *blake3::hash(bytes).as_bytes();
        let path = self.blob_path(&hash);
            if path.exists() {
                return Ok(hash);
            }
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).or_raise(|| StoreError::Io(std::io::Error::new(std::io::ErrorKind::Other, "create_dir_all failed")))?;
            }
            fs::write(&path, bytes).or_raise(|| StoreError::Io(std::io::Error::new(std::io::ErrorKind::Other, "write failed")))?;
            Ok(hash)
    }

    /// Read raw bytes for a blob by its BLAKE3 hash.
    ///
    /// Files < 4 096 bytes are read into a `Vec<u8>`; larger blobs are
    /// memory-mapped for zero-copy access.  Callers dereference the result
    /// to obtain a `&[u8]` slice.
    #[track_caller]
    pub fn read_blob(&self, hash: &Blake3Hash) -> Result<CasBytes, Exn<StoreError>> {
        let path = self.blob_path(hash);
            let mut file = fs::File::open(&path).or_raise(|| StoreError::Io(std::io::Error::new(std::io::ErrorKind::Other, "file open failed")))?;
            let len = file.metadata().or_raise(|| StoreError::Io(std::io::Error::new(std::io::ErrorKind::Other, "metadata failed")))?.len();
            if len < 4096 {
                let mut buf = Vec::with_capacity(len as usize);
                file.read_to_end(&mut buf).or_raise(|| StoreError::Io(std::io::Error::new(std::io::ErrorKind::Other, "read_to_end failed")))?;
                Ok(CasBytes::Owned(buf))
            } else {
                // SAFETY: CAS blobs are immutable once written — no writer holds
                // a reference after `write_blob` returns.
                Ok(CasBytes::Mapped(unsafe { Mmap::map(&file).or_raise(|| StoreError::Io(std::io::Error::new(std::io::ErrorKind::Other, "mmap failed")))? }))
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
    use std::collections::HashSet;

    use super::*;
    use crate::algebra::Atom;
    use crate::store::change::Change;

    fn sample_change() -> Change {
        let (author, signing_key) = crate::store::author::test_keypair();
        Change::new(
            HashSet::new(),
            vec![Atom::Insert {
                at: vec!["root".into(), "child".into()],
                content_hash: [0u8; 32],
            }],
            "test",
            author,
            &signing_key,
        )
    }

    #[test]
    fn test_cas_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let store = ObjectStore::new(dir.path());

        let original = sample_change();
        let hash = store.write_change(&original).unwrap();

        assert_eq!(hash, original.id, "write must return the change's own id");

        let loaded = store.read_change(&hash).unwrap();
        assert_eq!(
            loaded, original,
            "deserialized change must equal the original"
        );
    }

    #[test]
    fn test_cas_deduplication() {
        let dir = tempfile::tempdir().unwrap();
        let store = ObjectStore::new(dir.path());

        let change = sample_change();
        let h1 = store.write_change(&change).unwrap();
        let h2 = store.write_change(&change).unwrap();

        assert_eq!(
            h1, h2,
            "writing the same change twice must return the same hash"
        );

        let path = store.object_path(&change.id);
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
