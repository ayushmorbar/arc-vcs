use std::{
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};

use arc_store_types::newtypes::SnapshotId;
use dashmap::DashMap;
use serde::{Deserialize, Serialize};

use crate::tempfile as temp_registry;

/// One captured input artifact that fed the synthesized architecture decision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SynthArtifact {
    /// Repository-relative source path.
    pub path: String,
    /// BLAKE3 hash of file bytes at capture time.
    pub content_hash: [u8; 32],
    /// Byte size at capture time.
    pub byte_len: u64,
}

/// Immutable, content-addressed synthesis snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SynthesisSnapshot {
    /// Content-derived identifier.
    pub id: SnapshotId,
    /// Source system label (e.g. `jj-main`).
    pub source: String,
    /// Unix timestamp when snapshot was created.
    pub created_at_unix: u64,
    /// Artifact list included in this snapshot.
    pub artifacts: Vec<SynthArtifact>,
}

/// Cache refresh mode for synthesis snapshot loading.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnapshotRefresh {
    /// Use cached value when present.
    PreferCached,
    /// Bypass cache and refresh from disk.
    ForceRefresh,
}

/// Process-shared cache for synthesis snapshots.
#[derive(Debug, Clone, Default)]
pub struct SnapshotCache {
    entries: Arc<DashMap<SnapshotId, Arc<SynthesisSnapshot>>>,
}

impl SnapshotCache {
    /// Create a new empty snapshot cache.
    pub fn new() -> Self {
        Self::default()
    }

    /// Remove one cached snapshot entry.
    pub fn invalidate(&self, id: SnapshotId) {
        let _ = self.entries.remove(&id);
    }

    /// Force-clear all cached snapshots.
    pub fn clear(&self) {
        self.entries.clear();
    }

    fn get(&self, id: SnapshotId) -> Option<Arc<SynthesisSnapshot>> {
        self.entries.get(&id).map(|value| Arc::clone(value.value()))
    }

    fn put(&self, snapshot: Arc<SynthesisSnapshot>) {
        self.entries.insert(snapshot.id, snapshot);
    }
}

/// Load a synthesis snapshot with optional cache bypass.
pub fn load_with_cache(
    shared_root: &Path,
    id: SnapshotId,
    cache: &SnapshotCache,
    mode: SnapshotRefresh,
) -> anyhow::Result<Arc<SynthesisSnapshot>> {
    if mode == SnapshotRefresh::PreferCached
        && let Some(hit) = cache.get(id)
    {
        return Ok(hit);
    }

    let loaded = Arc::new(SynthesisSnapshot::load(shared_root, id)?);
    cache.put(Arc::clone(&loaded));
    Ok(loaded)
}

impl SynthesisSnapshot {
    /// Build a snapshot from source files, fail-fast if any file cannot be read.
    pub fn capture(
        root: &Path,
        source: impl Into<String>,
        files: &[PathBuf],
    ) -> anyhow::Result<Self> {
        let source = source.into();
        let mut artifacts: Vec<SynthArtifact> =
            files.iter().map(|p| build_artifact(root, p)).collect::<anyhow::Result<Vec<_>>>()?;
        artifacts.sort_by(|a, b| a.path.cmp(&b.path));

        let created_at_unix = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let id = compute_snapshot_id(&source, &artifacts)?;

        Ok(Self { id, source, created_at_unix, artifacts })
    }

