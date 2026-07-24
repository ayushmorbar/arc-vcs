use std::{
    cell::RefCell,
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Arc,
};

use arc_algebra_types::Blake3Hash;
use arc_store_cas::cas::ObjectStore;
use arc_swap::ArcSwap;
use thiserror::Error;

use crate::{
    ops::{OperationStage, SloTimer},
    store::StoreError,
};

/// Public result type for repository facade operations.
pub type ArcResult<T> = Result<T, ArcError>;

/// Unified facade error that aggregates domain-level crate errors.
#[derive(Debug, Error)]
pub enum ArcError {
    /// Filesystem and path operations failed.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    /// Persistent view and serialization failures.
    #[error("store error: {0}")]
    Store(#[from] StoreError),
    /// CAS-specific content-addressed storage failures.
    #[error("cas error: {0}")]
    Cas(#[from] arc_store_cas::cas::CasError),
    /// Option combinations that violate repository-open constraints.
    #[error("invalid open options: {reason}")]
    InvalidOpenOptions {
        /// Human-readable explanation for why options are invalid.
        reason: String,
    },
    /// A policy decision denied writing for the provided path.
    #[error("policy denied write for path: {path}")]
    PolicyDenied {
        /// Relative path rejected by policy.
        path: String,
    },
    /// Policy evaluation and load failures (native-only).
    #[cfg(all(feature = "native", not(target_arch = "wasm32")))]
    #[error("policy error: {0}")]
    Policy(#[from] arc_store_policy::PolicyStoreError),
    /// Transport-level failures (native-only).
    #[cfg(all(feature = "native", not(target_arch = "wasm32")))]
    #[error("network error: {0}")]
    Network(#[from] anyhow::Error),
}

/// Safety/trust profile used while opening repositories.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TrustMode {
    /// Favor strict checks over convenience.
    Strict,
    /// Balanced checks suitable for local development.
    #[default]
    Balanced,
    /// Prefer permissive behavior for trusted automation.
    Permissive,
}

/// Local read-cache strategy for thread-local repository handles.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheMode {
    /// Disable local read-through caching.
    Disabled,
    /// Cap the number of blob entries kept in-memory per handle.
    Entries {
        /// Maximum blob entries retained in this thread-local cache.
        max_entries: usize,
    },
}

impl Default for CacheMode {
    fn default() -> Self {
        Self::Entries { max_entries: 256 }
    }
}

/// Policy source behavior while opening repositories.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PolicyMode {
    /// Enforce path policy for path-aware writes and validate policy at open time.
    Enforce,
    /// Skip policy loading for isolated or test contexts.
    #[default]
    Bypass,
}

/// Builder for repository instantiation policy and performance settings.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct OpenOptions {
    trust_mode: TrustMode,
    cache_mode: CacheMode,
    policy_mode: PolicyMode,
}

impl OpenOptions {
    /// Create options with safe defaults.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set trust posture used for repository operations.
    pub fn trust_mode(mut self, mode: TrustMode) -> Self {
        self.trust_mode = mode;
        self
    }

    /// Set local cache behavior for thread-local handles.
    pub fn cache_mode(mut self, mode: CacheMode) -> Self {
        self.cache_mode = mode;
        self
    }

    /// Set policy behavior at open time.
    pub fn policy_mode(mut self, mode: PolicyMode) -> Self {
        self.policy_mode = mode;
        self
    }

    /// Build a thread-safe shared repository handle from `root`.
    pub fn open(self, root: impl AsRef<Path>) -> ArcResult<SharedRepository> {
        SharedRepository::open_with_options(root, self)
    }

    /// Return current trust mode.
    pub fn configured_trust_mode(&self) -> TrustMode {
        self.trust_mode
    }

    /// Return current cache mode.
    pub fn configured_cache_mode(&self) -> CacheMode {
        self.cache_mode
    }

    /// Return current policy mode.
    pub fn configured_policy_mode(&self) -> PolicyMode {
        self.policy_mode
    }
}

struct SharedState {
    root: PathBuf,
    store: Arc<ObjectStore>,
    frontier: ArcSwap<Vec<Blake3Hash>>,
    options: OpenOptions,
    #[cfg(all(feature = "native", not(target_arch = "wasm32")))]
    policy_matcher: Option<arc_store_policy::ArcIgnoreMatcher>,
}

/// Thread-safe repository handle that carries immutable shared state.
#[derive(Clone)]
pub struct SharedRepository {
    inner: Arc<SharedState>,
}

