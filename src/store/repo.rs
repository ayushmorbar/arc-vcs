use std::collections::{HashSet, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};

use crate::algebra::apply::{apply_change, MaterializedState};
use crate::algebra::commute::commutes;
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
    ///     HEAD            ("main")
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

        // Set active view to "main".
        fs::write(arc_dir.join("HEAD"), "main")?;

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

    /// Read the name of the currently active view from `.arc/HEAD`.
    pub fn current_view_name(&self) -> anyhow::Result<String> {
        let head_path = self.root.join(".arc").join("HEAD");
        let name = fs::read_to_string(&head_path)
            .map_err(|e| anyhow::anyhow!("failed to read .arc/HEAD: {e}"))?;
        Ok(name.trim().to_string())
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
        let view_name = self.current_view_name()?;
        self.hydrate(&view_name)?;
        let state = self.materialize(&view_name)?;

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

        let mut view = View::load(&self.root, &view_name)
            .map_err(|e| anyhow::anyhow!("failed to load view '{view_name}': {e}"))?;

        let change = Change::new(view.heads.clone(), all_atoms);
        self.store
            .write_change(&change)
            .map_err(|e| anyhow::anyhow!("CAS write error: {e}"))?;
        self.graph.add_change(change.clone());

        // Advance the frontier: new change becomes the sole head.
        view.heads = HashSet::from([change.id]);
        view.save(&self.root)
            .map_err(|e| anyhow::anyhow!("failed to save view: {e}"))?;

        Ok(Some(change.id))
    }

    // ------------------------------------------------------------------
    // View orchestration
    // ------------------------------------------------------------------

    /// Create a new view forked from the current view's heads.
    pub fn create_view(&self, name: &str) -> anyhow::Result<()> {
        let current_name = self.current_view_name()?;
        let current_view = View::load(&self.root, &current_name)
            .map_err(|e| anyhow::anyhow!("failed to load view '{current_name}': {e}"))?;

        let view_path = self.root.join(".arc").join("views").join(name);
        if view_path.exists() {
            anyhow::bail!("view '{name}' already exists");
        }

        let new_view = View::new(name, current_view.heads);
        new_view
            .save(&self.root)
            .map_err(|e| anyhow::anyhow!("failed to save view '{name}': {e}"))?;

        Ok(())
    }

    /// Switch the working directory to `target` view.
    ///
    /// Fails if the working directory has un-snapped changes.
    pub fn switch_view(&mut self, target: &str) -> anyhow::Result<()> {
        // Verify the target view exists.
        let target_view = View::load(&self.root, target)
            .map_err(|e| anyhow::anyhow!("view '{target}' not found: {e}"))?;

        let current_name = self.current_view_name()?;

        // Hydrate and materialize the current view to detect dirty state.
        self.hydrate(&current_name)?;
        let current_state = self.materialize(&current_name)?;

        // Check for un-snapped changes.
        let plugin = RustPlugin::new();
        let rs_files = collect_rs_files(&self.root)?;

        for filepath in &rs_files {
            let new_src = fs::read_to_string(self.root.join(filepath))?;
            let file_key = vec!["file".to_string(), filepath.clone()];
            let old_bytes = current_state.get(&file_key);

            if old_bytes != Some(&new_src.as_bytes().to_vec()) {
                if let Some(ob) = old_bytes {
                    let old_src = String::from_utf8_lossy(ob);
                    let ast_atoms = plugin
                        .diff(&old_src, &new_src)
                        .map_err(|e| anyhow::anyhow!("diff error: {e}"))?;
                    if !ast_atoms.is_empty() {
                        anyhow::bail!(
                            "working directory is dirty — snap your changes before switching views"
                        );
                    }
                } else {
                    anyhow::bail!(
                        "working directory is dirty — snap your changes before switching views"
                    );
                }
            }
        }

        // Check for files in state that no longer exist on disk.
        for key in current_state.keys() {
            if key.first().map(|s| s.as_str()) == Some("file")
                && let Some(filepath) = key.get(1)
                && !self.root.join(filepath).exists()
            {
                anyhow::bail!(
                    "working directory is dirty — snap your changes before switching views"
                );
            }
        }

        // Hydrate the target view.
        self.hydrate(target)?;

        // Materialize the target view.
        let target_state = if target_view.heads.is_empty() {
            MaterializedState::new()
        } else {
            self.materialize(target)?
        };

        // Replace working directory with target state.
        write_state_to_working_dir(&self.root, &target_state)?;

        // Update HEAD.
        fs::write(self.root.join(".arc").join("HEAD"), target)?;

        Ok(())
    }

    /// Merge `target_name` view into the current view using the algebraic
    /// merge law.
    ///
    /// If all exclusive changes commute, the merge is a simple head-union
    /// with no merge commit. If any pair conflicts, aborts with an error.
    pub fn merge_view(&mut self, target_name: &str) -> anyhow::Result<()> {
        let current_name = self.current_view_name()?;

        // Hydrate both views.
        self.hydrate(&current_name)?;
        self.hydrate(target_name)?;

        let current_view = View::load(&self.root, &current_name)
            .map_err(|e| anyhow::anyhow!("failed to load view '{current_name}': {e}"))?;
        let target_view = View::load(&self.root, target_name)
            .map_err(|e| anyhow::anyhow!("failed to load view '{target_name}': {e}"))?;

        // Find LCA.
        let lca_heads = self
            .graph
            .merge_base(&current_view.heads, &target_view.heads);

        // Compute ancestors from each side and from the LCA.
        let ancestors_current = self.graph.ancestors(&current_view.heads);
        let ancestors_target = self.graph.ancestors(&target_view.heads);
        let ancestors_lca = if lca_heads.is_empty() {
            HashSet::new()
        } else {
            self.graph.ancestors(&lca_heads)
        };

        // ΔA = changes in current but not in LCA ancestry.
        let delta_a: Vec<Blake3Hash> = ancestors_current
            .difference(&ancestors_lca)
            .copied()
            .collect();

        // ΔB = changes in target but not in LCA ancestry.
        let delta_b: Vec<Blake3Hash> = ancestors_target
            .difference(&ancestors_lca)
            .copied()
            .collect();

        // Cross-product commutativity check.
        for &id_a in &delta_a {
            let change_a = self
                .graph
                .get(&id_a)
                .ok_or_else(|| anyhow::anyhow!("change missing from graph"))?;
            for &id_b in &delta_b {
                let change_b = self
                    .graph
                    .get(&id_b)
                    .ok_or_else(|| anyhow::anyhow!("change missing from graph"))?;
                if !commutes(change_a, change_b) {
                    let hex_a: String = id_a.iter().map(|b| format!("{b:02x}")).collect();
                    let hex_b: String = id_b.iter().map(|b| format!("{b:02x}")).collect();
                    anyhow::bail!(
                        "Semantic Conflict detected between {hex_a} and {hex_b}. AI resolution required."
                    );
                }
            }
        }

        // All commute — union the heads.
        let mut merged_heads = current_view.heads;
        merged_heads.extend(&target_view.heads);

        let updated_view = View::new(&current_name, merged_heads);
        updated_view
            .save(&self.root)
            .map_err(|e| anyhow::anyhow!("failed to save merged view: {e}"))?;

        // Re-materialize and write to working directory.
        let merged_state = self.materialize(&current_name)?;
        write_state_to_working_dir(&self.root, &merged_state)?;

        Ok(())
    }
}

