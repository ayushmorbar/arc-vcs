use std::fs;
use std::path::{Path, PathBuf};

use memmap2::Mmap;

use crate::algebra::Blake3Hash;
use crate::store::StoreError;
use crate::store::change::Change;

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
    pub fn write_change(&self, change: &Change) -> Result<Blake3Hash, StoreError> {
        let path = self.object_path(&change.id);

        // Dedup: skip write when the object is already present.
        if path.exists() {
            return Ok(change.id);
        }

        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }

        let bytes = bincode::serialize(change)?;
        fs::write(&path, &bytes)?;

        Ok(change.id)
    }

    /// Read a [`Change`] back from the CAS using zero-copy memory mapping.
    pub fn read_change(&self, hash: &Blake3Hash) -> Result<Change, StoreError> {
        let path = self.object_path(hash);
        let file = fs::File::open(path)?;

        // SAFETY: the file is immutable once written (CAS guarantee).
        let mmap = unsafe { Mmap::map(&file)? };

        let change: Change = bincode::deserialize(&mmap)?;
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
    pub fn write_blob(&self, bytes: &[u8]) -> Result<Blake3Hash, StoreError> {
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
    pub fn read_blob(&self, hash: &Blake3Hash) -> Result<Vec<u8>, StoreError> {
        let path = self.blob_path(hash);
        Ok(fs::read(path)?)
    }

    /// Return `true` when the blob exists in `.arc/blobs/`.
    pub fn contains_blob(&self, hash: &Blake3Hash) -> bool {
        self.blob_path(hash).exists()
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
