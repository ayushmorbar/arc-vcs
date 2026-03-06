use std::collections::{HashSet, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::ai::AiResolver;
use crate::algebra::apply::{apply_change, MaterializedState};
use crate::algebra::commute::commutes;
use crate::algebra::{Atom, Blake3Hash, NodePath};
use crate::ast::rust_plugin::RustPlugin;
use crate::ast::LanguagePlugin;
use crate::store::author::Author;
use crate::store::cas::ObjectStore;
use crate::store::change::Change;
use crate::store::graph::ChangeGraph;
use crate::store::view::View;

/// Persisted conflict state written to `.arc/conflict` when a merge fails.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingConflict {
    pub current_view: String,
    pub target_heads: HashSet<Blake3Hash>,
    pub conflicting_pairs: Vec<(Blake3Hash, Blake3Hash)>,
}

/// Top-level repository handle, tying together the CAS, the change graph,
/// and the on-disk `.arc` layout.
pub struct Repository {
    pub root: PathBuf,
    pub store: ObjectStore,
    pub graph: ChangeGraph,
    /// Optional signing identity set via [`Repository::set_identity`].
    /// Required before calling [`Repository::snap`] or
    /// [`Repository::resolve_conflict`].
    identity: Option<(Author, ed25519_dalek::SigningKey)>,
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
            identity: None,
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
            identity: None,
        })
    }

    /// Store a signing identity on this repository handle.
    ///
    /// Must be called before [`snap`](Repository::snap) or
    /// [`resolve_conflict`](Repository::resolve_conflict).
    pub fn set_identity(&mut self, author: Author, signing_key: ed25519_dalek::SigningKey) {
        self.identity = Some((author, signing_key));
    }

    /// Return a reference to the signing identity, or an error if unset.
    fn signing_identity(
        &self,
    ) -> anyhow::Result<(&Author, &ed25519_dalek::SigningKey)> {
        self.identity
            .as_ref()
            .map(|(a, k)| (a, k))
            .ok_or_else(|| anyhow::anyhow!("no signing identity set — call set_identity() first"))
    }

    /// Read the name of the currently active view from `.arc/HEAD`.
    pub fn current_view_name(&self) -> anyhow::Result<String> {
        let head_path = self.root.join(".arc").join("HEAD");
        let name = fs::read_to_string(&head_path)
            .map_err(|e| anyhow::anyhow!("failed to read .arc/HEAD: {e}"))?;
        Ok(name.trim().to_string())
    }

    /// Populate the in-memory [`ChangeGraph`] by walking backward from an
    /// arbitrary set of heads through the CAS.
    ///
    /// This is idempotent — already-present nodes are skipped.
    pub fn hydrate_heads(&mut self, heads: &HashSet<Blake3Hash>) -> anyhow::Result<()> {
        let mut queue: VecDeque<Blake3Hash> = heads.iter().copied().collect();

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

    /// Populate the in-memory [`ChangeGraph`] by walking backward from a
    /// view's heads through the CAS.
    pub fn hydrate(&mut self, view_name: &str) -> anyhow::Result<()> {
        let view = View::load(&self.root, view_name)
            .map_err(|e| anyhow::anyhow!("failed to load view '{view_name}': {e}"))?;
        self.hydrate_heads(&view.heads)
    }

    /// Replay the DAG in topological order to produce a materialized state
    /// from an arbitrary set of heads.
    pub fn materialize_heads(&self, heads: &HashSet<Blake3Hash>) -> anyhow::Result<MaterializedState> {
        let order = self.graph.topological_sort(heads);
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

    /// Verify the cryptographic integrity of every change in the in-memory graph.
    ///
    /// Iterates all nodes and calls [`Change::verify_signature`] on each.
    /// Returns an error describing the first change that fails verification.
    pub fn verify_graph(&self) -> anyhow::Result<()> {
        for change in self.graph.iter() {
            if !change.verify_signature() {
                let hex: String =
                    change.id.iter().map(|b| format!("{b:02x}")).collect();
                anyhow::bail!("cryptographic verification failed for change {hex}");
            }
        }
        Ok(())
    }

    /// Replay the DAG in topological order to produce a materialized state.
    pub fn materialize(&self, view_name: &str) -> anyhow::Result<MaterializedState> {
        let view = View::load(&self.root, view_name)
            .map_err(|e| anyhow::anyhow!("failed to load view '{view_name}': {e}"))?;
        self.materialize_heads(&view.heads)
    }

    /// Scan the working directory, diff against the materialized history,
    /// and create a new semantic `Change`.
    ///
    /// Returns `Some(change_id)` if a change was created, or `None` if
    /// the working directory matches the materialized state exactly.
    ///
    /// Each file is decomposed into top-level AST items via `diff()`.
    /// The resulting atoms are prefixed with `["file", filepath]` so that
    /// `unparse()` can later reconstruct source per file.
    pub fn snap(&mut self, message: &str) -> anyhow::Result<Option<Blake3Hash>> {
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

            // Reconstruct old source from the materialized AST state.
            let old_src = plugin
                .unparse(&state, filepath)
                .unwrap_or_default();

            // Skip unchanged files entirely.
            if old_src == new_src {
                continue;
            }

            // Diff at the top-level item granularity.
            let ast_atoms = plugin
                .diff(&old_src, &new_src)
                .map_err(|e| anyhow::anyhow!("diff error for {filepath}: {e}"))?;

            if ast_atoms.is_empty() {
                // Whitespace-only change — not semantically meaningful.
                continue;
            }

            has_semantic_change = true;

            // Prefix every atom path with ["file", filepath].
            for atom in ast_atoms {
                all_atoms.push(prefix_atom_path(atom, filepath));
            }
        }

        // Detect deleted files: present in state but missing from disk.
        let state_filepaths = extract_filepaths_from_state(&state);
        for filepath in &state_filepaths {
            if !self.root.join(filepath).exists() {
                has_semantic_change = true;
                // Emit Delete atoms for every state entry belonging to this file.
                let prefix = ["file".to_string(), filepath.clone()];
                for key in state.keys() {
                    if key.len() > prefix.len() && key[..prefix.len()] == prefix[..] {
                        all_atoms.push(Atom::Delete { at: key.clone() });
                    }
                }
            }
        }

        if !has_semantic_change {
            return Ok(None);
        }

        let mut view = View::load(&self.root, &view_name)
            .map_err(|e| anyhow::anyhow!("failed to load view '{view_name}': {e}"))?;

        let (author, signing_key) = self.signing_identity()?;
        let change = Change::new(view.heads.clone(), all_atoms, message, author.clone(), signing_key);
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
        check_working_dir_clean(&self.root, &current_state, "switching views")?;

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
        self.hydrate(target_name)?;
        let target_view = View::load(&self.root, target_name)
            .map_err(|e| anyhow::anyhow!("failed to load view '{target_name}': {e}"))?;
        self.merge_heads(&target_view.heads)
    }

    /// Merge an arbitrary set of heads into the current view.
    ///
    /// This is the head-based primitive underlying `merge_view`. It performs
    /// a dirty-check on the working directory before mutating any state,
    /// then runs the algebraic commutativity check on the exclusive deltas.
    pub fn merge_heads(&mut self, target_heads: &HashSet<Blake3Hash>) -> anyhow::Result<()> {
        let current_name = self.current_view_name()?;

        // Hydrate both sides (idempotent — already-present nodes are skipped).
        self.hydrate(&current_name)?;
        self.hydrate_heads(target_heads)?;

        let current_view = View::load(&self.root, &current_name)
            .map_err(|e| anyhow::anyhow!("failed to load view '{current_name}': {e}"))?;

        // --- Dirty working-directory check ---
        let current_state = self.materialize_heads(&current_view.heads)?;
        check_working_dir_clean(&self.root, &current_state, "merging")?;

        // Find LCA.
        let lca_heads = self
            .graph
            .merge_base(&current_view.heads, target_heads);

        // Compute ancestors from each side and from the LCA.
        let ancestors_current = self.graph.ancestors(&current_view.heads);
        let ancestors_target = self.graph.ancestors(target_heads);
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
        let mut conflicting_pairs = Vec::new();
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
                    conflicting_pairs.push((id_a, id_b));
                }
            }
        }

        if !conflicting_pairs.is_empty() {
            let conflict = PendingConflict {
                current_view: current_name.clone(),
                target_heads: target_heads.clone(),
                conflicting_pairs: conflicting_pairs.clone(),
            };
            let conflict_path = self.root.join(".arc").join("conflict");
            let bytes = bincode::serialize(&conflict)
                .map_err(|e| anyhow::anyhow!("failed to serialize conflict: {e}"))?;
            fs::write(&conflict_path, bytes)?;

            let hex_a: String = conflicting_pairs[0].0.iter().map(|b| format!("{b:02x}")).collect();
            let hex_b: String = conflicting_pairs[0].1.iter().map(|b| format!("{b:02x}")).collect();
            anyhow::bail!(
                "Semantic Conflict detected between {hex_a} and {hex_b}. AI resolution required."
            );
        }

        // All commute — union the heads.
        let mut merged_heads = current_view.heads;
        merged_heads.extend(target_heads);

        let updated_view = View::new(&current_name, merged_heads.clone());
        updated_view
            .save(&self.root)
            .map_err(|e| anyhow::anyhow!("failed to save merged view: {e}"))?;

        // Re-materialize and write to working directory.
        let merged_state = self.materialize_heads(&merged_heads)?;
        write_state_to_working_dir(&self.root, &merged_state)?;

        Ok(())
    }

    /// Resolve a pending conflict stored in `.arc/conflict` using the
    /// provided [`AiResolver`].
    ///
    /// For each conflicting pair the resolver is called with the LCA base,
    /// both sides' content at the overlapping path, and their intents.
    /// The resolved content is committed as a new merge change.
    pub fn resolve_conflict(&mut self, resolver: &dyn AiResolver) -> anyhow::Result<Blake3Hash> {
        let conflict_path = self.root.join(".arc").join("conflict");
        if !conflict_path.exists() {
            anyhow::bail!("no pending conflict — nothing to resolve");
        }

        let conflict_bytes = fs::read(&conflict_path)?;
        let conflict: PendingConflict = bincode::deserialize(&conflict_bytes)
            .map_err(|e| anyhow::anyhow!("failed to deserialize conflict: {e}"))?;

        // Hydrate current view and target heads.
        self.hydrate(&conflict.current_view)?;
        self.hydrate_heads(&conflict.target_heads)?;

        let current_view = View::load(&self.root, &conflict.current_view)
            .map_err(|e| anyhow::anyhow!("failed to load view '{}': {e}", conflict.current_view))?;

        // Materialize LCA state directly from heads — no temp view needed.
        let lca_heads = self.graph.merge_base(&current_view.heads, &conflict.target_heads);
        let lca_state = if lca_heads.is_empty() {
            MaterializedState::new()
        } else {
            self.materialize_heads(&lca_heads)?
        };

        let current_state = self.materialize_heads(&current_view.heads)?;
        let target_state = self.materialize_heads(&conflict.target_heads)?;

        let mut merge_atoms = Vec::new();
        let mut combined_intent = String::from("AI merge: ");

        for (id_a, id_b) in &conflict.conflicting_pairs {
            let change_a = self.graph.get(id_a)
                .ok_or_else(|| anyhow::anyhow!("conflicting change missing from graph"))?
                .clone();
            let change_b = self.graph.get(id_b)
                .ok_or_else(|| anyhow::anyhow!("conflicting change missing from graph"))?
                .clone();

            let overlap = find_overlapping_path(&change_a.atoms, &change_b.atoms);
            let path = overlap.ok_or_else(|| {
                anyhow::anyhow!("no overlapping path found for conflicting pair")
            })?;

            let base_content = extract_content_at_path(&lca_state, &path);
            let ours_content = extract_content_at_path(&current_state, &path);
            let theirs_content = extract_content_at_path(&target_state, &path);

            let resolved = resolver
                .resolve(
                    &base_content,
                    &ours_content,
                    &theirs_content,
                    &change_a.intent,
                    &change_b.intent,
                )
                .map_err(|e| anyhow::anyhow!("AI resolver failed: {e}"))?;

            merge_atoms.push(Atom::Insert {
                at: path,
                content: resolved,
            });

            combined_intent.push_str(&change_a.intent);
            combined_intent.push_str(" + ");
            combined_intent.push_str(&change_b.intent);
            combined_intent.push_str("; ");
        }

        // Deps = union of current view's heads and target heads.
        let mut merge_deps = current_view.heads.clone();
        merge_deps.extend(&conflict.target_heads);

        let (author, signing_key) = self.signing_identity()?;
        let merge_change = Change::new(merge_deps, merge_atoms, combined_intent, author.clone(), signing_key);
        self.store.write_change(&merge_change)
            .map_err(|e| anyhow::anyhow!("CAS write error: {e}"))?;
        self.graph.add_change(merge_change.clone());

        // Update the current view to point to the merge change.
        let updated = View::new(&conflict.current_view, HashSet::from([merge_change.id]));
        updated.save(&self.root)
            .map_err(|e| anyhow::anyhow!("failed to save resolved view: {e}"))?;

        // Re-materialize and write to working directory.
        let resolved_state = self.materialize_heads(&HashSet::from([merge_change.id]))?;
        write_state_to_working_dir(&self.root, &resolved_state)?;

        // Remove the conflict file.
        fs::remove_file(&conflict_path)?;

        Ok(merge_change.id)
    }
}