impl SharedRepository {
    /// Open a repository with default options.
    pub fn open(root: impl AsRef<Path>) -> ArcResult<Self> {
        Self::open_with_options(root, OpenOptions::new())
    }

    /// Open a repository with custom options.
    pub fn open_with_options(root: impl AsRef<Path>, options: OpenOptions) -> ArcResult<Self> {
        if let CacheMode::Entries { max_entries } = options.cache_mode
            && max_entries == 0
        {
            return Err(ArcError::InvalidOpenOptions {
                reason: "cache entry budget must be greater than zero".to_string(),
            });
        }

        let root = root.as_ref().to_path_buf();

        #[cfg(any(not(feature = "native"), target_arch = "wasm32"))]
        if matches!(options.policy_mode, PolicyMode::Enforce) {
            return Err(ArcError::InvalidOpenOptions {
                reason: "PolicyMode::Enforce requires the 'native' feature".to_string(),
            });
        }

        #[cfg(all(feature = "native", not(target_arch = "wasm32")))]
        let policy_matcher = if matches!(options.policy_mode, PolicyMode::Enforce) {
            Some(arc_store_policy::ArcIgnoreMatcher::load(&root)?)
        } else {
            None
        };

        let state = SharedState {
            store: Arc::new(ObjectStore::new(&root)),
            root,
            frontier: ArcSwap::new(Arc::new(Vec::new())),
            options,
            #[cfg(all(feature = "native", not(target_arch = "wasm32")))]
            policy_matcher,
        };

        Ok(Self { inner: Arc::new(state) })
    }

    /// Return the repository root path.
    pub fn root(&self) -> &Path {
        &self.inner.root
    }

    /// Return open options used to construct this repository.
    pub fn options(&self) -> &OpenOptions {
        &self.inner.options
    }

    /// Convert shared handle into a thread-local handle with local caches.
    pub fn to_thread_local(&self) -> Repository {
        Repository::from_shared(self.clone())
    }

    /// Replace the current frontier snapshot atomically.
    pub fn set_frontier(&self, frontier: Vec<Blake3Hash>) {
        self.inner.frontier.store(Arc::new(frontier));
    }

    /// Read the current frontier snapshot.
    pub fn frontier(&self) -> Arc<Vec<Blake3Hash>> {
        self.inner.frontier.load_full()
    }

    /// Create a native network client using facade defaults.
    #[cfg(all(feature = "native", not(target_arch = "wasm32")))]
    pub fn network_client(&self) -> ArcResult<arc_network::NetworkClient> {
        let _ = self;
        Ok(arc_network::NetworkClient::new()?)
    }

    fn store(&self) -> Arc<ObjectStore> {
        Arc::clone(&self.inner.store)
    }

    /// Run a full CRDT sync cycle under stage-tagged tracing and an end-to-end latency SLO.
    ///
    /// The closure should execute the full synchronization exchange. This wrapper
    /// emits stage spans for discover/negotiate/transfer/materialize/finalize and
    /// logs a warning when total cycle latency exceeds `slo_threshold`.
    pub fn with_sync_cycle_slo<T>(
        &self,
        operation: &str,
        slo_threshold: std::time::Duration,
        run: impl FnOnce() -> ArcResult<T>,
    ) -> ArcResult<T> {
        let timer = SloTimer::new(operation, slo_threshold);

        timer.stage(OperationStage::Discover, || ());
        timer.stage(OperationStage::Negotiate, || ());

        let result = timer.stage(OperationStage::Transfer, run);

        timer.stage(OperationStage::Materialize, || ());
        timer.stage(OperationStage::Finalize, || ());
        timer.finish();

        result
    }

    /// Run a full CRDT sync cycle using the configured default SLO threshold.
    pub fn with_sync_cycle<T>(
        &self,
        operation: &str,
        run: impl FnOnce() -> ArcResult<T>,
    ) -> ArcResult<T> {
        let timer = SloTimer::from_env(operation);

        timer.stage(OperationStage::Discover, || ());
        timer.stage(OperationStage::Negotiate, || ());

        let result = timer.stage(OperationStage::Transfer, run);

        timer.stage(OperationStage::Materialize, || ());
        timer.stage(OperationStage::Finalize, || ());
        timer.finish();

        result
    }
}

/// Thread-local repository handle with mutable read-through caches.
pub struct Repository {
    shared: SharedRepository,
    local_blob_cache: RefCell<HashMap<Blake3Hash, Vec<u8>>>,
}

impl Repository {
    /// Open a repository with default options.
    pub fn open(root: impl AsRef<Path>) -> ArcResult<Self> {
        SharedRepository::open(root).map(|repo| repo.to_thread_local())
    }