    /// Persist this snapshot atomically at `<shared_root>/.arc/synthesis/<prefix>/<suffix>.bin`.
    ///
    /// Idempotent: if the target already exists, no write occurs.
    pub fn persist(&self, shared_root: &Path) -> anyhow::Result<()> {
        let path = snapshot_path(shared_root, self.id);
        if path.exists() {
            return Ok(());
        }

        let parent =
            path.parent().ok_or_else(|| anyhow::anyhow!("invalid snapshot path without parent"))?;
        fs::create_dir_all(parent)?;

        let bytes = bincode::serialize(self)
            .map_err(|e| anyhow::anyhow!("failed to serialize synthesis snapshot: {e}"))?;

        #[cfg(windows)]
        {
            // Windows can deny directory/file sync handles in temp dirs.
            // Keep atomic replacement semantics using temp-file + rename,
            // but avoid strict sync barriers that fail in some environments.
            let tmp_name = format!("{}.tmp-{}", self.id.to_hex(), std::process::id());
            let tmp_path = parent.join(tmp_name);
            let temp_id = temp_registry::register(tmp_path.clone());
            fs::write(&tmp_path, &bytes)?;

            match fs::rename(&tmp_path, &path) {
                Ok(()) => {
                    temp_registry::deregister(temp_id);
                    Ok(())
                }
                Err(e) => {
                    temp_registry::deregister(temp_id);
                    let _ = fs::remove_file(&tmp_path);
                    if path.exists() {
                        Ok(())
                    } else {
                        Err(anyhow::anyhow!(
                            "atomic rename failed for synthesis snapshot {}: {e}",
                            self.id
                        ))
                    }
                }
            }
        }

        #[cfg(not(windows))]
        {
            let tmp_name = format!("{}.tmp-{}", self.id.to_hex(), std::process::id());
            let tmp_path = parent.join(tmp_name);
            let temp_id = temp_registry::register(tmp_path.clone());
            fs::write(&tmp_path, &bytes)?;
            // Durability barrier 1: force temp-file data to stable storage.
            fs::File::open(&tmp_path)?.sync_all()?;

            match fs::rename(&tmp_path, &path) {
                Ok(()) => {
                    temp_registry::deregister(temp_id);
                    // Durability barrier 2: force directory entry updates.
                    sync_directory(parent)?;
                    Ok(())
                }
                Err(e) => {
                    temp_registry::deregister(temp_id);
                    let _ = fs::remove_file(&tmp_path);
                    if path.exists() {
                        Ok(())
                    } else {
                        Err(anyhow::anyhow!(
                            "atomic rename failed for synthesis snapshot {}: {e}",
                            self.id
                        ))
                    }
                }
            }
        }
    }

    /// Load a previously persisted synthesis snapshot by id.
    pub fn load(shared_root: &Path, id: SnapshotId) -> anyhow::Result<Self> {
        let path = snapshot_path(shared_root, id);
        let bytes = fs::read(&path)
            .map_err(|e| anyhow::anyhow!("failed to read synthesis snapshot {}: {e}", id))?;
        let snapshot: SynthesisSnapshot = bincode::deserialize(&bytes)
            .map_err(|e| anyhow::anyhow!("failed to deserialize synthesis snapshot {}: {e}", id))?;
        Ok(snapshot)
    }
}

/// List all synthesis snapshot IDs, sorted ascending by hex.
pub fn list_snapshot_ids(shared_root: &Path) -> anyhow::Result<Vec<SnapshotId>> {
    let root = shared_root.join(".arc").join("synthesis");
    if !root.exists() {
        return Ok(Vec::new());
    }

    let mut ids = Vec::new();
    for prefix in fs::read_dir(&root)? {
        let prefix = match prefix {
            Ok(v) => v,
            Err(_) => continue,
        };
        if !prefix.path().is_dir() {
            continue;
        }
        let prefix_name = prefix.file_name().to_string_lossy().to_string();
        if prefix_name.len() != 2 || !prefix_name.bytes().all(|b| b.is_ascii_hexdigit()) {
            continue;
        }

        for entry in fs::read_dir(prefix.path())? {
            let entry = match entry {
                Ok(v) => v,
                Err(_) => continue,
            };
            if !entry.path().is_file() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().to_string();
            if !name.ends_with(".bin") {
                continue;
            }
            let suffix = name.trim_end_matches(".bin");
            if suffix.len() != 62 || !suffix.bytes().all(|b| b.is_ascii_hexdigit()) {
                continue;
            }

            let hex = format!("{prefix_name}{suffix}");
            if let Ok(id) = SnapshotId::from_hex(&hex) {
                ids.push(id);
            }
        }
    }

    ids.sort_by_key(|id| id.to_hex());
    Ok(ids)
}

fn build_artifact(root: &Path, input: &Path) -> anyhow::Result<SynthArtifact> {
    let full_path = if input.is_absolute() { input.to_path_buf() } else { root.join(input) };
    let bytes = fs::read(&full_path).map_err(|e| {
        anyhow::anyhow!("failed to read synthesis input '{}': {e}", full_path.display())
    })?;

    let rel = full_path
        .strip_prefix(root)
        .map(|p| p.to_string_lossy().replace('\\', "/"))
        .unwrap_or_else(|_| full_path.to_string_lossy().replace('\\', "/"));

    Ok(SynthArtifact {
        path: rel,
        content_hash: *blake3::hash(&bytes).as_bytes(),
        byte_len: bytes.len() as u64,
    })
}

fn compute_snapshot_id(source: &str, artifacts: &[SynthArtifact]) -> anyhow::Result<SnapshotId> {
    // ID is content-addressed and deterministic: timestamp is intentionally excluded.
    let payload = bincode::serialize(&(source, artifacts))
        .map_err(|e| anyhow::anyhow!("failed to serialize snapshot payload: {e}"))?;
    Ok(SnapshotId(*blake3::hash(&payload).as_bytes()))
}

