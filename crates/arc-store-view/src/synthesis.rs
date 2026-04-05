use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use arc_store_types::newtypes::SnapshotId;

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

impl SynthesisSnapshot {
    /// Build a snapshot from source files, fail-fast if any file cannot be read.
    pub fn capture(
        root: &Path,
        source: impl Into<String>,
        files: &[PathBuf],
    ) -> anyhow::Result<Self> {
        let source = source.into();
        let mut artifacts: Vec<SynthArtifact> = files
            .iter()
            .map(|p| build_artifact(root, p))
            .collect::<anyhow::Result<Vec<_>>>()?;
        artifacts.sort_by(|a, b| a.path.cmp(&b.path));

        let created_at_unix = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let id = compute_snapshot_id(&source, &artifacts)?;

        Ok(Self {
            id,
            source,
            created_at_unix,
            artifacts,
        })
    }

    /// Persist this snapshot atomically at `<shared_root>/.arc/synthesis/<prefix>/<suffix>.bin`.
    ///
    /// Idempotent: if the target already exists, no write occurs.
    pub fn persist(&self, shared_root: &Path) -> anyhow::Result<()> {
        let path = snapshot_path(shared_root, self.id);
        if path.exists() {
            return Ok(());
        }

        let parent = path
            .parent()
            .ok_or_else(|| anyhow::anyhow!("invalid snapshot path without parent"))?;
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
            fs::write(&tmp_path, &bytes)?;

            match fs::rename(&tmp_path, &path) {
                Ok(()) => Ok(()),
                Err(e) => {
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
            fs::write(&tmp_path, &bytes)?;
            // Durability barrier 1: force temp-file data to stable storage.
            fs::File::open(&tmp_path)?.sync_all()?;

            match fs::rename(&tmp_path, &path) {
                Ok(()) => {
                    // Durability barrier 2: force directory entry updates.
                    sync_directory(parent)?;
                    Ok(())
                }
                Err(e) => {
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
    let full_path = if input.is_absolute() {
        input.to_path_buf()
    } else {
        root.join(input)
    };
    let bytes = fs::read(&full_path).map_err(|e| {
        anyhow::anyhow!(
            "failed to read synthesis input '{}': {e}",
            full_path.display()
        )
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
    shared_root
        .join(".arc")
        .join("synthesis")
        .join(&hex[..2])
        .join(format!("{}.bin", &hex[2..]))
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
        fs::write(
            root.join(".arc")
                .join("synthesis")
                .join("zz")
                .join("junk.bin"),
            b"x",
        )
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
}
