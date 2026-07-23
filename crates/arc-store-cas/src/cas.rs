use std::{
    collections::VecDeque,
    ffi::OsStr,
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
    thread,
    time::{SystemTime, UNIX_EPOCH},
};

use arc_algebra_types::Blake3Hash;
use arc_store_types::newtypes::{BlobId, ChangeId};
use bytes::Bytes;
#[cfg(feature = "native-io")]
use memmap2::{Mmap, MmapOptions};
use thiserror::Error;
use tracing::{debug, instrument, warn};

use crate::blake3_hasher::Blake3HashMap;

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
    /// The bytes read from disk did not match the expected content hash.
    #[error("checksum mismatch while reading content-addressed object")]
    ChecksumMismatch,
    /// Caller-provided key does not match payload bytes.
    #[error("caller-supplied object key does not match payload hash")]
    HashMismatch,
}

/// Policy hook that decides if a payload is eligible for cache insertion.
pub trait CachePolicy {
    /// Return `true` when `size_bytes` should be inserted into cache.
    fn should_cache(&self, size_bytes: usize) -> bool;
}

/// Strategy that disables caching entirely.
#[derive(Debug, Clone, Copy, Default)]
pub struct NeverCache;

impl CachePolicy for NeverCache {
    fn should_cache(&self, _size_bytes: usize) -> bool {
        false
    }
}

/// Basic size-window cache policy.
#[derive(Debug, Clone, Copy)]
pub struct SizeWindowCachePolicy {
    /// Minimum object size to admit.
    pub min_bytes: usize,
    /// Maximum object size to admit.
    pub max_bytes: usize,
}

impl Default for SizeWindowCachePolicy {
    fn default() -> Self {
        Self { min_bytes: 1, max_bytes: 2 * 1024 * 1024 }
    }
}

impl CachePolicy for SizeWindowCachePolicy {
    fn should_cache(&self, size_bytes: usize) -> bool {
        (self.min_bytes..=self.max_bytes).contains(&size_bytes)
    }
}

/// Common cache surface used by CAS read paths.
pub trait CasCache {
    /// Attempt to read cached bytes by content key.
    fn get(&mut self, key: &Blake3Hash) -> Option<Bytes>;
    /// Insert bytes for `key`.
    fn put(&mut self, key: Blake3Hash, bytes: &[u8]);
}

/// No-op cache strategy for call-sites that do not want memory caching.
#[derive(Debug, Default)]
pub struct NoCache;

impl CasCache for NoCache {
    fn get(&mut self, _key: &Blake3Hash) -> Option<Bytes> {
        None
    }

    fn put(&mut self, _key: Blake3Hash, _bytes: &[u8]) {}
}

#[derive(Debug)]
struct WeightedEntry {
    bytes: Bytes,
    weight: usize,
}

/// Weighted memory-capped LRU with byte-buffer freelist recycling.
#[derive(Debug)]
pub struct WeightedLruCache {
    capacity_bytes: usize,
    used_bytes: usize,
    entries: Blake3HashMap<WeightedEntry>,
    lru: VecDeque<Blake3Hash>,
    free_buffers: Vec<Vec<u8>>,
}

impl WeightedLruCache {
    /// Create a weighted LRU with a strict memory budget.
    pub fn new(capacity_bytes: usize) -> Self {
        Self {
            capacity_bytes,
            used_bytes: 0,
            entries: Blake3HashMap::default(),
            lru: VecDeque::new(),
            free_buffers: Vec::new(),
        }
    }

    fn touch(&mut self, key: Blake3Hash) {
        self.lru.retain(|k| *k != key);
        self.lru.push_back(key);
    }

    fn evict_until(&mut self, incoming: usize) {
        while self.used_bytes.saturating_add(incoming) > self.capacity_bytes {
            let Some(key) = self.lru.pop_front() else {
                break;
            };
            if let Some(entry) = self.entries.remove(&key) {
                self.used_bytes = self.used_bytes.saturating_sub(entry.weight);
                let mut recycled = entry.bytes.to_vec();
                recycled.clear();
                self.free_buffers.push(recycled);
            }
        }
    }

    /// Current in-memory footprint tracked by the cache.
    pub fn used_bytes(&self) -> usize {
        self.used_bytes
    }

    /// Number of recyclable buffers in the freelist.
    pub fn free_buffer_count(&self) -> usize {
        self.free_buffers.len()
    }
}

impl CasCache for WeightedLruCache {
    fn get(&mut self, key: &Blake3Hash) -> Option<Bytes> {
        let bytes = self.entries.get(key).map(|entry| entry.bytes.clone())?;
        self.touch(*key);
        Some(bytes)
    }

    fn put(&mut self, key: Blake3Hash, bytes: &[u8]) {
        if bytes.len() > self.capacity_bytes {
            return;
        }

        if let Some(prev) = self.entries.remove(&key) {
            self.used_bytes = self.used_bytes.saturating_sub(prev.weight);
            self.lru.retain(|k| *k != key);
            let mut recycled = prev.bytes.to_vec();
            recycled.clear();
            self.free_buffers.push(recycled);
        }

        self.evict_until(bytes.len());

        let mut buf = self.free_buffers.pop().unwrap_or_else(|| Vec::with_capacity(bytes.len()));
        buf.clear();
        buf.extend_from_slice(bytes);
        let stored = Bytes::from(buf);

        self.used_bytes = self.used_bytes.saturating_add(stored.len());
        self.entries.insert(key, WeightedEntry { bytes: stored, weight: bytes.len() });
        self.lru.push_back(key);
    }
}