    /// Open a repository with custom options.
    pub fn open_with_options(root: impl AsRef<Path>, options: OpenOptions) -> ArcResult<Self> {
        SharedRepository::open_with_options(root, options).map(|repo| repo.to_thread_local())
    }

    /// Create a local handle from a shared repository.
    pub fn from_shared(shared: SharedRepository) -> Self {
        Self { shared, local_blob_cache: RefCell::new(HashMap::new()) }
    }

    /// Return the shared state handle backing this repository.
    pub fn shared(&self) -> &SharedRepository {
        &self.shared
    }

    /// Persist a blob in content-addressed storage.
    pub fn write_blob(&self, bytes: &[u8]) -> ArcResult<Blake3Hash> {
        let hash = self.shared.store().write_blob(bytes)?;
        self.maybe_cache_insert(hash, bytes);
        Ok(hash)
    }

    /// Persist a blob for a logical repository path after policy enforcement.
    pub fn write_blob_for_path(&self, relative_path: &str, bytes: &[u8]) -> ArcResult<Blake3Hash> {
        if self.policy_denies(relative_path) {
            return Err(ArcError::PolicyDenied { path: relative_path.to_string() });
        }
        self.write_blob(bytes)
    }

    /// Read a blob with optional thread-local cache.
    pub fn read_blob(&self, hash: &Blake3Hash) -> ArcResult<Vec<u8>> {
        if let Some(bytes) = self.local_blob_cache.borrow().get(hash) {
            return Ok(bytes.clone());
        }

        let bytes = self.shared.store().read_blob(hash)?;
        let owned = bytes.as_ref().to_vec();
        self.maybe_cache_insert(*hash, &owned);
        Ok(owned)
    }

    /// Return the number of cached blobs in this local handle.
    pub fn local_cache_len(&self) -> usize {
        self.local_blob_cache.borrow().len()
    }

    fn maybe_cache_insert(&self, hash: Blake3Hash, bytes: &[u8]) {
        match self.shared.options().configured_cache_mode() {
            CacheMode::Disabled => {}
            CacheMode::Entries { max_entries } => {
                let mut cache = self.local_blob_cache.borrow_mut();
                if cache.len() >= max_entries
                    && let Some(first_key) = cache.keys().next().copied()
                {
                    cache.remove(&first_key);
                }
                cache.insert(hash, bytes.to_vec());
            }
        }
    }

