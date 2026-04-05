use std::ffi::OsStr;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

use arc_algebra_types::Blake3Hash;
use arc_store_types::newtypes::{BlobId, ChangeId};
use bytes::Bytes;
use memmap2::{Mmap, MmapOptions};
use thiserror::Error;
use tracing::{debug, instrument, warn};

const SMALL_OBJECT_THRESHOLD: u64 = 4096;
static TEMP_FILE_COUNTER: AtomicU64 = AtomicU64::new(0);
const CREATE_DIR_RETRY_LIMIT: usize = 6;
const CREATE_DIR_RETRY_DELAY_MS: u64 = 2;

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
    ///
    /// `Bytes` preserves zero-copy handoff into downstream streaming paths.
    Owned(Bytes),
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
        CasBytes::Owned(Bytes::new())
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
    /// CAS publish semantics are write-once: once an object path is present,
    /// concurrent writers will observe a benign dedup outcome.
    ///
    /// Durability note: publish uses temp-file write + `sync_data()` +
    /// atomic rename + directory sync (on supported platforms).
    #[instrument(skip_all)]
    pub fn write_object(&self, hash: &Blake3Hash, bytes: &[u8]) -> Result<Blake3Hash, CasError> {
        let path = self.object_path(hash);
        write_once_atomic(&path, bytes)?;
        Ok(*hash)
    }

    /// Read an object back from CAS by BLAKE3 key.
    ///
    /// Files < 4 096 bytes are read into a `Vec<u8>`; larger objects are
    /// memory-mapped to avoid a heap copy.
    #[instrument(skip_all)]
    pub fn read_object(&self, hash: &Blake3Hash) -> Result<CasBytes, CasError> {
        let path = self.object_path(hash);
        read_cas_bytes(&path)
    }

    /// Derive the on-disk path for a raw blob in `.arc/blobs/{hex(hash)}`.
    fn blob_path(&self, hash: &Blake3Hash) -> PathBuf {
        self.root.join("blobs").join(hex_encode(hash))
    }

    /// Persist raw bytes as a content-addressed blob.
    ///
    /// Returns the BLAKE3 hash of the content (the blob's storage key).
    /// If the blob already exists the write is skipped.
    #[instrument(skip_all)]
    pub fn write_blob(&self, bytes: &[u8]) -> Result<Blake3Hash, CasError> {
        let hash: Blake3Hash = *blake3::hash(bytes).as_bytes();
        let path = self.blob_path(&hash);
        write_once_atomic(&path, bytes)?;
        Ok(hash)
    }

    /// Persist raw bytes as a content-addressed blob and return a typed
    /// identifier for cross-crate APIs.
    ///
    /// This wrapper keeps type boundaries explicit (`BlobId` instead of
    /// bare hash arrays) while preserving the same CAS write path.
    #[instrument(skip_all)]
    pub fn write_blob_typed(&self, bytes: &[u8]) -> Result<BlobId, CasError> {
        self.write_blob(bytes).map(BlobId::from)
    }

    /// Read raw bytes for a blob by its BLAKE3 hash.
    ///
    /// Files < 4 096 bytes are read into a `Vec<u8>`; larger blobs are
    /// memory-mapped for zero-copy access.  Callers dereference the result
    /// to obtain a `&[u8]` slice.
    #[instrument(skip_all)]
    pub fn read_blob(&self, hash: &Blake3Hash) -> Result<CasBytes, CasError> {
        let path = self.blob_path(hash);
        read_cas_bytes(&path)
    }

    /// Read a blob by typed id.
    ///
    /// This keeps call-sites in orchestration crates free from raw hash
    /// plumbing while preserving zero-copy read behavior.
    #[instrument(skip_all)]
    pub fn read_blob_typed(&self, id: BlobId) -> Result<CasBytes, CasError> {
        self.read_blob(&id.0)
    }

    /// Return `true` when the blob exists in `.arc/blobs/`.
    #[instrument(skip_all)]
    pub fn contains_blob(&self, hash: &Blake3Hash) -> bool {
        self.blob_path(hash).exists()
    }

    /// Return the filesystem path where the given blob is stored.
    ///
    /// Useful for callers that need to stream the blob directly from disk
    /// rather than loading it into RAM (e.g. the HTTP push path in
    /// `arc-cli` streams via `PUT /blobs/:hash` without buffering).
    #[instrument(skip_all)]
    pub fn blob_file_path(&self, hash: &Blake3Hash) -> PathBuf {
        self.blob_path(hash)
    }

    /// Persist serialized change bytes under a typed change identifier.
    ///
    /// I/O boundary: this method writes to local CAS only. Network transfer,
    /// signature verification, and merge algebra remain outside this crate.
    #[instrument(skip_all)]
    pub fn write_change_bytes(&self, id: ChangeId, bytes: &[u8]) -> Result<ChangeId, CasError> {
        self.write_object(&id.0, bytes)?;
        Ok(id)
    }

    /// Read serialized change bytes by typed identifier.
    ///
    /// The returned value preserves zero-copy semantics for large payloads
    /// by using read-only memory maps.
    #[instrument(skip_all)]
    pub fn read_change_bytes(&self, id: ChangeId) -> Result<CasBytes, CasError> {
        self.read_object(&id.0)
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

/// Read CAS bytes with a small-object fast path and mmap for larger blobs.
///
/// This mirrors the high-throughput split used in modern object databases:
/// avoid mmap setup overhead for tiny payloads while preserving zero-copy
/// access for larger immutable files.
fn read_cas_bytes(path: &Path) -> Result<CasBytes, CasError> {
    let mut file = open_read_no_follow(path)?;
    let len = file.metadata()?.len();

    if len < SMALL_OBJECT_THRESHOLD {
        let mut buf = Vec::with_capacity(len as usize);
        file.read_to_end(&mut buf)?;
        return Ok(CasBytes::Owned(Bytes::from(buf)));
    }

    // SAFETY: CAS files are immutable after successful publish.
    let mapped = unsafe { MmapOptions::new().map(&file)? };
    Ok(CasBytes::Mapped(mapped))
}

/// Publish bytes atomically into CAS with crash-consistency semantics.
///
/// The write protocol is:
/// 1. write full payload into a unique temp file in the destination directory,
/// 2. fsync the temp file data,
/// 3. atomically rename temp into final path,
/// 4. fsync the parent directory when supported by the platform.
///
/// If another writer already published the same object concurrently,
/// this treats the race as a benign dedup outcome.
fn write_once_atomic(path: &Path, bytes: &[u8]) -> Result<bool, CasError> {
    if path.exists() {
        return Ok(false);
    }

    let parent = path.parent().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "CAS object path must have a parent directory",
        )
    })?;
    create_dir_all_retry(parent)?;

    let file_name = path.file_name().unwrap_or_else(|| OsStr::new("cas-object"));
    let tmp_path = create_unique_temp_path(parent, file_name)?;

    let mut tmp = fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&tmp_path)?;
    tmp.write_all(bytes)?;
    tmp.sync_data()?;
    drop(tmp);

    match fs::rename(&tmp_path, path) {
        Ok(()) => {
            sync_directory_if_supported(parent)?;
            Ok(true)
        }
        Err(err)
            if err.kind() == std::io::ErrorKind::AlreadyExists
                || (err.kind() == std::io::ErrorKind::PermissionDenied && path.exists()) =>
        {
            let _ = fs::remove_file(&tmp_path);
            Ok(false)
        }
        Err(err) => {
            let _ = fs::remove_file(&tmp_path);
            Err(err.into())
        }
    }
}