/// Find the first overlapping AST path between two sets of atoms.
fn find_overlapping_path(atoms_a: &[Atom], atoms_b: &[Atom]) -> Option<NodePath> {
    for a in atoms_a {
        for b in atoms_b {
            for pa in a.paths() {
                for pb in b.paths() {
                    let min_len = pa.len().min(pb.len());
                    if pa[..min_len] == pb[..min_len] {
                        // Return the longer (more specific) path.
                        if pa.len() >= pb.len() {
                            return Some(pa.clone());
                        } else {
                            return Some(pb.clone());
                        }
                    }
                }
            }
        }
    }
    None
}

/// Prepend `["file", filepath]` to every path inside an `Atom`.
pub(crate) fn prefix_atom_path(atom: Atom, filepath: &str) -> Atom {
    let prepend = |mut path: NodePath| -> NodePath {
        let mut prefixed = vec!["file".to_string(), filepath.to_string()];
        prefixed.append(&mut path);
        prefixed
    };
    match atom {
        Atom::Insert { at, content } => Atom::Insert { at: prepend(at), content },
        Atom::Delete { at } => Atom::Delete { at: prepend(at) },
        Atom::Move { from, to } => Atom::Move { from: prepend(from), to: prepend(to) },
        Atom::SemanticsPreserving { at, description } => {
            Atom::SemanticsPreserving { at: prepend(at), description }
        }
    }
}