#[derive(Clone, Debug)]
struct TinyEntry {
    key: Blake3Hash,
    bytes: Bytes,
    prev: Option<usize>,
    next: Option<usize>,
}

/// Fixed-size linked-list LRU for tiny hot sets.
pub struct TinyLinkedLruCache<const N: usize> {
    slots: [Option<TinyEntry>; N],
    index: Blake3HashMap<usize>,
    head: Option<usize>,
    tail: Option<usize>,
    free: Vec<usize>,
}

impl<const N: usize> Default for TinyLinkedLruCache<N> {
    fn default() -> Self {
        let free = (0..N).rev().collect();
        Self {
            slots: std::array::from_fn(|_| None),
            index: Blake3HashMap::default(),
            head: None,
            tail: None,
            free,
        }
    }
}

impl<const N: usize> TinyLinkedLruCache<N> {
    fn detach(&mut self, idx: usize) {
        let Some(entry) = self.slots[idx].as_ref() else {
            return;
        };

        let (prev, next) = (entry.prev, entry.next);
        if let Some(prev_idx) = prev
            && let Some(prev_entry) = self.slots[prev_idx].as_mut()
        {
            prev_entry.next = next;
        }
        if let Some(next_idx) = next
            && let Some(next_entry) = self.slots[next_idx].as_mut()
        {
            next_entry.prev = prev;
        }
        if self.head == Some(idx) {
            self.head = next;
        }
        if self.tail == Some(idx) {
            self.tail = prev;
        }
        if let Some(cur) = self.slots[idx].as_mut() {
            cur.prev = None;
            cur.next = None;
        }
    }

    fn push_back(&mut self, idx: usize) {
        match self.tail {
            Some(tail_idx) => {
                if let Some(tail) = self.slots[tail_idx].as_mut() {
                    tail.next = Some(idx);
                }
                if let Some(cur) = self.slots[idx].as_mut() {
                    cur.prev = Some(tail_idx);
                    cur.next = None;
                }
                self.tail = Some(idx);
            }
            None => {
                self.head = Some(idx);
                self.tail = Some(idx);
                if let Some(cur) = self.slots[idx].as_mut() {
                    cur.prev = None;
                    cur.next = None;
                }
            }
        }
    }

    fn move_to_back(&mut self, idx: usize) {
        if self.tail == Some(idx) {
            return;
        }
        self.detach(idx);
        self.push_back(idx);
    }

    fn allocate_slot(&mut self) -> usize {
        if let Some(idx) = self.free.pop() {
            return idx;
        }
        let idx = self.head.expect("tiny LRU head must exist when full");
        self.detach(idx);
        if let Some(old) = self.slots[idx].take() {
            self.index.remove(&old.key);
        }
        idx
    }
}

impl<const N: usize> CasCache for TinyLinkedLruCache<N> {
    fn get(&mut self, key: &Blake3Hash) -> Option<Bytes> {
        let idx = *self.index.get(key)?;
        let out = self.slots[idx].as_ref().map(|entry| entry.bytes.clone())?;
        self.move_to_back(idx);
        Some(out)
    }

    fn put(&mut self, key: Blake3Hash, bytes: &[u8]) {
        if N == 0 {
            return;
        }

        if let Some(&idx) = self.index.get(&key) {
            if let Some(entry) = self.slots[idx].as_mut() {
                entry.bytes = Bytes::copy_from_slice(bytes);
            }
            self.move_to_back(idx);
            return;
        }

        let idx = self.allocate_slot();
        self.slots[idx] =
            Some(TinyEntry { key, bytes: Bytes::copy_from_slice(bytes), prev: None, next: None });
        self.index.insert(key, idx);
        self.push_back(idx);
    }
}

/// Read-policy knobs for CAS reads.
#[derive(Debug, Clone, Copy)]
pub struct CasReadPolicy {
    /// Minimum size that should attempt mmap.
    pub mmap_threshold_bytes: u64,
}

impl Default for CasReadPolicy {
    fn default() -> Self {
        Self { mmap_threshold_bytes: SMALL_OBJECT_THRESHOLD }
    }
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
    #[cfg(feature = "native-io")]
    Mapped(Mmap),
}

