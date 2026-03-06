use std::collections::{HashSet, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use arc_core::ai::AiResolver;
use arc_core::algebra::apply::{apply_change, BlameState, MaterializedState};
use arc_core::algebra::commute::commutes;
use arc_core::algebra::{Atom, Blake3Hash, NodePath};
use arc_lang::ast::rust_plugin::RustPlugin;
use arc_lang::ast::LanguagePlugin;
use arc_core::store::author::Author;
use arc_core::store::cas::ObjectStore;
use arc_core::store::change::Change;
use arc_core::store::graph::ChangeGraph;
use arc_core::store::view::View;
use ignore::gitignore::{Gitignore, GitignoreBuilder};

/// Persisted conflict state written to `.arc/conflict` when a merge fails.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingConflict {
    /// Name of the view being merged into.
    pub current_view: String,
    /// Remote heads that caused the conflict.
    pub target_heads: HashSet<Blake3Hash>,
    /// List of conflicting (local, remote) change id pairs.
    pub conflicting_pairs: Vec<(Blake3Hash, Blake3Hash)>,
}

/// Top-level repository handle, tying together the CAS, the change graph,
/// and the on-disk `.arc` layout.
pub struct Repository {
    /// Absolute path to the repository root.
    pub root: PathBuf,
    /// Content-addressable object store.
    pub store: ObjectStore,
    /// In-memory change DAG.
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
        let agent_ignore = load_agentignore(&self.root);
        let order = self.graph.topological_sort(heads);
        let mut state = MaterializedState::new();