fn create_dir_all_retry(parent: &Path) -> Result<(), CasError> {
    for attempt in 0..CREATE_DIR_RETRY_LIMIT {
        match fs::create_dir_all(parent) {
            Ok(()) => return Ok(()),
            Err(err)
                if matches!(
                    err.kind(),
                    std::io::ErrorKind::Interrupted | std::io::ErrorKind::NotFound
                ) && attempt + 1 < CREATE_DIR_RETRY_LIMIT =>
            {
                debug!(
                    path = %parent.display(),
                    attempt = attempt + 1,
                    max_attempts = CREATE_DIR_RETRY_LIMIT,
                    kind = %err.kind(),
                    "retrying CAS parent directory creation"
                );
                thread::sleep(std::time::Duration::from_millis(CREATE_DIR_RETRY_DELAY_MS));
            }
            Err(err) => {
                warn!(
                    path = %parent.display(),
                    attempt = attempt + 1,
                    max_attempts = CREATE_DIR_RETRY_LIMIT,
                    kind = %err.kind(),
                    "failed to create CAS parent directory"
                );
                return Err(err.into());
            }
        }
    }

    Err(std::io::Error::other("exhausted CAS parent directory creation retries").into())
}

#[cfg(unix)]
fn open_read_no_follow(path: &Path) -> Result<fs::File, CasError> {
    use std::os::unix::fs::OpenOptionsExt;

    fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
        .map_err(CasError::from)
}