impl std::ops::Deref for CasBytes {
    type Target = [u8];
    #[inline]
    fn deref(&self) -> &[u8] {
        match self {
            CasBytes::Owned(v) => v,
            #[cfg(feature = "native-io")]
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
    read_policy: CasReadPolicy,
}

/// Stable boundary for local CAS implementations used by higher layers.
pub trait CasStorage {
    /// Persist bytes under a caller-supplied content hash.
    fn write_object(&self, hash: &Blake3Hash, bytes: &[u8]) -> Result<Blake3Hash, CasError>;
    /// Read bytes by content hash.
    fn read_object(&self, hash: &Blake3Hash) -> Result<CasBytes, CasError>;
    /// Stream a blob into CAS without buffering the full payload.
    fn write_blob_stream(&self, reader: &mut dyn Read) -> Result<(Blake3Hash, u64), CasError>;
}

/// Canonical local filesystem CAS implementation.
pub type LocalCas = ObjectStore;

impl ObjectStore {
    /// Create a new `ObjectStore` rooted at `root/.arc`.
    pub fn new(root: impl AsRef<Path>) -> Self {
        Self { root: root.as_ref().join(".arc"), read_policy: CasReadPolicy::default() }
    }

    /// Create an `ObjectStore` with an explicit CAS read policy.
    pub fn with_read_policy(root: impl AsRef<Path>, read_policy: CasReadPolicy) -> Self {
        Self { root: root.as_ref().join(".arc"), read_policy }
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
        let computed: Blake3Hash = *blake3::hash(bytes).as_bytes();
        if computed != *hash {
            return Err(CasError::HashMismatch);
        }
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
        read_cas_bytes_with_policy(&path, self.read_policy, Some(hash), None)
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

    /// Persist a blob from a streaming reader without buffering the entire
    /// payload in memory.
    ///
    /// Returns the BLAKE3 content hash and total number of bytes written.
    #[instrument(skip_all)]
    pub fn write_blob_stream<R: Read + ?Sized>(
        &self,
        reader: &mut R,
    ) -> Result<(Blake3Hash, u64), CasError> {
        let blobs_dir = self.root.join("blobs");
        create_dir_all_retry(&blobs_dir)?;

        let tmp_path = create_unique_temp_path(&blobs_dir, OsStr::new("blob-stream"))?;
        let mut tmp = fs::OpenOptions::new().create_new(true).write(true).open(&tmp_path)?;

        let mut hasher = blake3::Hasher::new();
        let mut total_bytes: u64 = 0;
        let mut buffer = [0u8; 64 * 1024];

        loop {
            let read = match reader.read(&mut buffer) {
                Ok(0) => break,
                Ok(n) => n,
                Err(err) if err.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(err) => {
                    let _ = fs::remove_file(&tmp_path);
                    return Err(CasError::Io(err));
                }
            };

            tmp.write_all(&buffer[..read])?;
            hasher.update(&buffer[..read]);
            total_bytes = total_bytes.saturating_add(read as u64);
        }

        tmp.sync_data()?;
        drop(tmp);

        let hash: Blake3Hash = *hasher.finalize().as_bytes();
        let final_path = self.blob_path(&hash);

        if final_path.exists() {
            let _ = fs::remove_file(&tmp_path);
            return Ok((hash, total_bytes));
        }

        match fs::rename(&tmp_path, &final_path) {
            Ok(()) => {
                sync_directory_if_supported(&blobs_dir)?;
                Ok((hash, total_bytes))
            }
            Err(err)
                if err.kind() == std::io::ErrorKind::AlreadyExists
                    || (err.kind() == std::io::ErrorKind::PermissionDenied
                        && final_path.exists()) =>
            {
                let _ = fs::remove_file(&tmp_path);
                Ok((hash, total_bytes))
            }
            Err(err) => {
                let _ = fs::remove_file(&tmp_path);
                Err(CasError::Io(err))
            }
        }
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
        read_cas_bytes_with_policy(&path, self.read_policy, Some(hash), None)
    }

    /// Read a blob with caller-provided cache strategy and policy.
    #[instrument(skip_all)]
    pub fn read_blob_cached<C, P>(
        &self,
        hash: &Blake3Hash,
        cache: &mut C,
        policy: &P,
    ) -> Result<CasBytes, CasError>
    where
        C: CasCache,
        P: CachePolicy,
    {
        if let Some(hit) = cache.get(hash) {
            return Ok(CasBytes::Owned(hit));
        }

        let loaded = self.read_blob(hash)?;
        if policy.should_cache(loaded.len()) {
            cache.put(*hash, loaded.as_ref());
        }
        Ok(loaded)
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
        let path = self.object_path(&id.0);
        write_once_atomic(&path, bytes)?;
        Ok(id)
    }

    /// Read serialized change bytes by typed identifier.
    ///
    /// The returned value preserves zero-copy semantics for large payloads
    /// by using read-only memory maps.
    #[instrument(skip_all)]
    pub fn read_change_bytes(&self, id: ChangeId) -> Result<CasBytes, CasError> {
        let path = self.object_path(&id.0);
        read_cas_bytes_with_policy(&path, self.read_policy, None, None)
    }
}

impl CasStorage for ObjectStore {
    fn write_object(&self, hash: &Blake3Hash, bytes: &[u8]) -> Result<Blake3Hash, CasError> {
        ObjectStore::write_object(self, hash, bytes)
    }

    fn read_object(&self, hash: &Blake3Hash) -> Result<CasBytes, CasError> {
        ObjectStore::read_object(self, hash)
    }

    fn write_blob_stream(&self, reader: &mut dyn Read) -> Result<(Blake3Hash, u64), CasError> {
        ObjectStore::write_blob_stream(self, reader)
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

/// Read CAS bytes with configurable mmap threshold and checksum fallback.
fn read_cas_bytes_with_policy(
    path: &Path,
    read_policy: CasReadPolicy,
    expected_hash: Option<&Blake3Hash>,
    interrupted: Option<&AtomicBool>,
) -> Result<CasBytes, CasError> {
    let mut file = open_read_no_follow(path)?;
    let len = file.metadata()?.len();

    if len < read_policy.mmap_threshold_bytes {
        let owned = read_owned_interrupt_aware(&mut file, len as usize, interrupted)?;
        if let Some(expected) = expected_hash
            && *expected != *blake3::hash(owned.as_ref()).as_bytes()
        {
            return Err(CasError::ChecksumMismatch);
        }
        return Ok(CasBytes::Owned(owned));
    }

    #[cfg(feature = "native-io")]
    {
        // SAFETY: CAS files are immutable after successful publish.
        match unsafe { MmapOptions::new().map(&file) } {
            Ok(mapped) => {
                if let Some(expected) = expected_hash
                    && *expected != *blake3::hash(&mapped).as_bytes()
                {
                    // Fallback to buffered retry path to avoid failing on one mmap
                    // read attempt under interrupted conditions.
                    let mut second = open_read_no_follow(path)?;
                    let owned = read_owned_interrupt_aware(&mut second, len as usize, interrupted)?;
                    if *expected != *blake3::hash(owned.as_ref()).as_bytes() {
                        return Err(CasError::ChecksumMismatch);
                    }
                    return Ok(CasBytes::Owned(owned));
                }
                Ok(CasBytes::Mapped(mapped))
            }
            Err(_) => {
                let mut second = open_read_no_follow(path)?;
                let owned = read_owned_interrupt_aware(&mut second, len as usize, interrupted)?;
                if let Some(expected) = expected_hash
                    && *expected != *blake3::hash(owned.as_ref()).as_bytes()
                {
                    return Err(CasError::ChecksumMismatch);
                }
                Ok(CasBytes::Owned(owned))
            }
        }
    }

    #[cfg(not(feature = "native-io"))]
    {
        let owned = read_owned_interrupt_aware(&mut file, len as usize, interrupted)?;
        if let Some(expected) = expected_hash
            && *expected != *blake3::hash(owned.as_ref()).as_bytes()
        {
            return Err(CasError::ChecksumMismatch);
        }
        Ok(CasBytes::Owned(owned))
    }
}

fn read_owned_interrupt_aware(
    file: &mut fs::File,
    expected_len: usize,
    interrupted: Option<&AtomicBool>,
) -> Result<Bytes, CasError> {
    let mut buf = Vec::with_capacity(expected_len);
    loop {
        if interrupted.is_some_and(|flag| flag.load(Ordering::Relaxed)) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::Interrupted,
                "read interrupted by cancellation flag",
            )
            .into());
        }
        match file.read_to_end(&mut buf) {
            Ok(_) => return Ok(Bytes::from(buf)),
            Err(err) if err.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(err) => return Err(err.into()),
        }
    }
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

    let mut tmp = fs::OpenOptions::new().create_new(true).write(true).open(&tmp_path)?;
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
    use std::sync::{Arc, Barrier};

    use arc_store_types::newtypes::{BlobId, ChangeId};

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
        assert_eq!(&*loaded, original.as_slice(), "bytes must roundtrip via CAS");
    }

    #[test]
    fn test_cas_deduplication() {
        let dir = tempfile::tempdir().unwrap();
        let store = ObjectStore::new(dir.path());

        let (hash, bytes) = sample_object();
        let h1 = store.write_object(&hash, &bytes).unwrap();
        let h2 = store.write_object(&hash, &bytes).unwrap();

        assert_eq!(h1, h2, "writing the same object twice must return the same hash");

        let path = store.object_path(&hash);
        assert!(path.exists());
    }

    #[test]
    fn test_object_path_layout() {
        let store = ObjectStore::new("/repo");
        let mut hash = [0u8; 32];
        hash[0] = 0xAB;
        hash[1] = 0xCD;

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

        let bytes = b"serialized-change";
        let raw: Blake3Hash = *blake3::hash(bytes).as_bytes();
        let id = ChangeId::from(raw);

        let written = store.write_change_bytes(id, bytes).unwrap();
        assert_eq!(written, id);
        let loaded = store.read_change_bytes(id).unwrap();
        assert_eq!(&*loaded, bytes);
    }

    #[test]
    fn test_read_blob_cached_with_never_cache_policy() {
        let dir = tempfile::tempdir().unwrap();
        let store = ObjectStore::new(dir.path());
        let hash = store.write_blob(b"cache-me").unwrap();

        let mut cache = WeightedLruCache::new(1024);
        let loaded = store.read_blob_cached(&hash, &mut cache, &NeverCache).unwrap();

        assert_eq!(&*loaded, b"cache-me");
        assert_eq!(cache.used_bytes(), 0);
    }

    #[test]
    fn test_weighted_lru_evicts_by_memory_budget() {
        let mut cache = WeightedLruCache::new(8);
        let k1: Blake3Hash = *blake3::hash(b"k1").as_bytes();
        let k2: Blake3Hash = *blake3::hash(b"k2").as_bytes();

        cache.put(k1, b"1234");
        cache.put(k2, b"56789");

        assert!(cache.get(&k1).is_none());
        assert_eq!(cache.get(&k2).as_deref(), Some(&b"56789"[..]));
        assert!(cache.used_bytes() <= 8);
    }

    #[test]
    fn test_tiny_linked_lru_prefers_recent_entry() {
        let mut cache = TinyLinkedLruCache::<2>::default();
        let a: Blake3Hash = *blake3::hash(b"a").as_bytes();
        let b: Blake3Hash = *blake3::hash(b"b").as_bytes();
        let c: Blake3Hash = *blake3::hash(b"c").as_bytes();

        cache.put(a, b"A");
        cache.put(b, b"B");
        let _ = cache.get(&a);
        cache.put(c, b"C");

        assert!(cache.get(&b).is_none());
        assert_eq!(cache.get(&a).as_deref(), Some(&b"A"[..]));
        assert_eq!(cache.get(&c).as_deref(), Some(&b"C"[..]));
    }

    #[test]
    fn test_read_policy_uses_owned_for_large_threshold() {
        let dir = tempfile::tempdir().unwrap();
        let policy = CasReadPolicy { mmap_threshold_bytes: 1 << 20 };
        let store = ObjectStore::with_read_policy(dir.path(), policy);
        let hash = store.write_blob(&vec![1u8; 8192]).unwrap();
        let loaded = store.read_blob(&hash).unwrap();
        assert!(matches!(loaded, CasBytes::Owned(_)));
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

    #[test]
    fn test_cas_error_display_covers_all_variants() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "missing");
        let e_io = CasError::Io(io_err);
        assert!(e_io.to_string().contains("I/O error"));

        let e_checksum = CasError::ChecksumMismatch;
        assert!(e_checksum.to_string().contains("checksum mismatch"));

        let e_hash = CasError::HashMismatch;
        assert!(e_hash.to_string().contains("object key does not match"));
    }

