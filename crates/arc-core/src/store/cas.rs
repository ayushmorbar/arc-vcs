use std::path::{Path, PathBuf};

use arc_store_cas::cas::ObjectStore as RawObjectStore;
use tracing::info_span;

pub use arc_store_cas::cas::CasBytes;

use crate::algebra::Blake3Hash;
use crate::error::{Exn, ResultExt};
use crate::ops::OperationStage;
use crate::store::StoreError;
use crate::store::change::Change;

/// Facade ObjectStore preserving arc-core API while delegating raw CAS IO to arc-store-cas.
pub struct ObjectStore {
    inner: RawObjectStore,
    #[cfg(test)]
    root: PathBuf,
}

impl ObjectStore {
    /// Create a new `ObjectStore` rooted at `root/.arc`.
    pub fn new(root: impl AsRef<Path>) -> Self {
        let root = root.as_ref().to_path_buf();
        Self {
            inner: RawObjectStore::new(&root),
            #[cfg(test)]
            root: root.join(".arc"),
        }
    }

    #[cfg(test)]
    /// Derive the on-disk path for a given BLAKE3 hash.
    fn object_path(&self, hash: &Blake3Hash) -> PathBuf {
        let hex = hash.iter().fold(String::with_capacity(64), |mut acc, b| {
            use std::fmt::Write;
            let _ = write!(acc, "{b:02x}");
            acc
        });
        self.root.join("store").join(&hex[..2]).join(&hex[2..])
    }

    /// Persist a [`Change`] to the CAS.
    #[track_caller]
    pub fn write_change(&self, change: &Change) -> Result<Blake3Hash, Exn<StoreError>> {
        let discover_span = info_span!(
            "arc_core.cas.write_change",
            stage = %OperationStage::Discover,
            object = "change"
        );
        let bytes = discover_span.in_scope(|| {
            bincode::serialize(change).or_raise(|| {
                StoreError::Serialization(Box::new(bincode::ErrorKind::Custom(
                    "serialize failed".to_string(),
                )))
            })
        })?;

        let transfer_span = info_span!(
            "arc_core.cas.write_change",
            stage = %OperationStage::Transfer,
            object = "change",
            bytes = bytes.len()
        );
        let hash = transfer_span.in_scope(|| {
            self.inner
                .write_object(&change.id, &bytes)
                .or_raise(|| StoreError::Io(std::io::Error::other("write object failed")))
        })?;

        let finalize_span = info_span!(
            "arc_core.cas.write_change",
            stage = %OperationStage::Finalize,
            object = "change"
        );
        finalize_span.in_scope(|| ());

        Ok(hash)
    }

    /// Read a [`Change`] back from the CAS.
    #[track_caller]
    pub fn read_change(&self, hash: &Blake3Hash) -> Result<Change, Exn<StoreError>> {
        let transfer_span = info_span!(
            "arc_core.cas.read_change",
            stage = %OperationStage::Transfer,
            object = "change"
        );
        let bytes = transfer_span.in_scope(|| {
            self.inner
                .read_object(hash)
                .or_raise(|| StoreError::Io(std::io::Error::other("read object failed")))
        })?;

        let materialize_span = info_span!(
            "arc_core.cas.read_change",
            stage = %OperationStage::Materialize,
            object = "change",
            bytes = bytes.len()
        );
        let change: Change = materialize_span.in_scope(|| {
            bincode::deserialize(&bytes).or_raise(|| {
                StoreError::Serialization(Box::new(bincode::ErrorKind::Custom(
                    "deserialize failed".to_string(),
                )))
            })
        })?;

        Ok(change)
    }

    /// Persist raw bytes as a content-addressed blob.
    #[track_caller]
    pub fn write_blob(&self, bytes: &[u8]) -> Result<Blake3Hash, Exn<StoreError>> {
        let transfer_span = info_span!(
            "arc_core.cas.write_blob",
            stage = %OperationStage::Transfer,
            object = "blob",
            bytes = bytes.len()
        );
        transfer_span.in_scope(|| {
            self.inner
                .write_blob(bytes)
                .or_raise(|| StoreError::Io(std::io::Error::other("write blob failed")))
        })
    }

    /// Read raw bytes for a blob by its BLAKE3 hash.
    #[track_caller]
    pub fn read_blob(&self, hash: &Blake3Hash) -> Result<CasBytes, Exn<StoreError>> {
        let transfer_span = info_span!(
            "arc_core.cas.read_blob",
            stage = %OperationStage::Transfer,
            object = "blob"
        );
        transfer_span.in_scope(|| {
            self.inner
                .read_blob(hash)
                .or_raise(|| StoreError::Io(std::io::Error::other("read blob failed")))
        })
    }

    /// Return `true` when the blob exists in `.arc/blobs/`.
    pub fn contains_blob(&self, hash: &Blake3Hash) -> bool {
        self.inner.contains_blob(hash)
    }

    /// Return the filesystem path where the given blob is stored.
    pub fn blob_file_path(&self, hash: &Blake3Hash) -> PathBuf {
        self.inner.blob_file_path(hash)
    }
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