#[cfg(windows)]
fn open_read_no_follow(path: &Path) -> Result<fs::File, CasError> {
    use std::os::windows::fs::{MetadataExt, OpenOptionsExt};

    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0400;
    const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
    let meta = fs::symlink_metadata(path)?;
    let file_type = meta.file_type();
    if file_type.is_symlink() || (meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT) != 0 {
        return Err(std::io::Error::other(format!(
            "refusing to follow reparse-point path {}",
            path.display()
        ))
        .into());
    }

    fs::OpenOptions::new()
        .read(true)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
        .map_err(CasError::from)
}

#[cfg(all(not(unix), not(windows)))]
fn open_read_no_follow(path: &Path) -> Result<fs::File, CasError> {
    fs::File::open(path).map_err(CasError::from)
}

fn create_unique_temp_path(parent: &Path, file_name: &OsStr) -> Result<PathBuf, CasError> {
    let pid = std::process::id();
    let base = file_name.to_string_lossy();

    for _ in 0..16 {
        let tick = TEMP_FILE_COUNTER.fetch_add(1, Ordering::Relaxed);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0u128, |duration| duration.as_nanos());
        let candidate = parent.join(format!(".{base}.tmp-{pid}-{nanos}-{tick}"));
        if !candidate.exists() {
            return Ok(candidate);
        }
    }

    Err(std::io::Error::new(
        std::io::ErrorKind::AlreadyExists,
        "failed to allocate a unique CAS temp-file path after 16 attempts",
    )
    .into())
}

#[cfg(unix)]
fn sync_directory_if_supported(parent: &Path) -> std::io::Result<()> {
    fs::File::open(parent)?.sync_all()
}

#[cfg(not(unix))]
fn sync_directory_if_supported(_parent: &Path) -> std::io::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use arc_store_types::newtypes::{BlobId, ChangeId};
    use std::sync::{Arc, Barrier};

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

    #[test]
    fn test_typed_blob_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let store = ObjectStore::new(dir.path());

        let id: BlobId = store.write_blob_typed(b"typed-blob").unwrap();
        let loaded = store.read_blob_typed(id).unwrap();
        assert_eq!(&*loaded, b"typed-blob");
    }

    #[test]
    fn test_typed_change_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let store = ObjectStore::new(dir.path());

        let raw: Blake3Hash = *blake3::hash(b"change-bytes").as_bytes();
        let id = ChangeId::from(raw);
        let bytes = b"serialized-change";

        let written = store.write_change_bytes(id, bytes).unwrap();
        assert_eq!(written, id);
        let loaded = store.read_change_bytes(id).unwrap();
        assert_eq!(&*loaded, bytes);
    }

    #[test]
    fn test_create_dir_all_retry_under_contention() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("nested").join("contended").join("path");
        let barrier = Arc::new(Barrier::new(2));

        let mut handles = Vec::new();
        for _ in 0..2 {
            let barrier = Arc::clone(&barrier);
            let target = target.clone();
            handles.push(std::thread::spawn(move || {
                barrier.wait();
                create_dir_all_retry(&target)
            }));
        }

        for handle in handles {
            handle.join().expect("thread join").expect("create dir");
        }
        assert!(target.is_dir());
    }

    #[test]
    fn test_create_dir_all_retry_fails_on_non_directory_parent() {
        let dir = tempfile::tempdir().unwrap();
        let file_parent = dir.path().join("not-a-dir");
        fs::write(&file_parent, b"x").unwrap();

        let err = create_dir_all_retry(&file_parent).expect_err("file path is not a directory");
        assert!(
            matches!(err, CasError::Io(ref io) if io.kind() == std::io::ErrorKind::AlreadyExists),
            "unexpected error: {err}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn test_open_read_no_follow_rejects_symlink() {
        use std::os::unix::fs::symlink;

        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real.bin");
        fs::write(&real, b"abc").unwrap();

        let link = dir.path().join("link.bin");
        symlink(&real, &link).unwrap();

        let err = open_read_no_follow(&link).expect_err("symlink read must fail with no-follow");
        assert!(
            matches!(
                err,
                CasError::Io(ref io)
                if matches!(io.kind(), std::io::ErrorKind::Other | std::io::ErrorKind::InvalidInput)
                    || io.raw_os_error() == Some(libc::ELOOP)
            ),
            "unexpected error: {err}"
        );
    }
}