    #[test]
    fn test_cas_error_from_io_error() {
        let io_err = std::io::Error::new(std::io::ErrorKind::BrokenPipe, "pipe");
        let cas_err: CasError = io_err.into();
        assert!(matches!(cas_err, CasError::Io(_)));
    }

    #[test]
    fn test_never_cache_always_returns_false() {
        let cache = NeverCache;
        assert!(!cache.should_cache(0));
        assert!(!cache.should_cache(1));
        assert!(!cache.should_cache(usize::MAX));
    }

    #[test]
    fn test_no_cache_get_always_none_and_put_is_noop() {
        let mut cache = NoCache;
        let key: Blake3Hash = *blake3::hash(b"key").as_bytes();
        assert!(cache.get(&key).is_none());
        cache.put(key, b"value");
        assert!(cache.get(&key).is_none());
    }

    #[test]
    fn test_size_window_cache_policy_default() {
        let policy = SizeWindowCachePolicy::default();
        assert_eq!(policy.min_bytes, 1);
        assert_eq!(policy.max_bytes, 2 * 1024 * 1024);
    }

    #[test]
    fn test_size_window_cache_policy_boundary_conditions() {
        let policy = SizeWindowCachePolicy { min_bytes: 10, max_bytes: 100 };
        assert!(!policy.should_cache(9));
        assert!(policy.should_cache(10));
        assert!(policy.should_cache(50));
        assert!(policy.should_cache(100));
        assert!(!policy.should_cache(101));
    }