/// Collect the set of unique file paths present in a materialized state.
///
/// Looks for keys whose first segment is `"file"` and extracts the second
/// segment as the filepath.
fn extract_filepaths_from_state(state: &MaterializedState) -> HashSet<String> {
    let mut paths = HashSet::new();
    for key in state.keys() {
        if key.len() >= 2 && key[0] == "file" {
            paths.insert(key[1].clone());
        }
    }
    paths
}

/// Extract content at the given path from a materialized state.
///
/// Returns an empty `Vec` if the path is not present.
fn extract_content_at_path(state: &MaterializedState, path: &NodePath) -> Vec<u8> {
    state.get(path).cloned().unwrap_or_default()
}

/// Check that the working directory matches the given materialized state.
///
/// Returns an error if un-snapped changes are detected, preventing
/// destructive overwrites during `merge_heads`, `pull`, or `switch_view`.
fn check_working_dir_clean(
    root: &Path,
    state: &MaterializedState,
    context: &str,
) -> anyhow::Result<()> {
    let plugin = RustPlugin::new();
    let rs_files = collect_rs_files(root)?;

    for filepath in &rs_files {
        let new_src = fs::read_to_string(root.join(filepath))?;
        let old_src = plugin.unparse(state, filepath).unwrap_or_default();

        if old_src == new_src {
            continue;
        }

        // Unparse returned empty but file exists → new file.
        if old_src.is_empty() {
            anyhow::bail!(
                "working directory is dirty — snap your changes before {context}"
            );
        }

        let ast_atoms = plugin
            .diff(&old_src, &new_src)
            .map_err(|e| anyhow::anyhow!("diff error: {e}"))?;
        if !ast_atoms.is_empty() {
            anyhow::bail!(
                "working directory is dirty — snap your changes before {context}"
            );
        }
    }

    // Check for files in state that no longer exist on disk.
    for filepath in extract_filepaths_from_state(state) {
        if !root.join(&filepath).exists() {
            anyhow::bail!(
                "working directory is dirty — snap your changes before {context}"
            );
        }
    }

    Ok(())
}

