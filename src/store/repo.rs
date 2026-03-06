use std::collections::{HashSet, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};

use crate::algebra::apply::{apply_change, MaterializedState};
use crate::algebra::{Atom, Blake3Hash};
use crate::ast::rust_plugin::RustPlugin;
use crate::ast::LanguagePlugin;
use crate::store::cas::ObjectStore;
use crate::store::change::Change;
use crate::store::graph::ChangeGraph;
use crate::store::view::View;

/// Top-level repository handle, tying together the CAS, the change graph,
/// and the on-disk `.arc` layout.
pub struct Repository {
    pub root: PathBuf,
    pub store: ObjectStore,
    pub graph: ChangeGraph,
}

impl Repository {
    /// Initialize a new arc repository at `path`.
    ///
    /// Creates the directory structure:
    /// ```text
    /// <path>/
    ///   .arc/
    ///     store/
    ///     views/
    ///       main          (empty-heads view)
    /// ```
    pub fn init(path: impl AsRef<Path>) -> Result<Self, anyhow::Error> {
        let root = path.as_ref().to_path_buf();
        let arc_dir = root.join(".arc");

        if arc_dir.exists() {
            anyhow::bail!("repository already exists at {}", arc_dir.display());
        }

        fs::create_dir_all(arc_dir.join("store"))?;
        fs::create_dir_all(arc_dir.join("views"))?;

        // Create the default "main" view with an empty head set.
        let main_view = View::new("main", HashSet::new());
        main_view
            .save(&root)
            .map_err(|e| anyhow::anyhow!("failed to save initial main view: {e}"))?;

        Ok(Self {
            store: ObjectStore::new(&root),
            graph: ChangeGraph::new(),
            root,
        })
    }

    /// Open an existing repository by locating the `.arc` directory at `path`.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, anyhow::Error> {
        let root = path.as_ref().to_path_buf();
        let arc_dir = root.join(".arc");

        if !arc_dir.exists() {
            anyhow::bail!("no arc repository found at {}", arc_dir.display());
        }

        Ok(Self {
            store: ObjectStore::new(&root),
            graph: ChangeGraph::new(),
            root,
        })
    }

    /// Populate the in-memory [`ChangeGraph`] by walking backward from a
    /// view's heads through the CAS.
    pub fn hydrate(&mut self, view_name: &str) -> anyhow::Result<()> {
        let view = View::load(&self.root, view_name)
            .map_err(|e| anyhow::anyhow!("failed to load view '{view_name}': {e}"))?;

        let mut queue: VecDeque<Blake3Hash> = view.heads.iter().copied().collect();

        while let Some(id) = queue.pop_front() {
            if self.graph.get(&id).is_some() {
                continue;
            }
            let change = self
                .store
                .read_change(&id)
                .map_err(|e| anyhow::anyhow!("failed to read change from CAS: {e}"))?;
            for &dep in &change.deps {
                if self.graph.get(&dep).is_none() {
                    queue.push_back(dep);
                }
            }
            self.graph.add_change(change);
        }

        Ok(())
    }

    /// Replay the DAG in topological order to produce a materialized state.
    pub fn materialize(&self, view_name: &str) -> anyhow::Result<MaterializedState> {
        let view = View::load(&self.root, view_name)
            .map_err(|e| anyhow::anyhow!("failed to load view '{view_name}': {e}"))?;

        let order = self.graph.topological_sort(&view.heads);
        let mut state = MaterializedState::new();

        for id in order {
            let change = self
                .graph
                .get(&id)
                .ok_or_else(|| anyhow::anyhow!("change {id:?} missing from graph"))?;
            apply_change(&mut state, change)
                .map_err(|e| anyhow::anyhow!("replay error: {e}"))?;
        }

        Ok(state)
    }

    /// Scan the working directory, diff against the materialized history,
    /// and create a new semantic `Change`.
    ///
    /// Returns `Some(change_id)` if a change was created, or `None` if
    /// the working directory matches the materialized state exactly.
    ///
    /// # Source-Map Strategy
    ///
    /// AST-level atoms from `tree-sitter` are used for semantic change
    /// detection only. The actual `Change` stores file-map atoms
    /// (`["file", path] → full source`) so that replay stays consistent.
    /// In Phase 7, when `unparse()` is implemented, we will switch to
    /// storing pure AST atoms and reconstructing source on demand.
    pub fn snap(&mut self, _message: &str) -> anyhow::Result<Option<Blake3Hash>> {
        self.hydrate("main")?;
        let state = self.materialize("main")?;

        let plugin = RustPlugin::new();
        let mut all_atoms = Vec::new();
        let mut has_semantic_change = false;

        // Collect .rs files from the working directory (recursive).
        let rs_files = collect_rs_files(&self.root)?;

        for filepath in &rs_files {
            let new_src = fs::read_to_string(self.root.join(filepath))?;
            let new_bytes = new_src.as_bytes();

            let file_key = vec!["file".to_string(), filepath.clone()];
            let old_bytes = state.get(&file_key);

            // Skip unchanged files entirely.
            if old_bytes == Some(&new_bytes.to_vec()) {
                continue;
            }

            // Use AST diff for semantic change detection when both
            // old and new source exist. New files are always a change.
            if let Some(ob) = old_bytes {
                let old_src = String::from_utf8_lossy(ob);
                let ast_atoms = plugin
                    .diff(&old_src, &new_src)
                    .map_err(|e| anyhow::anyhow!("diff error for {filepath}: {e}"))?;
                if !ast_atoms.is_empty() {
                    has_semantic_change = true;
                }
            } else {
                has_semantic_change = true;
            }

            // Store the full file text as the replayable representation.
            all_atoms.push(Atom::Insert {
                at: file_key,
                content: new_src.into_bytes(),
            });
        }

        if !has_semantic_change {
            return Ok(None);
        }

        let mut main_view = View::load(&self.root, "main")
            .map_err(|e| anyhow::anyhow!("failed to load main view: {e}"))?;

        let change = Change::new(main_view.heads.clone(), all_atoms);
        self.store
            .write_change(&change)
            .map_err(|e| anyhow::anyhow!("CAS write error: {e}"))?;
        self.graph.add_change(change.clone());

        // Advance the frontier: new change becomes the sole head.
        main_view.heads = HashSet::from([change.id]);
        main_view
            .save(&self.root)
            .map_err(|e| anyhow::anyhow!("failed to save main view: {e}"))?;

        Ok(Some(change.id))
    }
}