    #[test]
    fn test_size_window_cache_policy_zero_range() {
        let policy = SizeWindowCachePolicy { min_bytes: 50, max_bytes: 40 };
        assert!(!policy.should_cache(0));
        assert!(!policy.should_cache(45));
        assert!(!policy.should_cache(50));
    }

    #[test]
    fn test_cas_bytes_default_is_empty() {
        let default = CasBytes::default();
        assert_eq!(default.len(), 0);
        assert!(default.is_empty());
    }

    #[test]
    fn test_cas_bytes_as_ref() {
        let bytes = CasBytes::Owned(Bytes::from_static(b"hello"));
        let slice: &[u8] = bytes.as_ref();
        assert_eq!(slice, b"hello");
    }

    #[test]
    fn test_cas_bytes_deref() {
        let bytes = CasBytes::Owned(Bytes::from_static(b"deref-test"));
        assert_eq!(&*bytes, b"deref-test");
    }

    #[test]
    fn test_cas_read_policy_default() {
        let policy = CasReadPolicy::default();
        assert_eq!(policy.mmap_threshold_bytes, SMALL_OBJECT_THRESHOLD);
    }

    #[test]
    fn test_hex_encode_output_format() {
        let mut hash = [0u8; 32];
        hash[0] = 0xAB;
        hash[1] = 0xCD;
        hash[31] = 0xFF;
        let hex = hex_encode(&hash);
        assert_eq!(hex.len(), 64);
        assert!(hex.starts_with("abcd"));
        assert!(hex.ends_with("ff"));
        assert_eq!(&hex[..2], "ab");
    }

    #[test]
    fn test_hex_encode_all_zeros() {
        let hash = [0u8; 32];
        let hex = hex_encode(&hash);
        assert_eq!(hex.len(), 64);
        assert!(hex.chars().all(|c| c == '0'));
    }

    #[test]
    fn test_weighted_lru_oversized_entry_rejected() {
        let mut cache = WeightedLruCache::new(8);
        let key: Blake3Hash = *blake3::hash(b"big").as_bytes();
        let big = vec![1u8; 100];
        cache.put(key, &big);
        assert_eq!(cache.used_bytes(), 0);
        assert!(cache.get(&key).is_none());
    }

    #[test]
    fn test_weighted_lru_exact_capacity_fit() {
        let mut cache = WeightedLruCache::new(4);
        let key: Blake3Hash = *blake3::hash(b"exact").as_bytes();
        cache.put(key, b"1234");
        assert_eq!(cache.used_bytes(), 4);
        assert_eq!(cache.get(&key).as_deref(), Some(&b"1234"[..]));
    }

    #[test]
    fn test_weighted_lru_replaces_same_key() {
        let mut cache = WeightedLruCache::new(16);
        let key: Blake3Hash = *blake3::hash(b"dup").as_bytes();
        cache.put(key, b"first");
        cache.put(key, b"second-longer");
        assert_eq!(cache.used_bytes(), 13);
        assert_eq!(cache.get(&key).as_deref(), Some(&b"second-longer"[..]));
    }

