use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use arc_algebra_types::Blake3Hash;
use serde::{Deserialize, Serialize};
use tracing::instrument;

use crate::StoreError;
use crate::lock::LockFile;

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
    /// Uses a lock-file write/commit protocol:
    ///
    /// 1. acquire `<view>.lock` exclusively,
    /// 2. write full payload into the lock file,
    /// 3. fsync and atomically publish into the view path.
    ///
    /// Drop-guard semantics ensure lock cleanup if the process panics or exits
    /// before commit.
    #[instrument(skip_all, fields(view = %self.name))]
    pub fn save(&self, arc_root: impl AsRef<Path>) -> Result<(), StoreError> {
        let path = view_path(arc_root.as_ref(), &self.name);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let bytes = bincode::serialize(self)?;

        let mut lock = LockFile::acquire_for_update(&path)?;
        lock.write_all(&bytes)?;
        lock.commit()?;

        Ok(())
    }

    /// Load a view from `.arc/views/{name}`.
    #[instrument(skip_all, fields(view = %name))]
    pub fn load(arc_root: impl AsRef<Path>, name: &str) -> Result<Self, StoreError> {
        let path = view_path(arc_root.as_ref(), name);
        let bytes = fs::read(&path)?;
        let view: Self = bincode::deserialize(&bytes)?;
        Ok(view)
    }
}

/// Tie-break policy when both sorted sources contain the same key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverlayPrecedence {
    /// Keep the item from the left input.
    Left,
    /// Keep the item from the right input.
    Right,
}

/// Merge two sorted streams while preferring one source on key collision.
pub fn merge_sorted_overlay<T, I, J, K, F>(
    left: I,
    right: J,
    key_fn: F,
    precedence: OverlayPrecedence,
) -> std::vec::IntoIter<T>
where
    I: IntoIterator<Item = T>,
    J: IntoIterator<Item = T>,
    F: Fn(&T) -> K,
    K: Ord,
{
    let mut left = left.into_iter().peekable();
    let mut right = right.into_iter().peekable();
    let mut out = Vec::new();

    loop {
        match (left.peek(), right.peek()) {
            (Some(l), Some(r)) => {
                let lk = key_fn(l);
                let rk = key_fn(r);
                match lk.cmp(&rk) {
                    std::cmp::Ordering::Less => {
                        if let Some(item) = left.next() {
                            out.push(item);
                        }
                    }
                    std::cmp::Ordering::Greater => {
                        if let Some(item) = right.next() {
                            out.push(item);
                        }
                    }
                    std::cmp::Ordering::Equal => {
                        let l_item = left.next();
                        let r_item = right.next();
                        match precedence {
                            OverlayPrecedence::Left => {
                                if let Some(item) = l_item {
                                    out.push(item);
                                }
                            }
                            OverlayPrecedence::Right => {
                                if let Some(item) = r_item {
                                    out.push(item);
                                }
                            }
                        }
                    }
                }
            }
            (Some(_), None) => {
                out.extend(left);
                break;
            }
            (None, Some(_)) => {
                out.extend(right);
                break;
            }
            (None, None) => break,
        }
    }

    out.into_iter()
}

/// Load persisted views and merge an in-memory overlay by name.
pub fn load_views_with_overlay(
    arc_root: impl AsRef<Path>,
    overlay: &[View],
    precedence: OverlayPrecedence,
) -> Result<Vec<View>, StoreError> {
    let mut persisted = load_all_views(arc_root.as_ref())?;
    persisted.sort_by(|a, b| a.name.cmp(&b.name));
    let mut sorted_overlay = overlay.to_vec();
    sorted_overlay.sort_by(|a, b| a.name.cmp(&b.name));

    Ok(merge_sorted_overlay(
        persisted,
        sorted_overlay,
        |view| view.name.clone(),
        precedence,
    )
    .collect())
}

fn load_all_views(arc_root: &Path) -> Result<Vec<View>, StoreError> {
    let root = arc_root.join(".arc").join("views");
    if !root.exists() {
        return Ok(Vec::new());
    }

    let mut paths = Vec::new();
    collect_view_paths(&root, &mut paths)?;

    let mut views = Vec::new();
    for path in paths {
        let bytes = fs::read(&path)?;
        let view: View = bincode::deserialize(&bytes)?;
        views.push(view);
    }

    Ok(views)
}

fn collect_view_paths(dir: &Path, out: &mut Vec<PathBuf>) -> Result<(), StoreError> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_view_paths(&path, out)?;
        } else if path.is_file() {
            out.push(path);
        }
    }
    Ok(())
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

    #[test]
    fn test_sorted_overlay_prefers_right_on_collision() {
        let left = vec![
            View::new("a", HashSet::from([[1u8; 32]])),
            View::new("c", HashSet::from([[3u8; 32]])),
        ];
        let right = vec![
            View::new("b", HashSet::from([[2u8; 32]])),
            View::new("c", HashSet::from([[9u8; 32]])),
        ];

        let merged: Vec<View> = merge_sorted_overlay(
            left,
            right,
            |view| view.name.clone(),
            OverlayPrecedence::Right,
        )
        .collect();

        assert_eq!(merged.len(), 3);
        assert_eq!(merged[0].name, "a");
        assert_eq!(merged[1].name, "b");
        assert_eq!(merged[2].name, "c");
        assert!(merged[2].heads.contains(&[9u8; 32]));
    }
}
