use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use arc_algebra_types::Blake3Hash;
use serde::{Deserialize, Serialize};

use crate::StoreError;

/// A `View` is arc's replacement for a Git branch.
///
/// Instead of pointing to a single "tip" commit, a view tracks a
/// **set of heads** — changes that have no children within this view.
/// This naturally represents a partial order rather than forcing a
/// linear history.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct View {
    /// Human-readable name (e.g. `"main"`, `"feature/auth"`).
    pub name: String,
    /// The frontier of this view: changes with no dependents in scope.
    pub heads: HashSet<Blake3Hash>,
}

impl View {
    /// Create a new `View` with the given name and head set.
    pub fn new(name: impl Into<String>, heads: HashSet<Blake3Hash>) -> Self {
        Self {
            name: name.into(),
            heads,
        }
    }

    /// Persist this view to `.arc/views/{name}` using `bincode`.
    ///
    /// Uses an atomic rename pattern (write to `.tmp`, then rename) to
    /// prevent corruption when multiple AI agents write concurrently.
    pub fn save(&self, arc_root: impl AsRef<Path>) -> Result<(), StoreError> {
        let path = view_path(arc_root.as_ref(), &self.name);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let bytes = bincode::serialize(self)?;

        // Atomic write: tmp → rename prevents half-written files under
        // concurrent multi-agent access.
        let tmp_path = path.with_extension("tmp");
        fs::write(&tmp_path, &bytes)?;
        fs::rename(&tmp_path, &path)?;

        Ok(())
    }

    /// Load a view from `.arc/views/{name}`.
    pub fn load(arc_root: impl AsRef<Path>, name: &str) -> Result<Self, StoreError> {
        let path = view_path(arc_root.as_ref(), name);
        let bytes = fs::read(&path)?;
        let view: Self = bincode::deserialize(&bytes)?;
        Ok(view)
    }
}

/// Canonical path: `<arc_root>/.arc/views/<name>`
fn view_path(arc_root: &Path, name: &str) -> PathBuf {
    arc_root.join(".arc").join("views").join(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_view_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let heads = HashSet::from([[1u8; 32], [2u8; 32]]);
        let view = View::new("main", heads);

        view.save(dir.path()).unwrap();
        let loaded = View::load(dir.path(), "main").unwrap();

        assert_eq!(loaded, view);
    }

    /// A view with no heads (fresh branch) must survive a save/load cycle.
    #[test]
    fn test_view_empty_heads_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let view = View::new("empty-branch", HashSet::new());

        view.save(dir.path()).unwrap();
        let loaded = View::load(dir.path(), "empty-branch").unwrap();

        assert_eq!(loaded, view);
        assert!(
            loaded.heads.is_empty(),
            "empty-headed view must round-trip cleanly"
        );
    }

    /// Loading a non-existent view must return an error, not panic.
    #[test]
    fn test_view_load_nonexistent_returns_error() {
        let dir = tempfile::tempdir().unwrap();
        let result = View::load(dir.path(), "ghost-branch");
        assert!(
            result.is_err(),
            "loading a non-existent view must return an error"
        );
    }

    /// A view name containing a slash (nested branch) must round-trip correctly.
    #[test]
    fn test_view_nested_name_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let heads = HashSet::from([[7u8; 32]]);
        let view = View::new("feature/auth", heads);

        view.save(dir.path()).unwrap();
        let loaded = View::load(dir.path(), "feature/auth").unwrap();

        assert_eq!(loaded, view);
        assert_eq!(loaded.name, "feature/auth");
    }
}