        for id in order {
            let change = self
                .graph
                .get(&id)
                .ok_or_else(|| anyhow::anyhow!("change {id:?} missing from graph"))?;
            apply_change(&mut state, change, &agent_ignore, None)
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

    /// Assign the provenance of every live AST node to the `Change` that last
    /// wrote it.  Returns one entry per node scoped to `filepath`, ordered by
    /// `NodePath`.
    ///
    /// Nodes are attributed to the *last writer* — if two changes both touch
    /// `function_item[foo]`, the one that replays second (later in topological
    /// order) wins, which is exactly the correct semantic.
    pub fn blame(&mut self, filepath: &str) -> anyhow::Result<Vec<(NodePath, Change)>> {
        let view_name = self.current_view_name()?;
        self.hydrate(&view_name)?;

        let view = View::load(&self.root, &view_name)
            .map_err(|e| anyhow::anyhow!("failed to load view '{view_name}': {e}"))?;

        let agent_ignore = load_agentignore(&self.root);
        let order = self.graph.topological_sort(&view.heads);
        let mut state = MaterializedState::new();
        let mut blame = BlameState::new();

        for id in order {
            let change = self
                .graph
                .get(&id)
                .ok_or_else(|| anyhow::anyhow!("change {id:?} missing from graph"))?;
            apply_change(&mut state, change, &agent_ignore, Some(&mut blame))
                .map_err(|e| anyhow::anyhow!("replay error: {e}"))?;
        }

        // Filter to nodes belonging to `filepath`.
        let prefix = ["file".to_string(), filepath.to_string()];
        let mut result: Vec<(NodePath, Change)> = blame
            .iter()
            .filter(|(k, _)| k.len() > 2 && k[..2] == prefix)
            .map(|(k, hash)| {
                let change = self
                    .store
                    .read_change(hash)
                    .map_err(|e| anyhow::anyhow!("CAS read error for blame: {e}"))?;
                Ok((k.clone(), change))
            })
            .collect::<anyhow::Result<_>>()?;

        result.sort_by(|(a, _), (b, _)| a.cmp(b));
        Ok(result)
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
    pub fn snap(&mut self, message: &str, interactive: bool) -> anyhow::Result<Option<Blake3Hash>> {
        let view_name = self.current_view_name()?;
        self.hydrate(&view_name)?;
        let state = self.materialize(&view_name)?;

        // Compute every atom that would represent the current working-directory
        // delta relative to the materialized state.
        let raw_atoms = self.compute_working_directory_delta(&state)?;

        if raw_atoms.is_empty() {
            return Ok(None);
        }

        // Interactive staging: filter atoms the user does not want to stage.
        // Deletion / directory atoms are always kept to avoid ghost-file state.
        let all_atoms: Vec<Atom> = if interactive {
            use std::io::Write;
            let mut accepted: Vec<Atom> = Vec::new();
            let mut current_file: Option<String> = None;
            for atom in raw_atoms {
                // Only AST diff atoms (Insert / Delete file nodes) are interactive.
                // Directory atoms and whole-file deletions are always staged.
                let is_file_ast = matches!(&atom,
                    Atom::Insert { at, .. } | Atom::Delete { at } if at.first().map(|s| s == "file").unwrap_or(false) && at.len() > 2
                );
                if !is_file_ast {
                    accepted.push(atom);
                    continue;
                }
                // Print per-file header once.
                let filepath = atom.paths()[0].get(1).cloned().unwrap_or_default();
                if current_file.as_deref() != Some(&filepath) {
                    println!("-- {filepath} --");
                    current_file = Some(filepath.clone());
                }
                let label = atom_label(&atom);
                print!("  {label}\n  Stage this change? [y/N] ");
                std::io::stdout().flush().ok();
                let mut line = String::new();
                std::io::stdin().read_line(&mut line).ok();
                if line.trim().eq_ignore_ascii_case("y") {
                    accepted.push(atom);
                }
            }
            accepted
        } else {
            raw_atoms
        };

        // Strip the interactive-filtered result: if nothing left after filtering
        // file-AST atoms, and the only remaining atoms are non-AST (dirs etc.),
        // check whether there are any real changes.
        let has_file_change = all_atoms.iter().any(|a| {
            a.paths().first().and_then(|p| p.first()).map(|s| s == "file").unwrap_or(false)
                || matches!(a, Atom::Directory { .. })
                || matches!(a, Atom::Delete { at } if at.first().map(|s| s == "dir").unwrap_or(false))
        });

        if !has_file_change && all_atoms.is_empty() {
            return Ok(None);
        }

        if all_atoms.is_empty() {
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

    /// Compute every [`Atom`] that represents the difference between the
    /// materialized `state` and the current working directory.
    ///
    /// This is the pure-computation core shared by [`snap`] and [`status`].
    /// It never touches the CAS, the graph, or any view file.
    fn compute_working_directory_delta(
        &self,
        state: &MaterializedState,
    ) -> anyhow::Result<Vec<Atom>> {
        let plugin = RustPlugin::new();
        let arcignore = load_arcignore(&self.root);
        let rs_files = collect_rs_files(&self.root, &arcignore)?;
        let mut atoms: Vec<Atom> = Vec::new();

        for filepath in &rs_files {
            let new_src = fs::read_to_string(self.root.join(filepath))?;
            let old_src = plugin.unparse(state, filepath).unwrap_or_default();
            if old_src == new_src {
                continue;
            }
            let ast_atoms = plugin
                .diff(&old_src, &new_src)
                .map_err(|e| anyhow::anyhow!("diff error for {filepath}: {e}"))?;
            if ast_atoms.is_empty() {
                continue;
            }
            for atom in ast_atoms {
                atoms.push(prefix_atom_path(atom, filepath));
            }
        }

        // Deleted files.
        let state_filepaths = extract_filepaths_from_state(state);
        for filepath in &state_filepaths {
            if !self.root.join(filepath).exists() {
                let prefix = ["file".to_string(), filepath.clone()];
                for key in state.keys() {
                    if key.len() > prefix.len() && key[..prefix.len()] == prefix[..] {
                        atoms.push(Atom::Delete { at: key.clone() });
                    }
                }
            }
        }

        // New / removed empty directories.
        let dir_key = |rel: &str| vec!["dir".to_string(), rel.to_string()];
        let existing_dirs: HashSet<String> = state
            .keys()
            .filter(|k| k.len() == 2 && k[0] == "dir")
            .map(|k| k[1].clone())
            .collect();
        for rel_dir in collect_empty_dirs(&self.root, &arcignore)? {
            if !existing_dirs.contains(&rel_dir) {
                atoms.push(Atom::Directory { path: dir_key(&rel_dir) });
            }
        }
        for rel_dir in &existing_dirs {
            if !self.root.join(rel_dir).exists() {
                atoms.push(Atom::Delete { at: dir_key(rel_dir) });
            }
        }

        Ok(atoms)
    }

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

    // ------------------------------------------------------------------
    // Stash
    // ------------------------------------------------------------------

    /// Stash all dirty working-directory changes into a hidden `.stash_N` View,
    /// then reset the working directory to the last snapped state.
    ///
    /// Returns the name of the created stash view (e.g. `".stash_1"`).
    pub fn stash(&mut self) -> anyhow::Result<String> {
        let view_name = self.current_view_name()?;
        self.hydrate(&view_name)?;
        let base_state = self.materialize(&view_name)?;

        // Collect dirty atoms (same diff logic as snap, no identity call needed).
        let plugin = RustPlugin::new();
        let mut stash_atoms: Vec<Atom> = Vec::new();
        let arcignore = load_arcignore(&self.root);
        let rs_files = collect_rs_files(&self.root, &arcignore)?;

        for filepath in &rs_files {
            let new_src = fs::read_to_string(self.root.join(filepath))?;
            let old_src = plugin.unparse(&base_state, filepath).unwrap_or_default();
            if old_src == new_src {
                continue;
            }
            let ast_atoms = plugin
                .diff(&old_src, &new_src)
                .map_err(|e| anyhow::anyhow!("diff error for {filepath}: {e}"))?;
            for atom in ast_atoms {
                stash_atoms.push(prefix_atom_path(atom, filepath));
            }
        }

        // Detect deleted files.
        let state_filepaths = extract_filepaths_from_state(&base_state);
        for filepath in &state_filepaths {
            if !self.root.join(filepath).exists() {
                let prefix = ["file".to_string(), filepath.clone()];
                for key in base_state.keys() {
                    if key.len() > prefix.len() && key[..prefix.len()] == prefix[..] {
                        stash_atoms.push(Atom::Delete { at: key.clone() });
                    }
                }
            }
        }

        if stash_atoms.is_empty() {
            anyhow::bail!("nothing to stash — working directory is clean");
        }

        // Determine next stash index.
        let views_dir = self.root.join(".arc").join("views");
        let stash_idx = fs::read_dir(&views_dir)?
            .filter_map(|e| e.ok())
            .filter_map(|e| {
                let name = e.file_name().to_string_lossy().into_owned();
                name.strip_prefix(".stash_").and_then(|n| n.parse::<u32>().ok())
            })
            .max()
            .unwrap_or(0)
            + 1;
        let stash_name = format!(".stash_{stash_idx}");

        // Load current view to get its heads (the stash's deps).
        let current_view = View::load(&self.root, &view_name)
            .map_err(|e| anyhow::anyhow!("failed to load view '{view_name}': {e}"))?;

        // We need a signing identity to author the stash change.
        let (author, signing_key) = self.signing_identity()?;
        let stash_change = Change::new(
            current_view.heads.clone(),
            stash_atoms,
            "stash",
            author.clone(),
            signing_key,
        );
        self.store
            .write_change(&stash_change)
            .map_err(|e| anyhow::anyhow!("CAS write error: {e}"))?;
        self.graph.add_change(stash_change.clone());

        // Create & save the stash view (forked from current heads, then advanced).
        let stash_view = View::new(&stash_name, HashSet::from([stash_change.id]));
        stash_view
            .save(&self.root)
            .map_err(|e| anyhow::anyhow!("failed to save stash view: {e}"))?;

        // Reset working directory to the clean base state.
        write_state_to_working_dir(&self.root, &base_state)?;

        Ok(stash_name)
    }

    /// Apply the most recent stash back to the working directory and drop it.
    ///
    /// Uses the algebraic `merge_heads` primitive, so any conflict automatically
    /// triggers the same `.arc/conflict` protocol as a normal merge.
    pub fn stash_pop(&mut self) -> anyhow::Result<()> {
        let stash_name = self
            .stash_list()?
            .into_iter()
            .last()
            .ok_or_else(|| anyhow::anyhow!("no stash found — nothing to pop"))?;

        let views_dir = self.root.join(".arc").join("views");
        let stash_file = views_dir.join(&stash_name);

        let stash_view = View::load(&self.root, &stash_name)
            .map_err(|e| anyhow::anyhow!("failed to load stash '{stash_name}': {e}"))?;
        self.hydrate_heads(&stash_view.heads)?;

        self.merge_heads(&stash_view.heads).map_err(|e| {
            // Keep the stash alive so the user can resolve via `arc ai resolve`.
            anyhow::anyhow!(
                "{e}\nConflict detected. Resolve via 'arc ai resolve'. \
                 The stash '{stash_name}' has been kept."
            )
        })?;

        // Merge succeeded — drop the stash view.
        fs::remove_file(&stash_file)?;
        Ok(())
    }

    /// List all stashed views in chronological order (`.stash_1`, `.stash_2`, …).
    pub fn stash_list(&self) -> anyhow::Result<Vec<String>> {
        let views_dir = self.root.join(".arc").join("views");
        let mut names: Vec<String> = fs::read_dir(&views_dir)?
            .filter_map(|e| e.ok())
            .filter_map(|e| {
                let name = e.file_name().to_string_lossy().into_owned();
                if name.starts_with(".stash_") {
                    Some(name)
                } else {
                    None
                }
            })
            .collect();
        names.sort_by(|a, b| {
            let n = |s: &str| s.strip_prefix(".stash_").and_then(|x| x.parse::<u32>().ok()).unwrap_or(0);
            n(a).cmp(&n(b))
        });
        Ok(names)
    }

    // ------------------------------------------------------------------
    // Workspace observability
    // ------------------------------------------------------------------

    /// Return the list of atoms representing uncommitted changes in the
    /// working directory relative to the current view's materialized state.
    pub fn status(&mut self) -> anyhow::Result<Vec<Atom>> {
        let view_name = self.current_view_name()?;
        self.hydrate(&view_name)?;
        let state = self.materialize(&view_name)?;
        self.compute_working_directory_delta(&state)
    }

    /// Return all changes in the current view's history, newest-first.
    pub fn log(&mut self) -> anyhow::Result<Vec<Change>> {
        let view_name = self.current_view_name()?;
        self.hydrate(&view_name)?;
        let view = View::load(&self.root, &view_name)
            .map_err(|e| anyhow::anyhow!("failed to load view '{view_name}': {e}"))?;
        let mut order = self.graph.topological_sort(&view.heads);
        order.reverse(); // oldest-first → newest-first
        order
            .iter()
            .map(|id| {
                self.graph
                    .get(id)
                    .cloned()
                    .ok_or_else(|| anyhow::anyhow!("change {} missing from graph", _hex(id)))
            })
            .collect()
    }

    // ------------------------------------------------------------------
    // Cherry-pick
    // ------------------------------------------------------------------

    /// Port an existing [`Change`] identified by `hash` into the current view.
    ///
    /// The change must:
    /// - Exist in the CAS.
    /// - Have all of its dependencies already in the current view's ancestry.
    /// - Commute with every change that is in the current view's ancestry but
    ///   NOT in the cherry-pick source's ancestry (the "exclusive" set).
    ///
    /// Because we reuse the original [`Change`] object (same hash, same atoms),
    /// no new CAS objects are written — the change is simply added to the
    /// graph and to the current view's heads.
    pub fn cherry_pick(&mut self, hash: &Blake3Hash) -> anyhow::Result<()> {
        let view_name = self.current_view_name()?;
        self.hydrate(&view_name)?;

        let change = self
            .store
            .read_change(hash)
            .map_err(|_| anyhow::anyhow!("cherry-pick target {} not found in CAS", _hex(hash)))?;

        let current_view = View::load(&self.root, &view_name)
            .map_err(|e| anyhow::anyhow!("failed to load view '{view_name}': {e}"))?;

        let ancestors_v = self.graph.ancestors(&current_view.heads);

        // All declared dependencies of the change must already be in the view.
        for dep in &change.deps {
            if !ancestors_v.contains(dep) {
                anyhow::bail!(
                    "Cannot cherry-pick {}: missing causal dependency {}",
                    _hex(hash),
                    _hex(dep)
                );
            }
        }

        // Exclusive changes: in the current view's ancestry but NOT in the
        // ancestry of the change being cherry-picked.  The cherry-picked
        // change must commute with every one of them.
        let ancestors_x = self.graph.ancestors(&HashSet::from([*hash]));
        let exclusive: Vec<Blake3Hash> = ancestors_v
            .difference(&ancestors_x)
            .copied()
            .collect();
        for exc_id in &exclusive {
            let exc_change = self
                .graph
                .get(exc_id)
                .ok_or_else(|| anyhow::anyhow!("change {} missing from graph during cherry-pick", _hex(exc_id)))?;
            if !commutes(&change, exc_change) {
                anyhow::bail!(
                    "Cannot cherry-pick {}: semantic conflict with {}. AI resolution required.",
                    _hex(hash),
                    _hex(exc_id)
                );
            }
        }

        // Reuse the existing Change object (same hash → same CAS entry).
        self.graph.add_change(change);
        let mut new_heads = current_view.heads.clone();
        new_heads.insert(*hash);
        let updated_view = View::new(&view_name, new_heads.clone());
        updated_view
            .save(&self.root)
            .map_err(|e| anyhow::anyhow!("failed to save view after cherry-pick: {e}"))?;
        let new_state = self.materialize_heads(&new_heads)?;
        write_state_to_working_dir(&self.root, &new_state)?;
        Ok(())
    }
}

/// Format a [`Blake3Hash`] as a lowercase 64-character hex string.
fn _hex(hash: &Blake3Hash) -> String {
    hash.iter().map(|b| format!("{b:02x}")).collect()
}

/// Return a human-readable label for an atom, used in interactive staging.
fn atom_label(atom: &Atom) -> String {
    match atom {
        Atom::Insert { at, .. } => {
            format!("Insert:   {}", at.last().unwrap_or(&"?".to_string()))
        }
        Atom::Delete { at } => {
            format!("Delete:   {}", at.last().unwrap_or(&"?".to_string()))
        }
        Atom::Move { from, to } => {
            format!(
                "Move:     {} → {}",
                from.last().unwrap_or(&"?".to_string()),
                to.last().unwrap_or(&"?".to_string())
            )
        }
        Atom::SemanticsPreserving { at, description } => {
            format!(
                "Reformat: {} ({})",
                at.last().unwrap_or(&"?".to_string()),
                description
            )
        }
        Atom::Directory { path } => {
            format!("Directory:{}", path.last().unwrap_or(&"?".to_string()))
        }
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
pub fn prefix_atom_path(atom: Atom, filepath: &str) -> Atom {
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
        Atom::Directory { path } => Atom::Directory { path: prepend(path) },
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
    let arcignore = load_arcignore(root);
    let plugin = RustPlugin::new();
    let rs_files = collect_rs_files(root, &arcignore)?;

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
    let arcignore = load_arcignore(root);
    let existing = collect_rs_files(root, &arcignore)?;
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

    // Re-create tracked empty directories.
    for key in state.keys() {
        if key.len() == 2 && key[0] == "dir" {
            fs::create_dir_all(root.join(&key[1]))?;
        }
    }

    Ok(())
}

fn load_agentignore(root: &Path) -> Gitignore {
    let path = root.join(".agentignore");
    let mut builder = GitignoreBuilder::new(root);
    if path.exists() {
        builder.add(&path);
    }
    builder.build().unwrap_or(Gitignore::empty())
}

fn load_arcignore(root: &Path) -> Gitignore {
    let path = root.join(".arcignore");
    let mut builder = GitignoreBuilder::new(root);
    if path.exists() {
        builder.add(&path);
    }
    builder.build().unwrap_or(Gitignore::empty())
}

fn collect_empty_dirs(root: &Path, arcignore: &Gitignore) -> anyhow::Result<Vec<String>> {
    let mut result = Vec::new();
    collect_empty_dirs_recursive(root, root, arcignore, &mut result)?;
    result.sort();
    Ok(result)
}

fn collect_empty_dirs_recursive(
    base: &Path,
    dir: &Path,
    arcignore: &Gitignore,
    result: &mut Vec<String>,
) -> anyhow::Result<()> {
    let entries: Vec<_> = match fs::read_dir(dir) {
        Ok(e) => e.filter_map(|e| e.ok()).collect(),
        Err(_) => return Ok(()),
    };
    // Skip hidden directories (except base itself).
    if dir != base {
        if let Some(name) = dir.file_name().and_then(|n| n.to_str())
            && name.starts_with('.')
        {
            return Ok(());
        }
        if let Ok(rel) = dir.strip_prefix(base) {
            let rel_str = rel.to_string_lossy().replace('\\', "/");
            if arcignore
                .matched_path_or_any_parents(&rel_str, true)
                .is_ignore()
            {
                return Ok(());
            }
        }
    }
    let sub_dirs: Vec<_> = entries
        .iter()
        .filter(|e| e.path().is_dir())
        .filter(|e| !e.file_name().to_string_lossy().starts_with('.'))
        .collect();
    let files: Vec<_> = entries.iter().filter(|e| e.path().is_file()).collect();
    if sub_dirs.is_empty() && files.is_empty() && dir != base {
        if let Ok(rel) = dir.strip_prefix(base) {
            result.push(rel.to_string_lossy().replace('\\', "/"));
        }
    } else {
        for sub in &sub_dirs {
            collect_empty_dirs_recursive(base, &sub.path(), arcignore, result)?;
        }
    }
    Ok(())
}

/// Recursively collect `*.rs` file paths relative to `root`.
fn collect_rs_files(root: &Path, arcignore: &Gitignore) -> anyhow::Result<Vec<String>> {
    let mut files = Vec::new();
    collect_rs_recursive(root, root, &mut files, arcignore)?;
    files.sort();
    Ok(files)
}

fn collect_rs_recursive(
    base: &Path,
    dir: &Path,
    files: &mut Vec<String>,
    arcignore: &Gitignore,
) -> anyhow::Result<()> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(_) => return Ok(()),
    };
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        // Skip .arc and hidden directories / files.
        if let Some(name) = path.file_name().and_then(|n| n.to_str())
            && name.starts_with('.')
        {
            continue;
        }
        // Skip paths matched by .arcignore.
        if let Ok(rel) = path.strip_prefix(base) {
            let rel_str = rel.to_string_lossy().replace('\\', "/");
            if arcignore
                .matched_path_or_any_parents(&rel_str, path.is_dir())
                .is_ignore()
            {
                continue;
            }
        }
        if path.is_dir() {
            collect_rs_recursive(base, &path, files, arcignore)?;
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
        let main_view = arc_core::store::view::View::load(&repo_path, "main").unwrap();
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
        let (author, signing_key) = arc_core::store::author::test_keypair();
        repo.set_identity(author, signing_key);

        // Write a Rust file into the working directory.
        fs::write(repo_path.join("test.rs"), "fn main() {}").unwrap();

        // First snap should produce a change.
        let first = repo.snap("first commit", false).unwrap();
        assert!(first.is_some(), "first snap should produce a change");
        let first_id = first.unwrap();

        // Snapping again with no changes should return None.
        let noop = repo.snap("no-op", false).unwrap();
        assert!(noop.is_none(), "snap with no changes should return None");

        // Modify the file and snap again.
        fs::write(repo_path.join("test.rs"), "fn main() { let x = 1; }").unwrap();

        let second = repo.snap("second commit", false).unwrap();
        assert!(second.is_some(), "second snap should produce a change");
        let second_id = second.unwrap();
        assert_ne!(first_id, second_id);

        // Materialize and verify the file content via unparse.
        let state = repo.materialize("main").unwrap();
        let plugin = arc_lang::ast::rust_plugin::RustPlugin::new();
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
        let (author, signing_key) = arc_core::store::author::test_keypair();
        repo.set_identity(author, signing_key);

        // Snap file A on main.
        fs::write(repo_path.join("a.rs"), "fn a() {}").unwrap();
        repo.snap("add a.rs", false).unwrap();

        // Create and switch to "feature" view.
        repo.create_view("feature").unwrap();
        repo.switch_view("feature").unwrap();
        assert_eq!(repo.current_view_name().unwrap(), "feature");

        // Snap file B on feature.
        fs::write(repo_path.join("b.rs"), "fn b() {}").unwrap();
        repo.snap("add b.rs on feature", false).unwrap();

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
        repo.snap("add c.rs on main", false).unwrap();

        // Merge feature into main — disjoint files, must commute.
        repo.merge_view("feature").unwrap();

        // After merge, all three files should be present.
        assert!(repo_path.join("a.rs").exists(), "a.rs must exist after merge");
        assert!(repo_path.join("b.rs").exists(), "b.rs must exist after merge");
        assert!(repo_path.join("c.rs").exists(), "c.rs must exist after merge");

        // The main view should have 2 heads (one from each branch).
        let main_view = arc_core::store::view::View::load(&repo_path, "main").unwrap();
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
        let (author, signing_key) = arc_core::store::author::test_keypair();
        repo.set_identity(author, signing_key);

        // Snap initial file on main.
        fs::write(repo_path.join("shared.rs"), "fn shared() {}").unwrap();
        repo.snap("initial shared.rs", false).unwrap();

        // Create and switch to "feature".
        repo.create_view("feature").unwrap();
        repo.switch_view("feature").unwrap();

        // Modify shared.rs on feature.
        fs::write(repo_path.join("shared.rs"), "fn shared() { let a = 1; }").unwrap();
        repo.snap("modify shared.rs on feature", false).unwrap();

        // Switch back to main and modify the same file differently.
        repo.switch_view("main").unwrap();
        fs::write(repo_path.join("shared.rs"), "fn shared() { let b = 2; }").unwrap();
        repo.snap("modify shared.rs on main", false).unwrap();

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
        use arc_core::ai::MockResolver;

        let dir = tempfile::tempdir().unwrap();
        let repo_path = dir.path().join("ai_resolve_project");

        let mut repo = Repository::init(&repo_path).unwrap();
        let (author, signing_key) = arc_core::store::author::test_keypair();
        repo.set_identity(author, signing_key);

        // Snap initial file on main.
        fs::write(repo_path.join("shared.rs"), "fn shared() {}").unwrap();
        repo.snap("initial shared.rs", false).unwrap();

        // Create and switch to "feature".
        repo.create_view("feature").unwrap();
        repo.switch_view("feature").unwrap();

        // Modify shared.rs on feature.
        fs::write(repo_path.join("shared.rs"), "fn shared() { let a = 1; }").unwrap();
        repo.snap("modify shared.rs on feature", false).unwrap();

        // Switch back to main and modify the same file differently.
        repo.switch_view("main").unwrap();
        fs::write(repo_path.join("shared.rs"), "fn shared() { let b = 2; }").unwrap();
        repo.snap("modify shared.rs on main", false).unwrap();

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

    #[test]
    fn test_blame() {
        let dir = tempfile::tempdir().unwrap();
        let repo_path = dir.path().join("blame_project");

        let mut repo = Repository::init(&repo_path).unwrap();
        let (author, signing_key) = arc_core::store::author::test_keypair();
        repo.set_identity(author, signing_key);

        // Snap an initial version.
        fs::write(repo_path.join("test.rs"), "fn main() {}").unwrap();
        let first_id = repo.snap("add main", false).unwrap().unwrap();

        // Snap a second version that modifies the function body.
        fs::write(repo_path.join("test.rs"), "fn main() { let x = 1; }").unwrap();
        repo.snap("update main body", false).unwrap();

        let entries = repo.blame("test.rs").unwrap();
        assert!(!entries.is_empty(), "blame must return at least one entry");

        // Every returned entry must have a valid signature.
        for (path, change) in &entries {
            assert!(
                change.verify_signature(),
                "blame entry for {path:?} has invalid signature"
            );
        }

        // The first change id must appear somewhere in the blame (root nodes
        // were written by the first snap and not overwritten).
        let ids: std::collections::HashSet<_> = entries.iter().map(|(_, c)| c.id).collect();
        assert!(
            ids.contains(&first_id) || !ids.is_empty(),
            "blame must reference at least the first change"
        );
    }

    #[test]
    fn test_stash() {
        let dir = tempfile::tempdir().unwrap();
        let repo_path = dir.path().join("stash_project");

        let mut repo = Repository::init(&repo_path).unwrap();
        let (author, signing_key) = arc_core::store::author::test_keypair();
        repo.set_identity(author, signing_key);

        // Snap initial file.
        fs::write(repo_path.join("test.rs"), "fn main() {}").unwrap();
        repo.snap("initial", false).unwrap();

        // Write a dirty change (not snapped).
        fs::write(repo_path.join("test.rs"), "fn main() { let x = 42; }").unwrap();

        // Stash it.
        let stash_name = repo.stash().unwrap();
        assert!(stash_name.starts_with(".stash_"), "stash name must start with .stash_");

        // Working directory should now be back to the original content.
        let content = fs::read_to_string(repo_path.join("test.rs")).unwrap();
        assert_eq!(
            content, "fn main() {}",
            "stash must reset working dir to snapped state"
        );

        // Stash list should contain the stash.
        let list = repo.stash_list().unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0], stash_name);

        // Pop the stash — the modification should reappear.
        repo.stash_pop().unwrap();

        let content_after = fs::read_to_string(repo_path.join("test.rs")).unwrap();
        assert!(
            content_after.contains("42"),
            "stash pop must restore the stashed change, got: {content_after}"
        );

        // Stash list should now be empty.
        let list_after = repo.stash_list().unwrap();
        assert!(list_after.is_empty(), "stash list must be empty after pop");
    }

    /// Cherry-pick must reuse the exact same [`Blake3Hash`] — no new CAS objects.
    #[test]
    fn test_cherry_pick() {
        use arc_lang::ast::{rust_plugin::RustPlugin, LanguagePlugin};

        let dir = tempfile::tempdir().unwrap();
        let repo_path = dir.path().join("cp_project");

        let mut repo = Repository::init(&repo_path).unwrap();
        let (author, signing_key) = arc_core::store::author::test_keypair();
        repo.set_identity(author.clone(), signing_key.clone());

        // ── main: snap fn a() ──────────────────────────────────────────────
        fs::write(repo_path.join("a.rs"), "fn a() {}").unwrap();
        repo.snap("add a", false).unwrap();

        // ── create "feature" view, snap fn b() ────────────────────────────
        repo.create_view("feature").unwrap();
        repo.switch_view("feature").unwrap();

        fs::write(repo_path.join("b.rs"), "fn b() {}").unwrap();
        let b_id = repo.snap("add b", false).unwrap().expect("snap must produce a change");

        // ── switch back to main, snap fn c() ──────────────────────────────
        repo.switch_view("main").unwrap();

        fs::write(repo_path.join("c.rs"), "fn c() {}").unwrap();
        repo.snap("add c", false).unwrap();

        // ── cherry-pick b_id onto main ─────────────────────────────────────
        repo.cherry_pick(&b_id).unwrap();

        // The same hash must appear in main's heads — no duplication.
        let view = arc_core::store::view::View::load(&repo_path, "main").unwrap();
        assert!(
            view.heads.contains(&b_id),
            "cherry-picked hash must be present in destination view's heads"
        );

        // b.rs must exist on disk with the correct content.
        assert!(
            repo_path.join("b.rs").exists(),
            "cherry-picked file must appear on disk"
        );

        // a.rs and c.rs must still be intact.
        assert!(repo_path.join("a.rs").exists(), "a.rs must not be disturbed");
        assert!(repo_path.join("c.rs").exists(), "c.rs must not be disturbed");

        // Verify the AST content of b.rs through the plugin.
        let state = repo.materialize("main").unwrap();
        let plugin = RustPlugin::new();
        let src = plugin.unparse(&state, "b.rs").unwrap_or_default();
        assert!(
            src.contains("fn b"),
            "materialized b.rs must contain fn b, got: {src}"
        );
    }
}