fn snapshot_path(shared_root: &Path, id: SnapshotId) -> PathBuf {
    let hex = id.to_hex();
    shared_root.join(".arc").join("synthesis").join(&hex[..2]).join(format!("{}.bin", &hex[2..]))
}

#[cfg(not(windows))]
fn sync_directory(path: &Path) -> anyhow::Result<()> {
    #[cfg(unix)]
    {
        fs::File::open(path)?.sync_all()?;
        Ok(())
    }

    #[cfg(not(unix))]
    {
        // Best effort on uncommon non-Windows targets where directory sync
        // semantics are unavailable via std APIs.
        let _ = path;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capture_is_content_addressed() {
        let tmp = tempfile::tempdir().expect("tempdir must be creatable");
        let root = tmp.path();
        let file = root.join("a.txt");
        fs::write(&file, b"hello").expect("test file write must succeed");

        let first = SynthesisSnapshot::capture(root, "jj-main", &[PathBuf::from("a.txt")])
            .expect("first capture must succeed");
        std::thread::sleep(std::time::Duration::from_millis(2));
        let second = SynthesisSnapshot::capture(root, "jj-main", &[PathBuf::from("a.txt")])
            .expect("second capture must succeed");

        assert_eq!(first.id, second.id, "snapshot id must be content-addressed");
    }

    #[test]
    fn persist_and_load_roundtrip() {
        let tmp = tempfile::tempdir().expect("tempdir must be creatable");
        let root = tmp.path();
        fs::create_dir_all(root.join(".arc")).expect("arc dir create must succeed");
        fs::write(root.join("x.md"), b"docs").expect("file write must succeed");

        let snap = SynthesisSnapshot::capture(root, "jj-main", &[PathBuf::from("x.md")])
            .expect("capture must succeed");
        snap.persist(root).expect("persist must succeed");
        let loaded = SynthesisSnapshot::load(root, snap.id).expect("load must succeed");

        assert_eq!(loaded.id, snap.id);
        assert_eq!(loaded.artifacts.len(), 1);
        assert_eq!(loaded.artifacts[0].path, "x.md");
    }

    #[test]
    fn list_snapshot_ids_sorted_and_filtered() {
        let tmp = tempfile::tempdir().expect("tempdir must be creatable");
        let root = tmp.path();
        fs::create_dir_all(root.join(".arc").join("synthesis").join("zz"))
            .expect("junk dir create must succeed");
        fs::write(root.join(".arc").join("synthesis").join("zz").join("junk.bin"), b"x")
            .expect("junk file write must succeed");

        fs::write(root.join("one.txt"), b"one").expect("file write must succeed");
        fs::write(root.join("two.txt"), b"two").expect("file write must succeed");

        let s1 = SynthesisSnapshot::capture(root, "jj-main", &[PathBuf::from("one.txt")])
            .expect("capture one must succeed");
        let s2 = SynthesisSnapshot::capture(root, "jj-main", &[PathBuf::from("two.txt")])
            .expect("capture two must succeed");
        s1.persist(root).expect("persist one must succeed");
        s2.persist(root).expect("persist two must succeed");

        let listed = list_snapshot_ids(root).expect("listing must succeed");
        assert_eq!(listed.len(), 2);
        assert!(listed[0].to_hex() <= listed[1].to_hex());
    }

    #[test]
    fn load_with_cache_force_refresh_replaces_entry() {
        let tmp = tempfile::tempdir().expect("tempdir must be creatable");
        let root = tmp.path();
        fs::write(root.join("source.txt"), b"v1").expect("write must succeed");

        let first = SynthesisSnapshot::capture(root, "src", &[PathBuf::from("source.txt")])
            .expect("capture v1 must succeed");
        first.persist(root).expect("persist v1 must succeed");

        let cache = SnapshotCache::new();
        let loaded_v1 = load_with_cache(root, first.id, &cache, SnapshotRefresh::PreferCached)
            .expect("cached load must succeed");
        assert_eq!(loaded_v1.id, first.id);
        assert_eq!(loaded_v1.source, "src");

        let mut poisoned = (*loaded_v1).clone();
        poisoned.source = "stale-cache".to_string();
        cache.entries.insert(first.id, Arc::new(poisoned));

        let stale = load_with_cache(root, first.id, &cache, SnapshotRefresh::PreferCached)
            .expect("prefer-cached load must succeed");
        assert_eq!(stale.source, "stale-cache");

        let refreshed = load_with_cache(root, first.id, &cache, SnapshotRefresh::ForceRefresh)
            .expect("force refresh must succeed");
        assert_eq!(refreshed.id, first.id);
        assert_eq!(refreshed.source, "src");
    }

    #[test]
    fn cache_invalidate_removes_entry() {
        let tmp = tempfile::tempdir().expect("tempdir must be creatable");
        let root = tmp.path();
        fs::write(root.join("f.txt"), b"data").expect("write must succeed");
        let snap = SynthesisSnapshot::capture(root, "src", &[PathBuf::from("f.txt")])
            .expect("capture must succeed");
        snap.persist(root).expect("persist must succeed");
        let cache = SnapshotCache::new();
        let _ = load_with_cache(root, snap.id, &cache, SnapshotRefresh::PreferCached);
        assert!(cache.get(snap.id).is_some());
        cache.invalidate(snap.id);
        assert!(cache.get(snap.id).is_none());
    }

    #[test]
    fn cache_clear_removes_all_entries() {
        let tmp = tempfile::tempdir().expect("tempdir must be creatable");
        let root = tmp.path();
        fs::write(root.join("a.txt"), b"aaa").expect("write must succeed");
        fs::write(root.join("b.txt"), b"bbb").expect("write must succeed");
        let s1 = SynthesisSnapshot::capture(root, "src", &[PathBuf::from("a.txt")])
            .expect("capture must succeed");
        let s2 = SynthesisSnapshot::capture(root, "src", &[PathBuf::from("b.txt")])
            .expect("capture must succeed");
        let cache = SnapshotCache::new();
        let _ = load_with_cache(root, s1.id, &cache, SnapshotRefresh::PreferCached);
        let _ = load_with_cache(root, s2.id, &cache, SnapshotRefresh::PreferCached);
        cache.clear();
        assert!(cache.get(s1.id).is_none());
        assert!(cache.get(s2.id).is_none());
    }

    #[test]
    fn list_snapshot_ids_empty_when_no_synthesis_dir() {
        let tmp = tempfile::tempdir().expect("tempdir must be creatable");
        let listed = list_snapshot_ids(tmp.path()).expect("listing must succeed");
        assert!(listed.is_empty());
    }

    #[test]
    fn list_snapshot_ids_ignores_invalid_prefixes() {
        let tmp = tempfile::tempdir().expect("tempdir must be creatable");
        let root = tmp.path();
        let syn_root = root.join(".arc").join("synthesis");
        fs::create_dir_all(syn_root.join("zz")).expect("dir create must succeed");
        fs::write(syn_root.join("zz").join("junk.bin"), b"x").expect("write must succeed");
        fs::create_dir_all(syn_root.join("a")).expect("dir create must succeed");
        fs::write(syn_root.join("a").join("junk.bin"), b"x").expect("write must succeed");
        let listed = list_snapshot_ids(root).expect("listing must succeed");
        assert!(listed.is_empty());
    }

    #[test]
    fn list_snapshot_ids_ignores_non_bin_files() {
        let tmp = tempfile::tempdir().expect("tempdir must be creatable");
        let root = tmp.path();
        let prefix_dir = root.join(".arc").join("synthesis").join("ab");
        fs::create_dir_all(&prefix_dir).expect("dir create must succeed");
        fs::write(prefix_dir.join("notbin.txt"), b"x").expect("write must succeed");
        let listed = list_snapshot_ids(root).expect("listing must succeed");
        assert!(listed.is_empty());
    }

    #[test]
    fn list_snapshot_ids_ignores_wrong_suffix_length() {
        let tmp = tempfile::tempdir().expect("tempdir must be creatable");
        let root = tmp.path();
        let prefix_dir = root.join(".arc").join("synthesis").join("ab");
        fs::create_dir_all(&prefix_dir).expect("dir create must succeed");
        fs::write(prefix_dir.join("deadbeef.bin"), b"x").expect("write must succeed");
        let listed = list_snapshot_ids(root).expect("listing must succeed");
        assert!(listed.is_empty());
    }

    #[test]
    fn load_with_cache_prefer_cached_falls_through_when_empty() {
        let tmp = tempfile::tempdir().expect("tempdir must be creatable");
        let root = tmp.path();
        fs::write(root.join("g.txt"), b"gray").expect("write must succeed");
        let snap = SynthesisSnapshot::capture(root, "src", &[PathBuf::from("g.txt")])
            .expect("capture must succeed");
        snap.persist(root).expect("persist must succeed");
        let cache = SnapshotCache::new();
        let loaded = load_with_cache(root, snap.id, &cache, SnapshotRefresh::PreferCached)
            .expect("load must succeed");
        assert_eq!(loaded.id, snap.id);
        assert!(cache.get(snap.id).is_some());
    }

    #[test]
    fn persist_is_idempotent() {
        let tmp = tempfile::tempdir().expect("tempdir must be creatable");
        let root = tmp.path();
        fs::write(root.join("p.txt"), b"payload").expect("write must succeed");
        let snap = SynthesisSnapshot::capture(root, "src", &[PathBuf::from("p.txt")])
            .expect("capture must succeed");
        snap.persist(root).expect("first persist must succeed");
        let path = snapshot_path(root, snap.id);
        let meta1 = fs::metadata(&path).expect("file must exist");
        snap.persist(root).expect("second persist must be idempotent");
        let meta2 = fs::metadata(&path).expect("file must still exist");
        assert_eq!(meta1.modified().unwrap(), meta2.modified().unwrap());
    }

    #[test]
    fn capture_multiple_files_are_sorted() {
        let tmp = tempfile::tempdir().expect("tempdir must be creatable");
        let root = tmp.path();
        fs::write(root.join("z.txt"), b"zzz").expect("write must succeed");
        fs::write(root.join("a.txt"), b"aaa").expect("write must succeed");
        fs::write(root.join("m.txt"), b"mmm").expect("write must succeed");
        let snap = SynthesisSnapshot::capture(
            root,
            "src",
            &[PathBuf::from("z.txt"), PathBuf::from("a.txt"), PathBuf::from("m.txt")],
        )
        .expect("capture must succeed");
        assert_eq!(snap.artifacts.len(), 3);
        assert_eq!(snap.artifacts[0].path, "a.txt");
        assert_eq!(snap.artifacts[1].path, "m.txt");
        assert_eq!(snap.artifacts[2].path, "z.txt");
    }

    #[test]
    fn capture_missing_file_errors() {
        let tmp = tempfile::tempdir().expect("tempdir must be creatable");
        let root = tmp.path();
        let result = SynthesisSnapshot::capture(root, "src", &[PathBuf::from("nope.txt")]);
        assert!(result.is_err());
    }

    #[test]
    fn build_artifact_with_absolute_path() {
        let tmp = tempfile::tempdir().expect("tempdir must be creatable");
        let root = tmp.path();
        let file = root.join("abs.txt");
        fs::write(&file, b"absolute").expect("write must succeed");
        let snap = SynthesisSnapshot::capture(root, "src", std::slice::from_ref(&file))
            .expect("capture must succeed");
        assert_eq!(snap.artifacts.len(), 1);
        assert_eq!(snap.artifacts[0].byte_len, 8);
    }

    #[test]
    fn snapshot_path_structure() {
        let id = SnapshotId([0xAB; 32]);
        let path = snapshot_path(Path::new("/repo"), id);
        assert!(path.starts_with("/repo/.arc/synthesis/ab"));
        assert!(path.to_string_lossy().ends_with(".bin"));
    }

    #[test]
    fn compute_snapshot_id_deterministic() {
        let arts = vec![
            SynthArtifact { path: "a.rs".into(), content_hash: [1u8; 32], byte_len: 100 },
            SynthArtifact { path: "b.rs".into(), content_hash: [2u8; 32], byte_len: 200 },
        ];
        let id1 = compute_snapshot_id("test-src", &arts).expect("must succeed");
        let id2 = compute_snapshot_id("test-src", &arts).expect("must succeed");
        assert_eq!(id1, id2);
        let id3 = compute_snapshot_id("other-src", &arts).expect("must succeed");
        assert_ne!(id1, id3);
    }

    #[test]
    fn capture_empty_files_list() {
        let tmp = tempfile::tempdir().expect("tempdir must be creatable");
        let root = tmp.path();
        let snap = SynthesisSnapshot::capture(root, "src", &[]).expect("capture must succeed");
        assert!(snap.artifacts.is_empty());
    }

    #[test]
    fn list_snapshot_ids_nonexistent_prefix_dir_filtered() {
        let tmp = tempfile::tempdir().expect("tempdir must be creatable");
        let root = tmp.path();
        let syn_root = root.join(".arc").join("synthesis");
        fs::create_dir_all(&syn_root).expect("dir create must succeed");
        let listed = list_snapshot_ids(root).expect("listing must succeed");
        assert!(listed.is_empty());
    }
}