    fn policy_denies(&self, relative_path: &str) -> bool {
        #[cfg(all(feature = "native", not(target_arch = "wasm32")))]
        {
            self.shared.inner.policy_matcher.as_ref().is_some_and(|matcher| {
                matcher.matched_path_or_any_parents(relative_path, false).is_ignore()
            })
        }

        #[cfg(any(not(feature = "native"), target_arch = "wasm32"))]
        {
            let _ = relative_path;
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;

    #[test]
    fn open_options_builder_applies_values() {
        let opts = OpenOptions::new()
            .trust_mode(TrustMode::Strict)
            .cache_mode(CacheMode::Entries { max_entries: 8 })
            .policy_mode(PolicyMode::Bypass);

        assert_eq!(opts.configured_trust_mode(), TrustMode::Strict);
        assert_eq!(opts.configured_cache_mode(), CacheMode::Entries { max_entries: 8 });
        assert_eq!(opts.configured_policy_mode(), PolicyMode::Bypass);
    }

    #[test]
    fn shared_repository_clone_keeps_root() {
        let dir = tempdir().expect("tempdir must be created");
        let shared = SharedRepository::open(dir.path()).expect("shared open must succeed");
        let cloned = shared.clone();

        assert_eq!(shared.root(), cloned.root());
        assert_eq!(shared.options(), cloned.options());
    }

    #[test]
    fn thread_local_blob_cache_roundtrip() {
        let dir = tempdir().expect("tempdir must be created");
        let repo = Repository::open_with_options(
            dir.path(),
            OpenOptions::new().cache_mode(CacheMode::Entries { max_entries: 16 }),
        )
        .expect("open with options must succeed");

        let hash = repo.write_blob(b"capstone").expect("blob write must succeed");
        let first = repo.read_blob(&hash).expect("first read must succeed");
        let second = repo.read_blob(&hash).expect("second read must succeed");

        assert_eq!(first, b"capstone");
        assert_eq!(second, b"capstone");
        assert_eq!(repo.local_cache_len(), 1);
    }

    #[test]
    fn zero_cache_entries_is_rejected() {
        let dir = tempdir().expect("tempdir must be created");
        let err = SharedRepository::open_with_options(
            dir.path(),
            OpenOptions::new().cache_mode(CacheMode::Entries { max_entries: 0 }),
        )
        .err()
        .expect("zero-sized cache must fail");

        assert!(matches!(err, ArcError::InvalidOpenOptions { .. }));
    }

    #[cfg(not(feature = "native"))]
    #[test]
    fn enforce_policy_requires_native_feature() {
        let dir = tempdir().expect("tempdir must be created");
        let err = SharedRepository::open_with_options(
            dir.path(),
            OpenOptions::new().policy_mode(PolicyMode::Enforce),
        )
        .err()
        .expect("policy enforce should fail without native feature");

        assert!(matches!(err, ArcError::InvalidOpenOptions { .. }));
    }

    #[test]
    fn set_frontier_roundtrip() {
        let dir = tempdir().expect("tempdir must be created");
        let shared = SharedRepository::open(dir.path()).expect("shared open must succeed");

        let hashes: Vec<Blake3Hash> = vec![[1u8; 32], [2u8; 32], [0xFF; 32]];
        shared.set_frontier(hashes.clone());

        let got = shared.frontier();
        assert_eq!(*got, hashes);
    }

    #[test]
    fn frontier_default_is_empty() {
        let dir = tempdir().expect("tempdir must be created");
        let shared = SharedRepository::open(dir.path()).expect("shared open must succeed");

        let got = shared.frontier();
        assert!(got.is_empty());
        assert_eq!(got.len(), 0);
    }

    #[test]
    fn repository_shared_accessor_root_matches() {
        let dir = tempdir().expect("tempdir must be created");
        let repo = Repository::open(dir.path()).expect("repository open must succeed");

        assert_eq!(repo.shared().root(), dir.path());
    }

    #[test]
    fn repository_open_convenience() {
        let dir = tempdir().expect("tempdir must be created");
        let repo = Repository::open(dir.path()).expect("convenience open must succeed");

        assert_eq!(repo.shared().root(), dir.path());
        assert_eq!(repo.local_cache_len(), 0);
    }

    #[test]
    fn arc_error_display_variants() {
        let io_err = ArcError::Io(std::io::Error::other("boom"));
        assert_eq!(format!("{}", io_err), "io error: boom");

        let store_err = ArcError::Store(StoreError::Io(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "gone",
        )));
        assert_eq!(format!("{}", store_err), "store error: I/O error: gone");

        let cas_err = ArcError::Cas(arc_store_cas::cas::CasError::ChecksumMismatch);
        assert_eq!(
            format!("{}", cas_err),
            "cas error: checksum mismatch while reading content-addressed object"
        );

        let invalid = ArcError::InvalidOpenOptions { reason: "max_entries must be > 0".into() };
        assert_eq!(format!("{}", invalid), "invalid open options: max_entries must be > 0");

        let denied = ArcError::PolicyDenied { path: "secret.rs".into() };
        assert_eq!(format!("{}", denied), "policy denied write for path: secret.rs");
    }

    #[test]
    fn open_options_default_values() {
        let opts = OpenOptions::default();
        assert_eq!(opts.configured_trust_mode(), TrustMode::Balanced);
        assert_eq!(opts.configured_cache_mode(), CacheMode::Entries { max_entries: 256 });
        assert_eq!(opts.configured_policy_mode(), PolicyMode::Bypass);
    }

    #[test]
    fn cache_disabled_does_not_store_locally() {
        let dir = tempdir().expect("tempdir must be created");
        let repo = Repository::open_with_options(
            dir.path(),
            OpenOptions::new().cache_mode(CacheMode::Disabled),
        )
        .expect("open with disabled cache must succeed");

        let hash = repo.write_blob(b"no-cache").expect("blob write must succeed");
        let _ = repo.read_blob(&hash).expect("blob read must succeed");

        assert_eq!(repo.local_cache_len(), 0);
    }

    #[test]
    fn cache_entries_evicts_when_full() {
        let dir = tempdir().expect("tempdir must be created");
        let repo = Repository::open_with_options(
            dir.path(),
            OpenOptions::new().cache_mode(CacheMode::Entries { max_entries: 1 }),
        )
        .expect("open with max_entries=1 must succeed");

        let _h1 = repo.write_blob(b"first").expect("first blob write must succeed");
        assert_eq!(repo.local_cache_len(), 1);

        let _h2 = repo.write_blob(b"second").expect("second blob write must succeed");
        assert_eq!(repo.local_cache_len(), 1);
    }
}