    #[test]
    fn test_weighted_lru_get_promotes_recency() {
        let mut cache = WeightedLruCache::new(5);
        let k1: Blake3Hash = *blake3::hash(b"aa").as_bytes();
        let k2: Blake3Hash = *blake3::hash(b"bb").as_bytes();
        cache.put(k1, b"12");
        cache.put(k2, b"34");
        let _ = cache.get(&k1);
        let k3: Blake3Hash = *blake3::hash(b"cc").as_bytes();
        cache.put(k3, b"56");
        assert!(cache.get(&k2).is_none(), "k2 should be evicted first");
        assert!(cache.get(&k1).is_some(), "k1 should survive (recently accessed)");
    }

    #[test]
    fn test_weighted_lru_freelist_recycling() {
        let mut cache = WeightedLruCache::new(8);
        let k1: Blake3Hash = *blake3::hash(b"a").as_bytes();
        let k2: Blake3Hash = *blake3::hash(b"b").as_bytes();
        cache.put(k1, b"1234");
        let before = cache.free_buffer_count();
        cache.put(k2, b"5678");
        assert_eq!(cache.used_bytes(), 8);
        assert_eq!(cache.free_buffer_count(), before, "freelist should not grow on new entries");
    }

    #[test]
    fn test_tiny_linked_lru_capacity_one() {
        let mut cache = TinyLinkedLruCache::<1>::default();
        let a: Blake3Hash = *blake3::hash(b"a1").as_bytes();
        let b: Blake3Hash = *blake3::hash(b"b2").as_bytes();
        cache.put(a, b"A");
        assert_eq!(cache.get(&a).as_deref(), Some(&b"A"[..]));
        cache.put(b, b"B");
        assert!(cache.get(&a).is_none(), "capacity-1 cache must evict a");
        assert_eq!(cache.get(&b).as_deref(), Some(&b"B"[..]));
    }

    #[test]
    fn test_tiny_linked_lru_update_existing_key() {
        let mut cache = TinyLinkedLruCache::<3>::default();
        let a: Blake3Hash = *blake3::hash(b"upd").as_bytes();
        cache.put(a, b"old");
        cache.put(a, b"new");
        assert_eq!(cache.get(&a).as_deref(), Some(&b"new"[..]));
    }

    #[test]
    fn test_tiny_linked_lru_capacity_two_full_cycle() {
        let mut cache = TinyLinkedLruCache::<2>::default();
        let a: Blake3Hash = *blake3::hash(b"x").as_bytes();
        let b: Blake3Hash = *blake3::hash(b"y").as_bytes();
        let c: Blake3Hash = *blake3::hash(b"z").as_bytes();
        cache.put(a, b"A");
        cache.put(b, b"B");
        cache.put(c, b"C");
        assert!(cache.get(&a).is_none(), "a should be evicted");
        let _ = cache.get(&b); // promote b to tail
        let d: Blake3Hash = *blake3::hash(b"w").as_bytes();
        cache.put(d, b"D");
        assert!(cache.get(&b).is_some(), "b should survive (promoted to tail)");
        assert!(cache.get(&c).is_none(), "c should be evicted (was at head)");
    }