/// Overwrite the working directory with the given materialized state.
///
/// Instead of blindly wiping all `.rs` files (which can fail on Windows
/// if an IDE or language server holds a file lock), we iterate the
/// *known* file set, tolerate `NotFound` errors, and then write the
/// target state reconstructed via `unparse()`.
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

    // Reconstruct files from the materialized AST state via unparse.
    let plugin = RustPlugin::new();
    for filepath in extract_filepaths_from_state(state) {
        let source = plugin
            .unparse(state, &filepath)
            .map_err(|e| anyhow::anyhow!("unparse error for {filepath}: {e}"))?;

        if source.is_empty() {
            continue;
        }

        let full = root.join(&filepath);
        if let Some(parent) = full.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&full, source.as_bytes())?;
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
        let (author, signing_key) = crate::store::author::test_keypair();
        repo.set_identity(author, signing_key);

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

        // Materialize and verify the file content via unparse.
        let state = repo.materialize("main").unwrap();
        let plugin = crate::ast::rust_plugin::RustPlugin::new();
        let reconstructed = plugin.unparse(&state, "test.rs").unwrap();
        assert_eq!(reconstructed, "fn main() { let x = 1; }");

        // The old flat key ["file", "test.rs"] must NOT exist.
        let flat_key = vec!["file".to_string(), "test.rs".to_string()];
        assert!(
            state.get(&flat_key).is_none(),
            "source-map hack must be eliminated — no flat file key allowed"
        );
    }

    #[test]
    fn test_repo_branch_and_merge() {
        let dir = tempfile::tempdir().unwrap();
        let repo_path = dir.path().join("merge_project");

        let mut repo = Repository::init(&repo_path).unwrap();
        let (author, signing_key) = crate::store::author::test_keypair();
        repo.set_identity(author, signing_key);

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
        let (author, signing_key) = crate::store::author::test_keypair();
        repo.set_identity(author, signing_key);

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

        // Verify .arc/conflict was persisted.
        assert!(
            repo_path.join(".arc").join("conflict").exists(),
            ".arc/conflict must exist after a failed merge"
        );
    }

    #[test]
    fn test_ai_conflict_resolution() {
        use crate::ai::MockResolver;

        let dir = tempfile::tempdir().unwrap();
        let repo_path = dir.path().join("ai_resolve_project");

        let mut repo = Repository::init(&repo_path).unwrap();
        let (author, signing_key) = crate::store::author::test_keypair();
        repo.set_identity(author, signing_key);

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

        // Merge fails — creates .arc/conflict.
        let result = repo.merge_view("feature");
        assert!(result.is_err());

        // Resolve via the mock AI resolver.
        let resolver = MockResolver;
        let merge_id = repo.resolve_conflict(&resolver).unwrap();

        // The merge change ID should be non-zero.
        assert_ne!(merge_id, [0u8; 32]);

        // .arc/conflict should be cleaned up.
        assert!(
            !repo_path.join(".arc").join("conflict").exists(),
            ".arc/conflict must be removed after resolution"
        );

        // The working directory should have shared.rs with merged content.
        let content = fs::read_to_string(repo_path.join("shared.rs")).unwrap();
        // MockResolver concatenates ours + "\n" + theirs.
        assert!(
            content.contains("fn shared() { let b = 2; }")
                && content.contains("fn shared() { let a = 1; }"),
            "merged content must contain both sides, got: {content}"
        );
    }
}
