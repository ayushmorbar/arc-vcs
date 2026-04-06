use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use anyhow::Context as _;

fn fixture_lock() -> &'static Mutex<()> {
    static FIXTURE_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    FIXTURE_LOCK.get_or_init(|| Mutex::new(()))
}

/// Materialization mode for fixtures.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FixtureMode {
    /// Cache fixture content in a deterministic location.
    Cached,
    /// Materialize a writable copy in a temporary directory.
    WritableCopy,
}

/// Options controlling fixture materialization.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FixtureOptions {
    /// Logical fixture name included in the cache key.
    pub name: String,
    /// Version string included in the cache key.
    pub version: String,
    /// Materialization mode.
    pub mode: FixtureMode,
}

impl FixtureOptions {
    /// Construct default options for a named fixture.
    #[must_use]
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            version: "v1".to_string(),
            mode: FixtureMode::Cached,
        }
    }

    /// Override version for cache key invalidation.
    #[must_use]
    pub fn with_version(mut self, version: impl Into<String>) -> Self {
        self.version = version.into();
        self
    }

    /// Override materialization mode.
    #[must_use]
    pub fn with_mode(mut self, mode: FixtureMode) -> Self {
        self.mode = mode;
        self
    }
}

/// Deterministic fixture materializer and cache manager.
pub struct FixtureOrchestrator {
    cache_root: PathBuf,
}

impl FixtureOrchestrator {
    /// Create an orchestrator rooted at `cache_root`.
    #[must_use]
    pub fn new(cache_root: PathBuf) -> Self {
        Self { cache_root }
    }

    /// Compute deterministic cache key for a fixture source and options.
    pub fn cache_key(&self, source: &Path, options: &FixtureOptions) -> anyhow::Result<String> {
        let canonical_source = source
            .canonicalize()
            .with_context(|| format!("failed to canonicalize fixture source {}", source.display()))?;
        let mode = match options.mode {
            FixtureMode::Cached => "cached",
            FixtureMode::WritableCopy => "writable-copy",
        };

        let mut hasher = blake3::Hasher::new();
        hasher.update(canonical_source.to_string_lossy().as_bytes());
        hasher.update(&[0]);
        hasher.update(options.name.as_bytes());
        hasher.update(&[0]);
        hasher.update(options.version.as_bytes());
        hasher.update(&[0]);
        hasher.update(mode.as_bytes());
        Ok(hasher.finalize().to_hex().to_string())
    }

    /// Materialize fixture content according to options.
    pub fn materialize(&self, source: &Path, options: &FixtureOptions) -> anyhow::Result<PathBuf> {
        let _guard = fixture_lock().lock().expect("fixture lock poisoned");
        let (cache_path, _) = self.ensure_cached(source, options)?;

        match options.mode {
            FixtureMode::Cached => Ok(cache_path),
            FixtureMode::WritableCopy => {
                let temp = tempfile::Builder::new()
                    .prefix("arc-fixture-")
                    .tempdir()
                    .context("failed to create temporary writable fixture directory")?;
                copy_directory(&cache_path, temp.path())?;
                Ok(temp.keep())
            }
        }
    }

    /// Materialize a cached fixture and run post-processing callback on first creation.
    pub fn materialize_with_post<F>(
        &self,
        source: &Path,
        options: &FixtureOptions,
        post: F,
    ) -> anyhow::Result<PathBuf>
    where
        F: FnOnce(&Path) -> anyhow::Result<()>,
    {
        let _guard = fixture_lock().lock().expect("fixture lock poisoned");
        let (cache_path, created_by_this_call) = self.ensure_cached(source, options)?;
        if created_by_this_call {
            if let Err(error) = post(&cache_path) {
                if let Err(remove_error) = std::fs::remove_dir_all(&cache_path) {
                    return Err(anyhow::anyhow!(
                        "post-processing failed for fixture cache {} and cache invalidation failed: {}",
                        cache_path.display(),
                        remove_error
                    ))
                    .context(error.to_string());
                }
                return Err(error).with_context(|| {
                    format!(
                        "post-processing failed for fixture cache {}",
                        cache_path.display()
                    )
                });
            }
        }
        match options.mode {
            FixtureMode::Cached => Ok(cache_path),
            FixtureMode::WritableCopy => {
                let temp = tempfile::Builder::new()
                    .prefix("arc-fixture-")
                    .tempdir()
                    .context("failed to create temporary writable fixture directory")?;
                copy_directory(&cache_path, temp.path())?;
                Ok(temp.keep())
            }
        }
    }