    #[test]
    fn test_blob_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let store = ObjectStore::new(dir.path());
        let data = b"blob-content-addressable-data";
        let hash = store.write_blob(data).unwrap();
        let loaded = store.read_blob(&hash).unwrap();
        assert_eq!(&*loaded, data);
    }

    #[test]
    fn test_blob_deduplication() {
        let dir = tempfile::tempdir().unwrap();
        let store = ObjectStore::new(dir.path());
        let data = b"dedup-blob";
        let h1 = store.write_blob(data).unwrap();
        let h2 = store.write_blob(data).unwrap();
        assert_eq!(h1, h2);
    }

    #[test]
    fn test_contains_blob_returns_bool() {
        let dir = tempfile::tempdir().unwrap();
        let store = ObjectStore::new(dir.path());
        let data = b"check-existence";
        let hash = store.write_blob(data).unwrap();
        assert!(store.contains_blob(&hash));
        let missing: Blake3Hash = *blake3::hash(b"nope").as_bytes();
        assert!(!store.contains_blob(&missing));
    }

    #[test]
    fn test_blob_file_path_returns_correct_location() {
        let dir = tempfile::tempdir().unwrap();
        let store = ObjectStore::new(dir.path());
        let data = b"file-path-test";
        let hash = store.write_blob(data).unwrap();
        let path = store.blob_file_path(&hash);
        assert!(path.exists());
        assert!(path.to_string_lossy().contains("blobs"));
    }

    #[test]
    fn test_write_blob_stream_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let store = ObjectStore::new(dir.path());
        let data = b"streaming-blob-data";
        let mut reader = &data[..];
        let (hash, total) = store.write_blob_stream(&mut reader).unwrap();
        assert_eq!(total, data.len() as u64);
        let loaded = store.read_blob(&hash).unwrap();
        assert_eq!(&*loaded, data);
    }

    #[test]
    fn test_write_blob_stream_dedup() {
        let dir = tempfile::tempdir().unwrap();
        let store = ObjectStore::new(dir.path());
        let data = b"stream-dedup";
        let mut r1 = &data[..];
        let (h1, _) = store.write_blob_stream(&mut r1).unwrap();
        let mut r2 = &data[..];
        let (h2, _) = store.write_blob_stream(&mut r2).unwrap();
        assert_eq!(h1, h2);
    }

    #[test]
    fn test_read_blob_cached_cache_hit() {
        let dir = tempfile::tempdir().unwrap();
        let store = ObjectStore::new(dir.path());
        let data = b"cached-hit";
        let hash = store.write_blob(data).unwrap();
        let mut cache = WeightedLruCache::new(1024);
        let _ = store.read_blob_cached(&hash, &mut cache, &NeverCache).unwrap();
        assert!(cache.get(&hash).is_none(), "NeverCache should not store");
    }

    #[test]
    fn test_read_blob_cached_with_size_window_policy() {
        let dir = tempfile::tempdir().unwrap();
        let store = ObjectStore::new(dir.path());
        let data = b"window-cached";
        let hash = store.write_blob(data).unwrap();
        let mut cache = WeightedLruCache::new(1024);
        let policy = SizeWindowCachePolicy { min_bytes: 1, max_bytes: 1024 };
        let loaded = store.read_blob_cached(&hash, &mut cache, &policy).unwrap();
        assert_eq!(&*loaded, data);
        assert_eq!(cache.used_bytes(), data.len());
    }

    #[test]
    fn test_read_blob_cached_miss_then_hit() {
        let dir = tempfile::tempdir().unwrap();
        let store = ObjectStore::new(dir.path());
        let data = b"miss-to-hit";
        let hash = store.write_blob(data).unwrap();
        let mut cache = WeightedLruCache::new(1024);
        let policy = SizeWindowCachePolicy { min_bytes: 1, max_bytes: 1024 };
        let first = store.read_blob_cached(&hash, &mut cache, &policy).unwrap();
        assert_eq!(&*first, data);
        let second = store.read_blob_cached(&hash, &mut cache, &policy).unwrap();
        assert_eq!(&*second, data);
    }

    #[test]
    fn test_write_blob_stream_empty_reader() {
        let dir = tempfile::tempdir().unwrap();
        let store = ObjectStore::new(dir.path());
        let data: &[u8] = b"";
        let mut reader = data;
        let (hash, total) = store.write_blob_stream(&mut reader).unwrap();
        assert_eq!(total, 0);
        let expected_hash: Blake3Hash = *blake3::hash(b"").as_bytes();
        assert_eq!(hash, expected_hash);
        let loaded = store.read_blob(&hash).unwrap();
        assert!(loaded.is_empty());
    }

    #[test]
    fn test_object_store_new_and_with_read_policy() {
        let dir = tempfile::tempdir().unwrap();
        let store1 = ObjectStore::new(dir.path());
        assert!(store1.root.to_string_lossy().contains(".arc"));

        let policy = CasReadPolicy { mmap_threshold_bytes: 128 };
        let store2 = ObjectStore::with_read_policy(dir.path(), policy);
        assert_eq!(store2.read_policy.mmap_threshold_bytes, 128);
    }

    #[test]
    fn test_cas_storage_trait_impl() {
        let dir = tempfile::tempdir().unwrap();
        let store = ObjectStore::new(dir.path());
        let data = b"trait-impl-test";
        let hash: Blake3Hash = *blake3::hash(data).as_bytes();
        let written = CasStorage::write_object(&store, &hash, data).unwrap();
        assert_eq!(written, hash);
        let loaded = CasStorage::read_object(&store, &hash).unwrap();
        assert_eq!(&*loaded, data);
    }

    #[test]
    fn test_cas_storage_trait_blob_stream() {
        let dir = tempfile::tempdir().unwrap();
        let store = ObjectStore::new(dir.path());
        let data = b"trait-stream";
        let mut reader = &data[..];
        let (hash, total) = CasStorage::write_blob_stream(&store, &mut reader).unwrap();
        assert_eq!(total, data.len() as u64);
        let loaded = store.read_blob(&hash).unwrap();
        assert_eq!(&*loaded, data);
    }

    #[test]
    fn test_write_object_hash_mismatch() {
        let dir = tempfile::tempdir().unwrap();
        let store = ObjectStore::new(dir.path());
        let wrong_hash: Blake3Hash = *blake3::hash(b"wrong-key").as_bytes();
        let data = b"actual-content";
        let err = store.write_object(&wrong_hash, data).unwrap_err();
        assert!(matches!(err, CasError::HashMismatch));
    }

    #[test]
    fn test_read_object_missing_returns_io_error() {
        let dir = tempfile::tempdir().unwrap();
        let store = ObjectStore::new(dir.path());
        let missing: Blake3Hash = *blake3::hash(b"nonexistent").as_bytes();
        let result = store.read_object(&missing);
        assert!(result.is_err());
        match result {
            Err(CasError::Io(_)) => {}
            _ => panic!("expected Io error"),
        }
    }

    #[test]
    fn test_read_blob_missing_returns_io_error() {
        let dir = tempfile::tempdir().unwrap();
        let store = ObjectStore::new(dir.path());
        let missing: Blake3Hash = *blake3::hash(b"missing-blob").as_bytes();
        let result = store.read_blob(&missing);
        assert!(result.is_err());
        match result {
            Err(CasError::Io(_)) => {}
            _ => panic!("expected Io error"),
        }
    }

    #[test]
    fn test_write_change_bytes_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let store = ObjectStore::new(dir.path());
        let raw: Blake3Hash = *blake3::hash(b"change-data").as_bytes();
        let id = ChangeId::from(raw);
        store.write_change_bytes(id, b"payload").unwrap();
        let loaded = store.read_change_bytes(id).unwrap();
        assert_eq!(&*loaded, b"payload");
    }

    #[test]
    fn test_read_change_bytes_missing_returns_io_error() {
        let dir = tempfile::tempdir().unwrap();
        let store = ObjectStore::new(dir.path());
        let raw: Blake3Hash = *blake3::hash(b"missing-change").as_bytes();
        let id = ChangeId::from(raw);
        let result = store.read_change_bytes(id);
        assert!(result.is_err());
        match result {
            Err(CasError::Io(_)) => {}
            _ => panic!("expected Io error"),
        }
    }

    #[test]
    fn test_write_blob_hash_mismatch_not_checked() {
        let dir = tempfile::tempdir().unwrap();
        let store = ObjectStore::new(dir.path());
        let data = b"blob-write-no-check";
        let hash = store.write_blob(data).unwrap();
        let expected: Blake3Hash = *blake3::hash(data).as_bytes();
        assert_eq!(hash, expected, "write_blob returns correct content hash");
    }

    #[test]
    fn test_hex_encode_all_ff() {
        let hash = [0xFFu8; 32];
        let hex = hex_encode(&hash);
        assert_eq!(hex.len(), 64);
        assert!(hex.chars().all(|c| c == 'f'));
    }

    #[test]
    fn test_weighted_lru_empty_cache() {
        let mut cache = WeightedLruCache::new(100);
        let key: Blake3Hash = *blake3::hash(b"empty").as_bytes();
        assert!(cache.get(&key).is_none());
        assert_eq!(cache.used_bytes(), 0);
        assert_eq!(cache.free_buffer_count(), 0);
    }

    #[test]
    fn test_tiny_linked_lru_get_nonexistent() {
        let mut cache = TinyLinkedLruCache::<4>::default();
        let key: Blake3Hash = *blake3::hash(b"ghost").as_bytes();
        assert!(cache.get(&key).is_none());
    }

    #[test]
    fn test_tiny_linked_lru_many_inserts_eviction_order() {
        let mut cache = TinyLinkedLruCache::<3>::default();
        let keys: Vec<Blake3Hash> = (0..5).map(|i| *blake3::hash(&[i]).as_bytes()).collect();
        for (i, k) in keys.iter().enumerate() {
            cache.put(*k, &[i as u8]);
        }
        for (i, k) in keys.iter().enumerate() {
            let expected = if i >= 2 { Some(true) } else { None };
            let got = cache.get(k).is_some();
            assert_eq!(got, expected.is_some(), "keys[{}] present={}", i, got);
        }
    }

    #[cfg(windows)]
    #[test]
    fn test_open_read_no_follow_rejects_symlink_on_windows() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real_win.bin");
        fs::write(&real, b"win").unwrap();

        std::os::windows::fs::symlink_file(&real, dir.path().join("link_win.bin")).unwrap();
        let err =
            open_read_no_follow(&dir.path().join("link_win.bin")).expect_err("symlink must fail");
        assert!(matches!(err, CasError::Io(_)));
    }

    #[test]
    fn test_tiny_linked_lru_capacity_zero_put_is_noop() {
        let mut cache = TinyLinkedLruCache::<0>::default();
        let key: Blake3Hash = *blake3::hash(b"zero-cap").as_bytes();
        cache.put(key, b"value");
        assert!(cache.get(&key).is_none(), "capacity-0 cache must never store entries");
    }

    #[test]
    fn test_weighted_lru_zero_capacity_rejects_all_puts() {
        let mut cache = WeightedLruCache::new(0);
        let k1: Blake3Hash = *blake3::hash(b"a").as_bytes();
        let k2: Blake3Hash = *blake3::hash(b"b").as_bytes();
        cache.put(k1, b"one");
        cache.put(k2, b"two");
        assert!(cache.get(&k1).is_none(), "zero-capacity cache must reject put");
        assert!(cache.get(&k2).is_none(), "zero-capacity cache must reject put");
        assert_eq!(cache.used_bytes(), 0, "used_bytes must stay 0");
    }

    #[test]
    fn test_read_blob_typed_roundtrip_via_write_blob() {
        let dir = tempfile::tempdir().unwrap();
        let store = ObjectStore::new(dir.path());
        let data = b"typed-roundtrip-payload";
        let hash = store.write_blob(data).unwrap();
        let id = BlobId(hash);
        let loaded = store.read_blob_typed(id).unwrap();
        assert_eq!(&*loaded, data, "read_blob_typed must return bytes written via write_blob");
    }

    #[test]
    fn test_contains_blob_false_for_objects() {
        let dir = tempfile::tempdir().unwrap();
        let store = ObjectStore::new(dir.path());
        let (hash, bytes) = sample_object();
        store.write_object(&hash, &bytes).unwrap();
        assert!(
            !store.contains_blob(&hash),
            "contains_blob must return false for an object stored via write_object"
        );
    }

    #[test]
    fn test_cas_bytes_default_len_zero_deref_empty() {
        let default = CasBytes::default();
        assert_eq!(default.len(), 0, "default CasBytes must have len 0");
        let slice: &[u8] = &default;
        assert!(slice.is_empty(), "default CasBytes must deref to empty slice");
    }
}
