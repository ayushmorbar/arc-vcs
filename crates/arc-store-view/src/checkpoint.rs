//! Crash-consistent JSON checkpoint persistence helpers.
//!
//! These utilities use [`crate::lock::LockFile`] so checkpoint updates are
//! atomic and durable with the same guarantees used by view/oplog writes.

use std::fs;
use std::io::ErrorKind;
use std::path::Path;

use anyhow::{Context, Result};
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::lock::LockFile;

/// Load a JSON checkpoint if it exists.
pub fn load_json<T>(path: &Path) -> Result<Option<T>>
where
    T: DeserializeOwned,
{
    match fs::read_to_string(path) {
        Ok(raw) => {
            let value = serde_json::from_str::<T>(&raw).with_context(|| {
                format!(
                    "failed to parse checkpoint JSON '{}': {}",
                    path.display(),
                    raw
                )
            })?;
            Ok(Some(value))
        }
        Err(err) if err.kind() == ErrorKind::NotFound => Ok(None),
        Err(err) => {
            Err(err).with_context(|| format!("failed to read checkpoint '{}'", path.display()))
        }
    }
}

/// Persist a JSON checkpoint atomically.
pub fn save_json_atomic<T>(path: &Path, value: &T) -> Result<()>
where
    T: Serialize,
{
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| {
            format!("failed to create checkpoint parent '{}'", parent.display())
        })?;
    }

    let payload = serde_json::to_vec_pretty(value)
        .with_context(|| format!("failed to serialize checkpoint '{}'", path.display()))?;
    let mut lock = LockFile::acquire_for_update(path)
        .with_context(|| format!("failed to acquire checkpoint lock '{}'", path.display()))?;
    lock.write_all(&payload)
        .with_context(|| format!("failed to write checkpoint '{}'", path.display()))?;
    lock.commit()
        .with_context(|| format!("failed to commit checkpoint '{}'", path.display()))?;
    Ok(())
}

/// Remove a checkpoint file if it exists.
pub fn remove(path: &Path) -> Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == ErrorKind::NotFound => Ok(()),
        Err(err) => {
            Err(err).with_context(|| format!("failed to remove checkpoint '{}'", path.display()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
    struct Sample {
        id: u32,
        label: String,
    }

    #[test]
    fn save_load_remove_roundtrip() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("checkpoint.json");

        let sample = Sample {
            id: 7,
            label: "restack".to_string(),
        };

        save_json_atomic(&path, &sample).expect("save checkpoint");
        let loaded = load_json::<Sample>(&path)
            .expect("load checkpoint")
            .expect("checkpoint exists");
        assert_eq!(loaded, sample);

        remove(&path).expect("remove checkpoint");
        assert!(
            load_json::<Sample>(&path)
                .expect("load after remove")
                .is_none()
        );
    }
}