    fn ensure_cached(
        &self,
        source: &Path,
        options: &FixtureOptions,
    ) -> anyhow::Result<(PathBuf, bool)> {
        let key = self.cache_key(source, options)?;
        let cache_path = self.cache_root.join("fixtures").join(&key);
        if cache_path.exists() {
            return Ok((cache_path, false));
        }

        let cache_parent = cache_path
            .parent()
            .context("cache path must have a parent directory")?;
        std::fs::create_dir_all(cache_parent).with_context(|| {
            format!(
                "failed to create fixture cache parent {}",
                cache_parent.display()
            )
        })?;

        let staging = tempfile::Builder::new()
            .prefix("arc-fixture-stage-")
            .tempdir_in(cache_parent)
            .context("failed to create fixture staging directory")?;
        let staged_payload = staging.path().join("payload");
        copy_directory(source, &staged_payload)?;

        match std::fs::rename(&staged_payload, &cache_path) {
            Ok(()) => Ok((cache_path, true)),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                Ok((cache_path, false))
            }
            Err(error) => Err(error).with_context(|| {
                format!(
                    "failed to promote fixture cache from {} to {}",
                    staged_payload.display(),
                    cache_path.display()
                )
            }),
        }
    }
}

fn copy_directory(source: &Path, destination: &Path) -> anyhow::Result<()> {
    if !source.is_dir() {
        anyhow::bail!("fixture source is not a directory: {}", source.display());
    }
    std::fs::create_dir_all(destination)
        .with_context(|| format!("failed to create destination {}", destination.display()))?;

    for entry in std::fs::read_dir(source)
        .with_context(|| format!("failed to read source directory {}", source.display()))?
    {
        let entry = entry.with_context(|| format!("failed to read entry under {}", source.display()))?;
        let entry_path = entry.path();
        let dest_path = destination.join(entry.file_name());
        let metadata = entry
            .metadata()
            .with_context(|| format!("failed to read metadata for {}", entry_path.display()))?;

        if metadata.is_dir() {
            copy_directory(&entry_path, &dest_path)?;
        } else if metadata.is_file() {
            std::fs::copy(&entry_path, &dest_path).with_context(|| {
                format!(
                    "failed to copy fixture file {} to {}",
                    entry_path.display(),
                    dest_path.display()
                )
            })?;
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::{FixtureMode, FixtureOptions, FixtureOrchestrator};

    fn create_source_fixture() -> tempfile::TempDir {
        let source = tempfile::tempdir().expect("tempdir");
        std::fs::write(source.path().join("data.txt"), "fixture-data").expect("write source");
        source
    }

    #[test]
    fn cache_key_is_deterministic_for_same_input() {
        let cache_root = tempfile::tempdir().expect("cache root");
        let source = create_source_fixture();
        let orchestrator = FixtureOrchestrator::new(cache_root.path().to_path_buf());
        let options = FixtureOptions::new("demo").with_version("v1");

        let first = orchestrator
            .cache_key(source.path(), &options)
            .expect("first key");
        let second = orchestrator
            .cache_key(source.path(), &options)
            .expect("second key");
        assert_eq!(first, second);
    }

    #[test]
    fn cache_key_changes_when_version_changes() {
        let cache_root = tempfile::tempdir().expect("cache root");
        let source = create_source_fixture();
        let orchestrator = FixtureOrchestrator::new(cache_root.path().to_path_buf());

        let v1 = orchestrator
            .cache_key(source.path(), &FixtureOptions::new("demo").with_version("v1"))
            .expect("v1 key");
        let v2 = orchestrator
            .cache_key(source.path(), &FixtureOptions::new("demo").with_version("v2"))
            .expect("v2 key");

        assert_ne!(v1, v2);
    }

    #[test]
    fn writable_copy_does_not_mutate_source() {
        let cache_root = tempfile::tempdir().expect("cache root");
        let source = create_source_fixture();
        let orchestrator = FixtureOrchestrator::new(cache_root.path().to_path_buf());
        let options = FixtureOptions::new("demo").with_mode(FixtureMode::WritableCopy);

        let writable_path = orchestrator
            .materialize(source.path(), &options)
            .expect("materialize writable copy");
        let writable_file = writable_path.join("data.txt");
        std::fs::write(&writable_file, "mutated").expect("mutate writable copy");

        let source_contents = std::fs::read_to_string(source.path().join("data.txt"))
            .expect("read source after mutation");
        assert_eq!(source_contents, "fixture-data");
    }

    #[test]
    fn post_processing_runs_once_per_cache_key() {
        let cache_root = tempfile::tempdir().expect("cache root");
        let source = create_source_fixture();
        let orchestrator = FixtureOrchestrator::new(cache_root.path().to_path_buf());
        let options = FixtureOptions::new("demo");

        let calls = Arc::new(Mutex::new(0usize));
        let first_calls = Arc::clone(&calls);
        orchestrator
            .materialize_with_post(source.path(), &options, move |_| {
                let mut count = first_calls.lock().expect("lock");
                *count += 1;
                Ok(())
            })
            .expect("first materialize");

        let second_calls = Arc::clone(&calls);
        orchestrator
            .materialize_with_post(source.path(), &options, move |_| {
                let mut count = second_calls.lock().expect("lock");
                *count += 1;
                Ok(())
            })
            .expect("second materialize");

        assert_eq!(*calls.lock().expect("lock"), 1);
    }
}