/// Recursively collect `*.rs` file paths relative to `root`.
fn collect_rs_files(root: &Path) -> anyhow::Result<Vec<String>> {
    let mut files = Vec::new();
    collect_rs_recursive(root, root, &mut files)?;
    files.sort();
    Ok(files)
}

fn collect_rs_recursive(
    base: &Path,
    dir: &Path,
    files: &mut Vec<String>,
) -> anyhow::Result<()> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(_) => return Ok(()),
    };
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        // Skip .arc and hidden directories.
        if let Some(name) = path.file_name().and_then(|n| n.to_str())
            && name.starts_with('.')
        {
            continue;
        }
        if path.is_dir() {
            collect_rs_recursive(base, &path, files)?;
        } else if path.extension().and_then(|e| e.to_str()) == Some("rs")
            && let Ok(rel) = path.strip_prefix(base)
        {
            let rel_str = rel.to_string_lossy().replace('\\', "/");
            files.push(rel_str);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_repo_init() {
        let dir = tempfile::tempdir().unwrap();
        let repo_path = dir.path().join("my_project");

        let repo = Repository::init(&repo_path).unwrap();

        // Verify .arc directory structure exists.
        assert!(repo_path.join(".arc").is_dir());
        assert!(repo_path.join(".arc").join("store").is_dir());
        assert!(repo_path.join(".arc").join("views").is_dir());
        assert!(repo_path.join(".arc").join("views").join("main").is_file());

        // Verify the main view can be loaded and has empty heads.
        let main_view = View::load(&repo_path, "main").unwrap();
        assert_eq!(main_view.name, "main");
        assert!(main_view.heads.is_empty());

        // Verify re-init on the same path fails.
        let result = Repository::init(&repo_path);
        assert!(result.is_err());

        // Verify open works on an initialized repo.
        let reopened = Repository::open(&repo_path).unwrap();
        assert_eq!(reopened.root, repo.root);
    }

    #[test]
    fn test_repo_snap() {
        let dir = tempfile::tempdir().unwrap();
        let repo_path = dir.path().join("snap_project");

        let mut repo = Repository::init(&repo_path).unwrap();

        // Write a Rust file into the working directory.
        fs::write(repo_path.join("test.rs"), "fn main() {}").unwrap();

        // First snap should produce a change.
        let first = repo.snap("first commit").unwrap();
        assert!(first.is_some(), "first snap should produce a change");
        let first_id = first.unwrap();

        // Snapping again with no changes should return None.
        let noop = repo.snap("no-op").unwrap();
        assert!(noop.is_none(), "snap with no changes should return None");

        // Modify the file and snap again.
        fs::write(repo_path.join("test.rs"), "fn main() { let x = 1; }").unwrap();

        let second = repo.snap("second commit").unwrap();
        assert!(second.is_some(), "second snap should produce a change");
        let second_id = second.unwrap();
        assert_ne!(first_id, second_id);

        // Materialize and verify the file content.
        let state = repo.materialize("main").unwrap();
        let file_key = vec!["file".to_string(), "test.rs".to_string()];
        let content = state.get(&file_key).expect("file key must exist");
        assert_eq!(
            String::from_utf8_lossy(content),
            "fn main() { let x = 1; }"
        );
    }
}