/// Overwrite the working directory with the given materialized state.
///
/// Instead of blindly wiping all `.rs` files (which can fail on Windows
/// if an IDE or language server holds a file lock), we iterate the
/// *known* file set, tolerate `NotFound` errors, and then write the
/// target state.
fn write_state_to_working_dir(root: &Path, state: &MaterializedState) -> anyhow::Result<()> {
    // Remove existing .rs files, tolerating NotFound.
    let existing = collect_rs_files(root)?;
    for filepath in &existing {
        let full = root.join(filepath);
        match fs::remove_file(&full) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(anyhow::anyhow!("failed to remove {}: {e}", full.display())),
        }
    }

    // Write files from the materialized state.
    for (key, content) in state {
        if key.first().map(|s| s.as_str()) == Some("file")
            && let Some(filepath) = key.get(1)
        {
            let full = root.join(filepath);
            if let Some(parent) = full.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::write(&full, content)?;
        }
    }

    Ok(())
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
        assert!(repo_path.join(".arc").join("HEAD").is_file());

        // Verify HEAD points to "main".
        assert_eq!(repo.current_view_name().unwrap(), "main");

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

    #[test]
    fn test_repo_branch_and_merge() {
        let dir = tempfile::tempdir().unwrap();
        let repo_path = dir.path().join("merge_project");

        let mut repo = Repository::init(&repo_path).unwrap();

        // Snap file A on main.
        fs::write(repo_path.join("a.rs"), "fn a() {}").unwrap();
        repo.snap("add a.rs").unwrap();

        // Create and switch to "feature" view.
        repo.create_view("feature").unwrap();
        repo.switch_view("feature").unwrap();
        assert_eq!(repo.current_view_name().unwrap(), "feature");

        // Snap file B on feature.
        fs::write(repo_path.join("b.rs"), "fn b() {}").unwrap();
        repo.snap("add b.rs on feature").unwrap();

        // Switch back to main.
        repo.switch_view("main").unwrap();
        assert_eq!(repo.current_view_name().unwrap(), "main");

        // Verify b.rs is gone (main doesn't have it).
        assert!(
            !repo_path.join("b.rs").exists(),
            "b.rs should not exist on main"
        );

        // Snap file C on main.
        fs::write(repo_path.join("c.rs"), "fn c() {}").unwrap();
        repo.snap("add c.rs on main").unwrap();

        // Merge feature into main — disjoint files, must commute.
        repo.merge_view("feature").unwrap();

        // After merge, all three files should be present.
        assert!(repo_path.join("a.rs").exists(), "a.rs must exist after merge");
        assert!(repo_path.join("b.rs").exists(), "b.rs must exist after merge");
        assert!(repo_path.join("c.rs").exists(), "c.rs must exist after merge");

        // The main view should have 2 heads (one from each branch).
        let main_view = View::load(&repo_path, "main").unwrap();
        assert_eq!(
            main_view.heads.len(),
            2,
            "merged view must have 2 heads, got: {:?}",
            main_view.heads
        );
    }

    #[test]
    fn test_repo_conflict() {
        let dir = tempfile::tempdir().unwrap();
        let repo_path = dir.path().join("conflict_project");

        let mut repo = Repository::init(&repo_path).unwrap();

        // Snap initial file on main.
        fs::write(repo_path.join("shared.rs"), "fn shared() {}").unwrap();
        repo.snap("initial shared.rs").unwrap();

        // Create and switch to "feature".
        repo.create_view("feature").unwrap();
        repo.switch_view("feature").unwrap();

        // Modify shared.rs on feature.
        fs::write(repo_path.join("shared.rs"), "fn shared() { let a = 1; }").unwrap();
        repo.snap("modify shared.rs on feature").unwrap();

        // Switch back to main and modify the same file differently.
        repo.switch_view("main").unwrap();
        fs::write(repo_path.join("shared.rs"), "fn shared() { let b = 2; }").unwrap();
        repo.snap("modify shared.rs on main").unwrap();

        // Merge should fail — same file modified on both sides.
        let result = repo.merge_view("feature");
        assert!(result.is_err(), "merge of conflicting changes must fail");
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("Semantic Conflict"),
            "error must mention 'Semantic Conflict', got: {err_msg}"
        );
    }
}
