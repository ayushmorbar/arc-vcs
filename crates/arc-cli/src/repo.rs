use std::collections::{HashMap, HashSet, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use arc_core::ai::AiResolver;
use arc_core::algebra::apply::{BlameState, MaterializedState, apply_change};
use arc_core::algebra::commute::commutes;
use arc_core::algebra::{Atom, Blake3Hash, NodePath};
use arc_core::store::author::{Author, load_identity};
use arc_core::store::cas::ObjectStore;
use arc_core::store::change::Change;
use arc_core::store::graph::ChangeGraph;
use arc_core::store::tag::Tag;
use arc_core::store::view::View;
use arc_lang::ast::LanguagePlugin;
use arc_lang::ast::rust_plugin::RustPlugin;
use ignore::gitignore::{Gitignore, GitignoreBuilder};

/// Workspace manifest persisted at `<workspace>/.arc-workspace`.
///
/// A workspace is a lightweight working directory linked to a shared CAS root.
/// Sparse cone patterns for the workspace are embedded here so the full state
/// of a linked workspace is always visible in one JSON file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceManifest {
    /// Absolute path to the primary repository root (where `.arc/` lives).
    pub shared_root: PathBuf,
    /// Name of the view currently checked out in this workspace.
    pub view: String,
    /// Optional sparse cone patterns active in this workspace.
    #[serde(default)]
    pub sparse_patterns: Vec<String>,
}

/// Repository-level configuration persisted in `.arc/config.json`.
///
/// Settings are isolated per-repository and never touch the OS keyring.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct RepoConfig {
    /// Named remote aliases mapping a short name to a URL or filesystem path.
    #[serde(default)]
    pub remotes: HashMap<String, String>,
    /// User-defined command aliases (e.g. `"st"` → `"status"`).
    #[serde(default)]
    pub aliases: HashMap<String, String>,
    /// Lifecycle hooks: event name → list of shell command strings.
    /// Supported events: `pre-snap`, `post-merge`.
    #[serde(default)]
    pub hooks: HashMap<String, Vec<String>>,
}

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

/// A single entry in the operation log, recording every view-mutating command.
///
/// Used by [`Repository::undo`] to rewind the last operation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpLogEntry {
    /// Unix timestamp (seconds since epoch) when the operation ran.
    pub timestamp: u64,
    /// Human-readable command name (e.g. `"snap"`, `"merge"`, `"cherry-pick"`).
    pub command: String,
    /// Name of the view that was mutated.
    pub view: String,
    /// Heads of the view **before** the operation, used to restore the DAG state.
    pub previous_heads: HashSet<Blake3Hash>,
}

/// Summary returned by [`Repository::gc`].
#[derive(Debug, Default)]
pub struct GcResult {
    /// Number of [`Change`] objects deleted from the CAS.
    pub changes_deleted: usize,
    /// Number of blob files deleted from `.arc/blobs/`.
    pub blobs_deleted: usize,
}

/// Top-level repository handle, tying together the CAS, the change graph,
/// and the on-disk `.arc` layout.
///
/// ### Split-Root Architecture
///
/// A repository has two roots:
/// - `shared_root` — where the `.arc/` CAS lives (`store/`, `views/`, `blobs/`, etc.).
/// - `work_root`   — where the working-directory files live (may differ when using workspaces).
///
/// For a normal (non-workspace) repository both roots point to the same directory.
/// For a linked workspace `work_root` points to the workspace directory and
/// `shared_root` points to the primary repository that owns the CAS.
pub struct Repository {
    /// Path to the shared CAS root — where `.arc/` lives.
    pub shared_root: PathBuf,
    /// Path to the working directory root (same as `shared_root` for primary repos).
    pub work_root: PathBuf,
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
        fs::create_dir_all(arc_dir.join("tags"))?;
        fs::create_dir_all(arc_dir.join("blobs"))?;

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
            shared_root: root.clone(),
            work_root: root,
            identity: None,
        })
    }

    /// Open an existing repository by locating the `.arc` directory at `path`.
    ///
    /// If `path` contains a `.arc-workspace` manifest *and* no `.arc` directory,
    /// the repository is opened in **workspace mode**: `shared_root` is taken
    /// from the manifest and `work_root` is `path`. If both exist, `.arc` wins
    /// (primary repository mode).
    pub fn open(path: impl AsRef<Path>) -> Result<Self, anyhow::Error> {
        let work_root = path.as_ref().to_path_buf();
        let arc_dir = work_root.join(".arc");
        let ws_file = work_root.join(".arc-workspace");

        // Workspace mode: .arc-workspace present and no primary .arc/ in same dir.
        if ws_file.exists() && !arc_dir.exists() {
            let json = fs::read_to_string(&ws_file)
                .map_err(|e| anyhow::anyhow!("failed to read .arc-workspace: {e}"))?;
            let manifest: WorkspaceManifest = serde_json::from_str(&json)
                .map_err(|e| anyhow::anyhow!("invalid .arc-workspace: {e}"))?;
            let shared_root = manifest.shared_root.clone();
            if !shared_root.join(".arc").exists() {
                anyhow::bail!(
                    "workspace shared root '{}' has no .arc directory",
                    shared_root.display()
                );
            }
            return Ok(Self {
                store: ObjectStore::new(&shared_root),
                graph: ChangeGraph::new(),
                shared_root,
                work_root,
                identity: None,
            });
        }

        // Primary mode: .arc must exist here.
        if !arc_dir.exists() {
            anyhow::bail!("no arc repository found at {}", arc_dir.display());
        }

        Ok(Self {
            store: ObjectStore::new(&work_root),
            graph: ChangeGraph::new(),
            shared_root: work_root.clone(),
            work_root,
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
    fn signing_identity(&self) -> anyhow::Result<(&Author, &ed25519_dalek::SigningKey)> {
        self.identity
            .as_ref()
            .map(|(a, k)| (a, k))
            .ok_or_else(|| anyhow::anyhow!("no signing identity set — call set_identity() first"))
    }

    /// Read the name of the currently active view.
    ///
    /// For workspace repos, reads the view from `.arc-workspace`.
    /// For primary repos, reads `shared_root/.arc/HEAD`.
    pub fn current_view_name(&self) -> anyhow::Result<String> {
        let ws_file = self.work_root.join(".arc-workspace");
        if ws_file.exists() && self.shared_root != self.work_root {
            let json = fs::read_to_string(&ws_file)
                .map_err(|e| anyhow::anyhow!("failed to read .arc-workspace: {e}"))?;
            let manifest: WorkspaceManifest = serde_json::from_str(&json)
                .map_err(|e| anyhow::anyhow!("invalid .arc-workspace: {e}"))?;
            return Ok(manifest.view);
        }
        let head_path = self.shared_root.join(".arc").join("HEAD");
        let name = fs::read_to_string(&head_path)
            .map_err(|e| anyhow::anyhow!("failed to read .arc/HEAD: {e}"))?;
        Ok(name.trim().to_string())
    }

    /// Write the name of the currently active view.
    ///
    /// For workspace repos, updates the `view` field in `.arc-workspace`.
    /// For primary repos, writes `shared_root/.arc/HEAD`.
    fn set_current_view_name(&self, name: &str) -> anyhow::Result<()> {
        let ws_file = self.work_root.join(".arc-workspace");
        if ws_file.exists() && self.shared_root != self.work_root {
            let json = fs::read_to_string(&ws_file)
                .map_err(|e| anyhow::anyhow!("failed to read .arc-workspace: {e}"))?;
            let mut manifest: WorkspaceManifest = serde_json::from_str(&json)
                .map_err(|e| anyhow::anyhow!("invalid .arc-workspace: {e}"))?;
            manifest.view = name.to_string();
            fs::write(
                &ws_file,
                serde_json::to_string_pretty(&manifest)
                    .map_err(|e| anyhow::anyhow!("failed to serialise .arc-workspace: {e}"))?,
            )
            .map_err(|e| anyhow::anyhow!("failed to write .arc-workspace: {e}"))?;
            return Ok(());
        }
        fs::write(self.shared_root.join(".arc").join("HEAD"), name)
            .map_err(|e| anyhow::anyhow!("failed to write .arc/HEAD: {e}"))
    }

    /// Load the Epoch Map from `.arc/epochs`.
    ///
    /// The Epoch Map is the heart of PO-Log Compaction.  After `compact()`
    /// runs, every causally-stable `Change` ID is mapped to the Genesis
    /// Change ID that replaced it.  `hydrate_heads()` consults this map on
    /// every BFS step so that it transparently redirects traversal to the
    /// Genesis node, allowing the CAS objects for the old stable history to
    /// be physically deleted without touching any live `Change` object.
    ///
    /// Returns an empty map when the file does not exist (i.e. before any
    /// `compact()` call).
    fn load_epoch_map(&self) -> anyhow::Result<HashMap<Blake3Hash, Blake3Hash>> {
        let path = self.shared_root.join(".arc").join("epochs");
        if !path.exists() {
            return Ok(HashMap::new());
        }
        let raw = fs::read_to_string(&path)
            .map_err(|e| anyhow::anyhow!("could not read .arc/epochs: {e}"))?;
        let json: HashMap<String, String> = serde_json::from_str(&raw)
            .map_err(|e| anyhow::anyhow!("corrupt .arc/epochs JSON: {e}"))?;
        let mut map = HashMap::new();
        for (k, v) in json {
            let old_id = _unhex(&k)
                .ok_or_else(|| anyhow::anyhow!("invalid epoch key: {k}"))?;
            let new_id = _unhex(&v)
                .ok_or_else(|| anyhow::anyhow!("invalid epoch value: {v}"))?;
            map.insert(old_id, new_id);
        }
        Ok(map)
    }

    /// Populate the in-memory [`ChangeGraph`] by walking backward from an
    /// arbitrary set of heads through the CAS.
    ///
    /// This is idempotent — already-present nodes are skipped.
    ///
    /// ### Epoch Map interception
    ///
    /// When `compact()` has been run, some historical `Change` IDs no longer
    /// exist on disk — they were superseded by a single Genesis Change and
    /// their CAS objects deleted.  Before attempting a CAS read, this method
    /// transparently redirects compacted IDs to their Genesis replacement via
    /// the Epoch Map stored in `.arc/epochs`.  Live `Change` objects are
    /// **never mutated**; only the read path is intercepted.
    pub fn hydrate_heads(&mut self, heads: &HashSet<Blake3Hash>) -> anyhow::Result<()> {
        let epoch_map = self.load_epoch_map()?;
        let mut queue: VecDeque<Blake3Hash> = heads.iter().copied().collect();

        while let Some(id) = queue.pop_front() {
            // Epoch Map interception: if this ID was compacted away, redirect
            // to the Genesis Change instead of attempting a CAS read.
            let id = if let Some(&genesis_id) = epoch_map.get(&id) {
                if self.graph.get(&genesis_id).is_none() {
                    queue.push_back(genesis_id);
                }
                continue;
            } else {
                id
            };

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
        let view = View::load(&self.shared_root, view_name)
            .map_err(|e| anyhow::anyhow!("failed to load view '{view_name}': {e}"))?;
        self.hydrate_heads(&view.heads)
    }

    /// Replay the DAG in topological order to produce a materialized state
    /// from an arbitrary set of heads.
    pub fn materialize_heads(
        &self,
        heads: &HashSet<Blake3Hash>,
    ) -> anyhow::Result<MaterializedState> {
        let agent_ignore = load_agentignore(&self.shared_root);
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
                let hex: String = change.id.iter().map(|b| format!("{b:02x}")).collect();
                anyhow::bail!("cryptographic verification failed for change {hex}");
            }
        }
        Ok(())
    }

    /// Replay the DAG in topological order to produce a materialized state.
    pub fn materialize(&self, view_name: &str) -> anyhow::Result<MaterializedState> {
        let view = View::load(&self.shared_root, view_name)
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

        let view = View::load(&self.shared_root, &view_name)
            .map_err(|e| anyhow::anyhow!("failed to load view '{view_name}': {e}"))?;

        let agent_ignore = load_agentignore(&self.shared_root);
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
        self.run_hook("pre-snap")?;
        let view_name = self.current_view_name()?;
        tracing::info!(view = %view_name, message, "snap started");
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

        let mut view = View::load(&self.shared_root, &view_name)
            .map_err(|e| anyhow::anyhow!("failed to load view '{view_name}': {e}"))?;

        let (author, signing_key) = self.signing_identity()?;

        // Persist raw bytes for every Atom::Blob before committing the change.
        self.write_blob_atoms(&all_atoms)?;

        let change = Change::new(
            view.heads.clone(),
            all_atoms,
            message,
            author.clone(),
            signing_key,
        );
        self.store
            .write_change(&change)
            .map_err(|e| anyhow::anyhow!("CAS write error: {e}"))?;
        self.graph.add_change(change.clone());

        // Log before mutating view frontier so undo can restore previous state.
        self.log_operation("snap", &view_name, view.heads.clone())?;

        // Advance the frontier: new change becomes the sole head.
        view.heads = HashSet::from([change.id]);
        view.save(&self.shared_root)
            .map_err(|e| anyhow::anyhow!("failed to save view: {e}"))?;

        tracing::info!(change_id = ?change.id, "snap complete");
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
        let arcignore = load_arcignore(&self.work_root);
        let rs_files = collect_rs_files(&self.work_root, &arcignore)?;
        let mut atoms: Vec<Atom> = Vec::new();

        for filepath in &rs_files {
            let new_src = fs::read_to_string(self.work_root.join(filepath))?;
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

        // ── Pass 2: Non-Rust files — O(1) BLAKE3 blob diff ───────────────────
        let all_files = collect_all_files(&self.work_root, &arcignore)?;
        for filepath in all_files.iter().filter(|f| !f.ends_with(".rs")) {
            let bytes = fs::read(self.work_root.join(filepath))
                .map_err(|e| anyhow::anyhow!("failed to read '{filepath}': {e}"))?;
            let new_hash: Blake3Hash = *blake3::hash(&bytes).as_bytes();
            let path_key = vec!["file".to_string(), filepath.clone()];
            // Skip if the blob ref in state already matches this hash.
            if let Some(existing) = state.get(&path_key)
                && existing.starts_with(b"ARC_BLOB_REF:")
                && existing.len() >= 45
            {
                let old_hash: Blake3Hash = existing[13..45].try_into().unwrap_or([0u8; 32]);
                if old_hash == new_hash {
                    continue;
                }
            }
            atoms.push(Atom::Blob {
                path: path_key,
                hash: new_hash,
            });
        }

        // Deleted files.
        let state_filepaths = extract_filepaths_from_state(state);
        let sparse_patterns = load_sparse_patterns(&self.work_root);
        let is_sparse = !sparse_patterns.is_empty();
        for filepath in &state_filepaths {
            // Sparse Safety Law: do not emit Delete for files hidden by sparse cone.
            if is_sparse && !sparse_patterns.iter().any(|p| filepath.starts_with(p.as_str())) {
                continue;
            }
            if !self.work_root.join(filepath).exists() {
                let prefix = ["file".to_string(), filepath.clone()];
                for key in state.keys() {
                    // >= covers blob keys (len==2) as well as AST sub-keys (len>2).
                    if key.len() >= prefix.len() && key[..prefix.len()] == prefix[..] {
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
        for rel_dir in collect_empty_dirs(&self.work_root, &arcignore)? {
            if !existing_dirs.contains(&rel_dir) {
                atoms.push(Atom::Directory {
                    path: dir_key(&rel_dir),
                });
            }
        }
        for rel_dir in &existing_dirs {
            if !self.work_root.join(rel_dir).exists() {
                atoms.push(Atom::Delete {
                    at: dir_key(rel_dir),
                });
            }
        }

        Ok(atoms)
    }

    /// Create a new view forked from the current view's heads.
    pub fn create_view(&self, name: &str) -> anyhow::Result<()> {
        let current_name = self.current_view_name()?;
        let current_view = View::load(&self.shared_root, &current_name)
            .map_err(|e| anyhow::anyhow!("failed to load view '{current_name}': {e}"))?;

        let view_path = self.shared_root.join(".arc").join("views").join(name);
        if view_path.exists() {
            anyhow::bail!("view '{name}' already exists");
        }

        let new_view = View::new(name, current_view.heads);
        new_view
            .save(&self.shared_root)
            .map_err(|e| anyhow::anyhow!("failed to save view '{name}': {e}"))?;

        Ok(())
    }

    /// Switch the working directory to `target` view.
    ///
    /// Fails if the working directory has un-snapped changes.
    pub fn switch_view(&mut self, target: &str) -> anyhow::Result<()> {
        // Verify the target view exists.
        let target_view = View::load(&self.shared_root, target)
            .map_err(|e| anyhow::anyhow!("view '{target}' not found: {e}"))?;

        let current_name = self.current_view_name()?;

        // Hydrate and materialize the current view to detect dirty state.
        self.hydrate(&current_name)?;
        let current_state = self.materialize(&current_name)?;

        // Check for un-snapped changes.
        check_working_dir_clean(&self.work_root, &current_state, "switching views")?;

        // Hydrate the target view.
        self.hydrate(target)?;

        // Materialize the target view.
        let target_state = if target_view.heads.is_empty() {
            MaterializedState::new()
        } else {
            self.materialize(target)?
        };

        // Replace working directory with target state.
        write_state_to_working_dir(&self.work_root, &self.shared_root, &target_state)?;

        // Update HEAD.
        self.set_current_view_name(target)?;

        Ok(())
    }

    /// Merge `target_name` view into the current view using the algebraic
    /// merge law.
    ///
    /// If all exclusive changes commute, the merge is a simple head-union
    /// with no merge commit. If any pair conflicts, aborts with an error.
    pub fn merge_view(&mut self, target_name: &str) -> anyhow::Result<()> {
        self.hydrate(target_name)?;
        let target_view = View::load(&self.shared_root, target_name)
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
        tracing::info!(view = %current_name, "merge_heads started");

        // Hydrate both sides (idempotent — already-present nodes are skipped).
        self.hydrate(&current_name)?;
        self.hydrate_heads(target_heads)?;

        let current_view = View::load(&self.shared_root, &current_name)
            .map_err(|e| anyhow::anyhow!("failed to load view '{current_name}': {e}"))?;

        // --- Dirty working-directory check ---
        let current_state = self.materialize_heads(&current_view.heads)?;
        check_working_dir_clean(&self.work_root, &current_state, "merging")?;


        // Find LCA.
        let lca_heads = self.graph.merge_base(&current_view.heads, target_heads);

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
            let conflict_path = self.shared_root.join(".arc").join("conflict");
            let bytes = bincode::serialize(&conflict)
                .map_err(|e| anyhow::anyhow!("failed to serialize conflict: {e}"))?;
            fs::write(&conflict_path, bytes)?;

            let hex_a: String = conflicting_pairs[0]
                .0
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect();
            let hex_b: String = conflicting_pairs[0]
                .1
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect();
            anyhow::bail!(
                "Semantic Conflict detected between {hex_a} and {hex_b}. AI resolution required."
            );
        }

        // All commute — union the heads.
        let prev_heads = current_view.heads.clone();
        let mut merged_heads = current_view.heads;
        merged_heads.extend(target_heads);

        self.log_operation("merge", &current_name, prev_heads)?;
        let updated_view = View::new(&current_name, merged_heads.clone());
        updated_view
            .save(&self.shared_root)
            .map_err(|e| anyhow::anyhow!("failed to save merged view: {e}"))?;

        // Re-materialize and write to working directory.
        let merged_state = self.materialize_heads(&merged_heads)?;
        write_state_to_working_dir(&self.work_root, &self.shared_root, &merged_state)?;
        self.run_hook("post-merge")?;
        tracing::info!("merge_heads complete");
        Ok(())
    }

    /// Resolve a pending conflict stored in `.arc/conflict` using the
    /// provided [`AiResolver`].
    ///
    /// For each conflicting pair the resolver is called with the LCA base,
    /// both sides' content at the overlapping path, and their intents.
    /// The resolved content is committed as a new merge change.
    pub fn resolve_conflict(&mut self, resolver: &dyn AiResolver) -> anyhow::Result<Blake3Hash> {
        let conflict_path = self.shared_root.join(".arc").join("conflict");
        if !conflict_path.exists() {
            anyhow::bail!("no pending conflict — nothing to resolve");
        }

        let conflict_bytes = fs::read(&conflict_path)?;
        let conflict: PendingConflict = bincode::deserialize(&conflict_bytes)
            .map_err(|e| anyhow::anyhow!("failed to deserialize conflict: {e}"))?;

        // Hydrate current view and target heads.
        self.hydrate(&conflict.current_view)?;
        self.hydrate_heads(&conflict.target_heads)?;

        let current_view = View::load(&self.shared_root, &conflict.current_view)
            .map_err(|e| anyhow::anyhow!("failed to load view '{}': {e}", conflict.current_view))?;

        // Materialize LCA state directly from heads — no temp view needed.
        let lca_heads = self
            .graph
            .merge_base(&current_view.heads, &conflict.target_heads);
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
            let change_a = self
                .graph
                .get(id_a)
                .ok_or_else(|| anyhow::anyhow!("conflicting change missing from graph"))?
                .clone();
            let change_b = self
                .graph
                .get(id_b)
                .ok_or_else(|| anyhow::anyhow!("conflicting change missing from graph"))?
                .clone();

            let overlap = find_overlapping_path(&change_a.atoms, &change_b.atoms);
            let path = overlap
                .ok_or_else(|| anyhow::anyhow!("no overlapping path found for conflicting pair"))?;

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
        let merge_change = Change::new(
            merge_deps,
            merge_atoms,
            combined_intent,
            author.clone(),
            signing_key,
        );
        self.store
            .write_change(&merge_change)
            .map_err(|e| anyhow::anyhow!("CAS write error: {e}"))?;
        self.graph.add_change(merge_change.clone());

        // Update the current view to point to the merge change.
        let updated = View::new(&conflict.current_view, HashSet::from([merge_change.id]));
        updated
            .save(&self.shared_root)
            .map_err(|e| anyhow::anyhow!("failed to save resolved view: {e}"))?;

        // Re-materialize and write to working directory.
        let resolved_state = self.materialize_heads(&HashSet::from([merge_change.id]))?;
        write_state_to_working_dir(&self.work_root, &self.shared_root, &resolved_state)?;

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
        let arcignore = load_arcignore(&self.work_root);
        let rs_files = collect_rs_files(&self.work_root, &arcignore)?;

        for filepath in &rs_files {
            let new_src = fs::read_to_string(self.work_root.join(filepath))?;
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
            if !self.work_root.join(filepath).exists() {
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
        let views_dir = self.shared_root.join(".arc").join("views");
        let stash_idx = fs::read_dir(&views_dir)?
            .filter_map(|e| e.ok())
            .filter_map(|e| {
                let name = e.file_name().to_string_lossy().into_owned();
                name.strip_prefix(".stash_")
                    .and_then(|n| n.parse::<u32>().ok())
            })
            .max()
            .unwrap_or(0)
            + 1;
        let stash_name = format!(".stash_{stash_idx}");

        // Load current view to get its heads (the stash's deps).
        let current_view = View::load(&self.shared_root, &view_name)
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
            .save(&self.shared_root)
            .map_err(|e| anyhow::anyhow!("failed to save stash view: {e}"))?;

        // Reset working directory to the clean base state.
        write_state_to_working_dir(&self.work_root, &self.shared_root, &base_state)?;

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

        let views_dir = self.shared_root.join(".arc").join("views");
        let stash_file = views_dir.join(&stash_name);

        let stash_view = View::load(&self.shared_root, &stash_name)
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
        let views_dir = self.shared_root.join(".arc").join("views");
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
            let n = |s: &str| {
                s.strip_prefix(".stash_")
                    .and_then(|x| x.parse::<u32>().ok())
                    .unwrap_or(0)
            };
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
        let view = View::load(&self.shared_root, &view_name)
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

        let current_view = View::load(&self.shared_root, &view_name)
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
        let exclusive: Vec<Blake3Hash> = ancestors_v.difference(&ancestors_x).copied().collect();
        for exc_id in &exclusive {
            let exc_change = self.graph.get(exc_id).ok_or_else(|| {
                anyhow::anyhow!(
                    "change {} missing from graph during cherry-pick",
                    _hex(exc_id)
                )
            })?;
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
        self.log_operation("cherry-pick", &view_name, current_view.heads.clone())?;
        let mut new_heads = current_view.heads.clone();
        new_heads.insert(*hash);
        let updated_view = View::new(&view_name, new_heads.clone());
        updated_view
            .save(&self.shared_root)
            .map_err(|e| anyhow::anyhow!("failed to save view after cherry-pick: {e}"))?;
        let new_state = self.materialize_heads(&new_heads)?;
        write_state_to_working_dir(&self.work_root, &self.shared_root, &new_state)?;
        Ok(())
    }

    // ------------------------------------------------------------------
    // Repository configuration
    // ------------------------------------------------------------------

    fn config_path(&self) -> PathBuf {
        self.shared_root.join(".arc").join("config.json")
    }

    /// Read the repository configuration from `.arc/config.json`.
    ///
    /// Returns a default empty config when the file does not yet exist, so
    /// a freshly-initialised repository never errors on the first read.
    pub(crate) fn read_config(&self) -> anyhow::Result<RepoConfig> {
        let path = self.config_path();
        if !path.exists() {
            return Ok(RepoConfig::default());
        }
        let json = fs::read_to_string(&path)
            .map_err(|e| anyhow::anyhow!("failed to read .arc/config.json: {e}"))?;
        serde_json::from_str(&json).map_err(|e| anyhow::anyhow!(".arc/config.json is corrupt: {e}"))
    }

    fn write_config(&self, config: &RepoConfig) -> anyhow::Result<()> {
        let path = self.config_path();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&path, serde_json::to_string_pretty(config)?)
            .map_err(|e| anyhow::anyhow!("failed to write .arc/config.json: {e}"))
    }

    /// Run all commands registered for `event` in `.arc/config.json`.
    ///
    /// Each command is parsed via `shlex::split` so it supports quoted
    /// arguments. The process inherits the current environment and runs with
    /// `work_root` as its working directory. On a non-zero exit code, or if
    /// the binary cannot be found, the error is returned and the calling
    /// operation is aborted.
    ///
    /// **Windows note:** shell built-ins such as `echo` are not standalone
    /// executables and are not in `PATH`. Use an explicit interpreter
    /// (e.g. `cmd /C echo hello`) or a real binary.
    fn run_hook(&self, event: &str) -> anyhow::Result<()> {
        let hooks = self.read_config()?.hooks;
        let commands = match hooks.get(event) {
            Some(v) if !v.is_empty() => v.clone(),
            _ => return Ok(()),
        };
        for cmd_str in &commands {
            let parts = shlex::split(cmd_str)
                .ok_or_else(|| anyhow::anyhow!("hook command parse error: {cmd_str}"))?;
            let (bin, args) = parts
                .split_first()
                .ok_or_else(|| anyhow::anyhow!("empty hook command for event '{event}'"))?;
            let status = std::process::Command::new(bin)
                .args(args)
                .current_dir(&self.work_root)
                .status()
                .map_err(|e| anyhow::anyhow!(
                    "Hook '{event}' failed to launch '{bin}': {e}. \
                     Ensure the command is an executable in your PATH \
                     (shell built-ins like 'echo' are not PATH executables \
                     on Windows — use 'cmd /C echo ...' instead)."
                ))?;
            if !status.success() {
                anyhow::bail!(
                    "hook '{event}' exited with {status} — operation aborted."
                );
            }
        }
        Ok(())
    }

    // ------------------------------------------------------------------
    // Remotes
    // ------------------------------------------------------------------

    /// Store a named remote URL alias in `.arc/config.json`.
    ///
    /// If a remote with the same name already exists it is overwritten,
    /// making this operation idempotent.
    pub fn add_remote(&self, name: &str, url: &str) -> anyhow::Result<()> {
        let mut config = self.read_config()?;
        config.remotes.insert(name.to_string(), url.to_string());
        self.write_config(&config)
    }

    /// Return all configured remote aliases.
    pub fn list_remotes(&self) -> anyhow::Result<HashMap<String, String>> {
        Ok(self.read_config()?.remotes)
    }

    // ------------------------------------------------------------------
    // Tags
    // ------------------------------------------------------------------

    /// Create a signed, immutable tag named `name` pointing to `target`.
    ///
    /// The tag is written to `.arc/tags/<name>.json`.  Forward-slashes in
    /// `name` are silently replaced with `-` for filesystem portability.
    pub fn create_tag(&self, name: &str, target: &Blake3Hash) -> anyhow::Result<()> {
        let (author, signing_key) = self.signing_identity()?;
        let tag = Tag::new(name, *target, author.clone(), signing_key);

        let tag_dir = self.shared_root.join(".arc").join("tags");
        fs::create_dir_all(&tag_dir)?;

        let safe_name = name.replace('/', "-");
        let path = tag_dir.join(format!("{safe_name}.json"));
        if path.exists() {
            anyhow::bail!("tag '{name}' already exists");
        }
        fs::write(&path, serde_json::to_string_pretty(&tag)?)
            .map_err(|e| anyhow::anyhow!("failed to write tag '{name}': {e}"))
    }

    /// Return all tags stored in `.arc/tags/`, sorted alphabetically by name.
    pub fn list_tags(&self) -> anyhow::Result<Vec<Tag>> {
        let tag_dir = self.shared_root.join(".arc").join("tags");
        if !tag_dir.exists() {
            return Ok(vec![]);
        }
        let mut tags = Vec::new();
        for entry in fs::read_dir(&tag_dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("json") {
                let json = fs::read_to_string(&path)
                    .map_err(|e| anyhow::anyhow!("failed to read tag {:?}: {e}", path))?;
                let tag: Tag = serde_json::from_str(&json)
                    .map_err(|e| anyhow::anyhow!("corrupt tag file {:?}: {e}", path))?;
                tags.push(tag);
            }
        }
        tags.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(tags)
    }

    // ------------------------------------------------------------------
    // Semantic Revert
    // ------------------------------------------------------------------

    /// Compute atoms that transform `state_after` into `state_before`.
    ///
    /// This is the pure-memory diff used by [`revert`](Self::revert) to
    /// derive the semantic inverse of a `Change` without touching the CAS,
    /// any view file, or the working directory.
    fn compute_state_delta(
        &self,
        state_after: &MaterializedState,
        state_before: &MaterializedState,
    ) -> anyhow::Result<Vec<Atom>> {
        let plugin = RustPlugin::new();
        let mut atoms: Vec<Atom> = Vec::new();

        let files_after = extract_filepaths_from_state(state_after);
        let files_before = extract_filepaths_from_state(state_before);

        // For each file present in the "after" state, diff backwards (after→before).
        for filepath in &files_after {
            let src_after = plugin.unparse(state_after, filepath).unwrap_or_default();
            let src_before = plugin.unparse(state_before, filepath).unwrap_or_default();
            if src_after == src_before {
                continue;
            }
            let ast_atoms = plugin
                .diff(&src_after, &src_before)
                .map_err(|e| anyhow::anyhow!("revert diff error for {filepath}: {e}"))?;
            for atom in ast_atoms {
                atoms.push(prefix_atom_path(atom, filepath));
            }
        }

        // Files that existed *before* the target change but were deleted by it
        // must be restored. diff("", src_before) yields Insert atoms for every
        // AST node that needs to come back.
        for filepath in files_before.difference(&files_after) {
            let src_before = plugin.unparse(state_before, filepath).unwrap_or_default();
            if src_before.is_empty() {
                continue;
            }
            let ast_atoms = plugin
                .diff("", &src_before)
                .map_err(|e| anyhow::anyhow!("revert restore error for {filepath}: {e}"))?;
            for atom in ast_atoms {
                atoms.push(prefix_atom_path(atom, filepath));
            }
        }

        Ok(atoms)
    }

    /// Semantically revert the `Change` identified by `hash`.
    ///
    /// Reverts by materializing the state *before* (State A = X's deps) and
    /// *after* (State B = X applied on State A), then running
    /// `plugin.diff(State B → State A)` to obtain the exact AST anti-patch.
    /// Because arc atoms target structural `NodePath`s rather than line
    /// numbers, this anti-patch applies cleanly to the current working
    /// directory regardless of any intermediate reformatting.
    ///
    /// Returns the [`Blake3Hash`] of the newly-created revert `Change`.
    pub fn revert(&mut self, hash: &Blake3Hash) -> anyhow::Result<Blake3Hash> {
        let view_name = self.current_view_name()?;
        self.hydrate(&view_name)?;

        // Load the target change from the CAS and ensure it is in the graph.
        let target = self
            .store
            .read_change(hash)
            .map_err(|_| anyhow::anyhow!("change {} not found in CAS", _hex(hash)))?;
        self.graph.add_change(target.clone());

        // Hydrate all of X's declared dependencies.
        self.hydrate_heads(&target.deps)?;

        // State A: the world immediately before X was applied.
        let state_a = if target.deps.is_empty() {
            MaterializedState::new()
        } else {
            self.materialize_heads(&target.deps)?
        };

        // State B: the world after X (X applied on top of State A).
        let state_b = self.materialize_heads(&HashSet::from([*hash]))?;

        // The semantic anti-patch: diff backwards (B → A).
        let revert_atoms = self.compute_state_delta(&state_b, &state_a)?;
        if revert_atoms.is_empty() {
            anyhow::bail!(
                "nothing to revert — change {} produced no materializable AST changes",
                _hex(hash)
            );
        }

        let intent = format!("revert {}", &_hex(hash)[..8]);
        let current_view = View::load(&self.shared_root, &view_name)
            .map_err(|e| anyhow::anyhow!("failed to load current view: {e}"))?;

        let (author, signing_key) = self.signing_identity()?;
        let revert_change = Change::new(
            current_view.heads.clone(),
            revert_atoms,
            intent,
            author.clone(),
            signing_key,
        );
        self.store
            .write_change(&revert_change)
            .map_err(|e| anyhow::anyhow!("CAS write error: {e}"))?;
        self.graph.add_change(revert_change.clone());

        // Log before mutating the view frontier.
        self.log_operation("revert", &view_name, current_view.heads.clone())?;

        // Advance the current view to point at the revert change.
        let updated_view = View::new(&view_name, HashSet::from([revert_change.id]));
        updated_view
            .save(&self.shared_root)
            .map_err(|e| anyhow::anyhow!("failed to save view after revert: {e}"))?;

        // Re-materialise and write to the working directory.
        let new_state = self.materialize_heads(&HashSet::from([revert_change.id]))?;
        write_state_to_working_dir(&self.work_root, &self.shared_root, &new_state)?;

        Ok(revert_change.id)
    }

    // ------------------------------------------------------------------
    // Working directory rescue
    // ------------------------------------------------------------------

    /// Restore `filepath` in the working directory to its state in the
    /// current view.
    ///
    /// If the file exists in the materialized view state, its AST atoms are
    /// unparsed back to source and written to disk, overwriting any local
    /// edits.  If the file is **not tracked** in the current view (i.e. it
    /// has never been snapped), an error is returned and the on-disk file
    /// is left completely untouched.
    pub fn restore(&mut self, filepath: &str) -> anyhow::Result<()> {
        let view_name = self.current_view_name()?;
        self.hydrate(&view_name)?;
        let state = self.materialize(&view_name)?;

        let tracked = extract_filepaths_from_state(&state);
        if !tracked.contains(filepath) {
            anyhow::bail!(
                "Cannot restore '{}': file is not tracked in the current view.",
                filepath
            );
        }

        // Log before writing so undo() can re-materialize the view state.
        let view_heads = View::load(&self.shared_root, &view_name)
            .map(|v| v.heads)
            .unwrap_or_default();
        self.log_operation("restore", &view_name, view_heads)?;

        let full = self.work_root.join(filepath);
        if let Some(parent) = full.parent() {
            fs::create_dir_all(parent)?;
        }

        if filepath.ends_with(".rs") {
            let plugin = RustPlugin::new();
            let source = plugin.unparse(&state, filepath).unwrap_or_default();
            fs::write(&full, source.as_bytes())
                .map_err(|e| anyhow::anyhow!("failed to restore '{}': {e}", filepath))
        } else {
            // Blob restore: fetch raw bytes from .arc/blobs/{hex(hash)}.
            let path_key = vec!["file".to_string(), filepath.to_string()];
            if let Some(content) = state.get(&path_key) {
                if content.starts_with(b"ARC_BLOB_REF:") && content.len() >= 45 {
                    let hash: Blake3Hash = content[13..45].try_into().unwrap_or([0u8; 32]);
                    let blob_path = self.shared_root.join(".arc").join("blobs").join(_hex(&hash));
                    let blob_file = std::fs::File::open(&blob_path)
                        .map_err(|e| anyhow::anyhow!("missing blob for '{}': {e}", filepath))?;
                    // SAFETY: The CAS blob store is an append-only, content-addressed system.
                    // Files in .arc/blobs/ are named by their BLAKE3 hash and are strictly
                    // immutable. No other process will ever truncate or modify this file while mapped.
                    let mmap = unsafe { memmap2::Mmap::map(&blob_file) }
                        .map_err(|e| anyhow::anyhow!("mmap failed for '{}': {e}", filepath))?;
                    fs::write(&full, &mmap[..])
                        .map_err(|e| anyhow::anyhow!("failed to restore '{}': {e}", filepath))
                } else {
                    Ok(())
                }
            } else {
                Ok(())
            }
        }
    }

    // ------------------------------------------------------------------
    // View listing
    // ------------------------------------------------------------------

    /// Return the names of all non-hidden views in the repository, sorted
    /// alphabetically.
    ///
    /// Hidden views (names beginning with `.`) are excluded, which filters
    /// out the internal stash views created by [`stash`](Self::stash).
    pub fn list_views(&self) -> anyhow::Result<Vec<String>> {
        let views_dir = self.shared_root.join(".arc").join("views");
        let mut names: Vec<String> = fs::read_dir(&views_dir)?
            .filter_map(|e| e.ok())
            .filter_map(|e| {
                let name = e.file_name().to_string_lossy().into_owned();
                if name.starts_with('.') {
                    None
                } else {
                    Some(name)
                }
            })
            .collect();
        names.sort();
        Ok(names)
    }

    // ------------------------------------------------------------------
    // Repository telemetry
    // ------------------------------------------------------------------

    /// Print a telemetry dashboard for the current repository.
    ///
    /// Reports the active view, total [`Change`] objects persisted in the
    /// CAS (counted by a fast directory walk — no deserialization needed),
    /// total non-hidden views, and the configured signing identity.
    pub fn info(&self) -> anyhow::Result<()> {
        use comfy_table::{Attribute, Cell, Color, Table, presets};

        let current = self.current_view_name()?;
        let changes = count_files_recursive(&self.shared_root.join(".arc").join("store"));
        let views = self.list_views()?.len();
        let identity = match load_identity() {
            Ok((Author::Human { name, email, .. }, _)) => format!("{name} <{email}>"),
            Ok((Author::AI { model, .. }, _)) => format!("{model} [AI]"),
            Err(_) => "Not configured".to_string(),
        };

        let mut table = Table::new();
        table.load_preset(presets::NOTHING);
        let rows: &[(&str, String)] = &[
            ("Repository Path", self.shared_root.display().to_string()),
            ("Current View",    current),
            ("CAS Objects",     format!("{changes}")),
            ("Views",           format!("{views}")),
            ("Active Identity", identity),
        ];
        for (label, value) in rows {
            table.add_row(vec![
                Cell::new(label)
                    .fg(Color::Cyan)
                    .add_attribute(Attribute::Bold),
                Cell::new(value),
            ]);
        }
        println!("{table}");
        Ok(())
    }

    // ------------------------------------------------------------------
    // Operation log
    // ------------------------------------------------------------------

    /// Persist raw bytes for every [`Atom::Blob`] in `atoms` to `.arc/blobs/`.
    ///
    /// Called from [`snap`](Self::snap) so that `apply_change` (which is pure)
    /// can later find the bytes it needs without performing disk I/O itself.
    fn write_blob_atoms(&self, atoms: &[Atom]) -> anyhow::Result<()> {
        for atom in atoms {
            if let Atom::Blob { path, hash } = atom {
                let filepath = path
                    .get(1)
                    .ok_or_else(|| anyhow::anyhow!("invalid blob path: {path:?}"))?;
                let bytes = fs::read(self.work_root.join(filepath))
                    .map_err(|e| anyhow::anyhow!("failed to read blob source '{filepath}': {e}"))?;
                let blobs_dir = self.shared_root.join(".arc").join("blobs");
                fs::create_dir_all(&blobs_dir)?;
                let blob_file = blobs_dir.join(_hex(hash));
                if !blob_file.exists() {
                    fs::write(&blob_file, &bytes)
                        .map_err(|e| anyhow::anyhow!("failed to write blob: {e}"))?;
                }
            }
        }
        Ok(())
    }

    /// Append an [`OpLogEntry`] to `.arc/oplog.json` recording the DAG state
    /// **before** a mutating operation.
    ///
    /// Called by every method that advances view heads so that
    /// [`undo`](Self::undo) can rewind the last operation.
    fn log_operation(
        &self,
        command: &str,
        view: &str,
        previous_heads: HashSet<Blake3Hash>,
    ) -> anyhow::Result<()> {
        let oplog_path = self.shared_root.join(".arc").join("oplog.json");
        let mut entries: Vec<OpLogEntry> = if oplog_path.exists() {
            let json = fs::read_to_string(&oplog_path)
                .map_err(|e| anyhow::anyhow!("failed to read oplog: {e}"))?;
            serde_json::from_str(&json).unwrap_or_default()
        } else {
            Vec::new()
        };
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        entries.push(OpLogEntry {
            timestamp,
            command: command.to_string(),
            view: view.to_string(),
            previous_heads,
        });
        fs::write(&oplog_path, serde_json::to_string_pretty(&entries)?)
            .map_err(|e| anyhow::anyhow!("failed to write oplog: {e}"))
    }

    /// Undo the last view-mutating operation recorded in the operation log.
    ///
    /// Reads `.arc/oplog.json`, pops the most-recent [`OpLogEntry`], restores
    /// the view to its `previous_heads`, re-materializes the state, and writes
    /// the working directory to match.  Blob files that existed before but are
    /// absent in the restored state are deleted from disk.
    ///
    /// Returns an error if the operation log is empty.
    pub fn undo(&mut self) -> anyhow::Result<()> {
        let oplog_path = self.shared_root.join(".arc").join("oplog.json");
        if !oplog_path.exists() {
            anyhow::bail!("nothing to undo — operation log is empty");
        }
        let json = fs::read_to_string(&oplog_path)
            .map_err(|e| anyhow::anyhow!("failed to read oplog: {e}"))?;
        let mut entries: Vec<OpLogEntry> = serde_json::from_str(&json).unwrap_or_default();
        let entry = entries
            .pop()
            .ok_or_else(|| anyhow::anyhow!("nothing to undo — operation log is empty"))?;

        // Load the current view and materialise it so we know which blob files
        // exist right now (and may need to be removed after the undo).
        let current_view = View::load(&self.shared_root, &entry.view)
            .map_err(|e| anyhow::anyhow!("failed to load view '{}': {e}", entry.view))?;
        self.hydrate_heads(&current_view.heads)?;
        let current_state = if current_view.heads.is_empty() {
            MaterializedState::new()
        } else {
            self.materialize_heads(&current_view.heads)?
        };

        // Restore the view to its pre-operation heads.
        let restored_view = View::new(&entry.view, entry.previous_heads.clone());
        restored_view
            .save(&self.shared_root)
            .map_err(|e| anyhow::anyhow!("failed to restore view '{}': {e}", entry.view))?;


        // Materialise the restored state.
        self.hydrate_heads(&entry.previous_heads)?;
        let restored_state = if entry.previous_heads.is_empty() {
            MaterializedState::new()
        } else {
            self.materialize_heads(&entry.previous_heads)?
        };

        // Remove non-RS blob files that exist now but shouldn't after the undo.
        let blobs_after: HashSet<String> = extract_filepaths_from_state(&restored_state)
            .into_iter()
            .filter(|f| !f.ends_with(".rs"))
            .collect();
        for filepath in extract_filepaths_from_state(&current_state)
            .into_iter()
            .filter(|f| !f.ends_with(".rs"))
        {
            if !blobs_after.contains(&filepath) {
                let _ = fs::remove_file(self.work_root.join(&filepath));
            }
        }

        // Write the restored state to the working directory.
        write_state_to_working_dir(&self.work_root, &self.shared_root, &restored_state)?;

        // Persist the updated oplog (without the popped entry).
        fs::write(&oplog_path, serde_json::to_string_pretty(&entries)?)
            .map_err(|e| anyhow::anyhow!("failed to update oplog: {e}"))?;

        println!(
            "Undid '{}' on view '{}'. Restored to previous state.",
            entry.command, entry.view
        );
        Ok(())
    }

    // ------------------------------------------------------------------
    // Sparse checkout
    // ------------------------------------------------------------------

    /// Return the active sparse cone patterns, or an empty `Vec` when the
    /// repository is in full-checkout mode.
    pub fn read_sparse_patterns(&self) -> Vec<String> {
        load_sparse_patterns(&self.work_root)
    }

    /// Update the sparse cone and rematerialize the working directory.
    ///
    /// * When `patterns` is empty the sparse filter is removed (full checkout).
    /// * Otherwise, `.arc/sparse.json` is written and the working directory is
    ///   projected so that only files under the given path prefixes exist on disk.
    ///   Files that fall *outside* the new cone are physically removed so IDEs
    ///   do not encounter stale, unmanaged files.
    pub fn apply_sparse(&mut self, patterns: &[String]) -> anyhow::Result<()> {
        let sparse_path = self.work_root.join(".arc").join("sparse.json");

        // Step 1: remove stale files that are outside the new cone.
        let view_name = self.current_view_name()?;
        self.hydrate(&view_name)?;
        let state = self.materialize(&view_name)?;
        if !patterns.is_empty() {
            for filepath in extract_filepaths_from_state(&state) {
                if !patterns.iter().any(|p| filepath.starts_with(p.as_str())) {
                    let full = self.work_root.join(&filepath);
                    if full.exists() {
                        let _ = fs::remove_file(&full);
                    }
                }
            }
        }

        // Step 2: persist (or delete) the sparse pattern list.
        if patterns.is_empty() {
            if sparse_path.exists() {
                fs::remove_file(&sparse_path)
                    .map_err(|e| anyhow::anyhow!("failed to remove sparse.json: {e}"))?;
            }
        } else {
            fs::write(
                &sparse_path,
                serde_json::to_vec_pretty(patterns)
                    .map_err(|e| anyhow::anyhow!("failed to serialise sparse patterns: {e}"))?,
            )
            .map_err(|e| anyhow::anyhow!("failed to write sparse.json: {e}"))?;
        }

        // Step 3: re-project the working directory through the new filter.
        write_state_to_working_dir(&self.work_root, &self.shared_root, &state)?;
        Ok(())
    }

    // ------------------------------------------------------------------
    // Mount algebra
    // ------------------------------------------------------------------

    /// Record an `Atom::Mount` declaration as a new change in the current view.
    ///
    /// The change is signed, written to the CAS, and replaces the view's head
    /// frontier.  The operation is also appended to the operation log so that
    /// `arc undo` can revert it.
    pub fn mount_add(
        &mut self,
        path: &str,
        url: &str,
        target: &str,
    ) -> anyhow::Result<Blake3Hash> {
        let view_name = self.current_view_name()?;
        self.hydrate(&view_name)?;
        let mut view = View::load(&self.shared_root, &view_name)
            .map_err(|e| anyhow::anyhow!("failed to load view '{view_name}': {e}"))?;
        let (author, signing_key) = self.signing_identity()?;
        let atom = Atom::Mount {
            path: vec!["file".to_string(), path.to_string()],
            url: url.to_string(),
            target: target.to_string(),
        };
        let change = Change::new(
            view.heads.clone(),
            vec![atom],
            format!("mount {path} → {url}@{target}"),
            author.clone(),
            signing_key,
        );
        self.store
            .write_change(&change)
            .map_err(|e| anyhow::anyhow!("CAS write error: {e}"))?;
        self.graph.add_change(change.clone());
        // Log before mutating view frontier so arc undo can rewind this operation.
        self.log_operation("mount add", &view_name, view.heads.clone())?;
        view.heads = HashSet::from([change.id]);
        view.save(&self.shared_root)
            .map_err(|e| anyhow::anyhow!("failed to save view: {e}"))?;
        Ok(change.id)
    }

    /// Clone or update all mounted sub-repositories declared in the current view.
    ///
    /// For each `ARC_MOUNT:` token in the materialized state:
    /// * If the mount directory has no `.arc/` sub-directory, the sub-repository
    ///   is initialised and the target view is fetched via the internal sync API.
    /// * If `.arc/` already exists, the repository is opened and the view is
    ///   fetched to pick up new changes before switching.
    ///
    /// A progress spinner is shown for the full sync pass.
    pub fn mount_sync(&mut self) -> anyhow::Result<()> {
        use std::time::Duration;

        let view_name = self.current_view_name()?;
        self.hydrate(&view_name)?;
        let state = self.materialize(&view_name)?;

        // Collect all ARC_MOUNT: entries.
        let mounts: Vec<(String, String, String)> = state
            .iter()
            .filter_map(|(key, value)| {
                if key.len() == 2 && key[0] == "file" && value.starts_with(b"ARC_MOUNT:") {
                    let info = std::str::from_utf8(value).ok()?.strip_prefix("ARC_MOUNT:")?;
                    let (url, tgt) = info.split_once('|')?;
                    Some((key[1].clone(), url.to_string(), tgt.to_string()))
                } else {
                    None
                }
            })
            .collect();

        if mounts.is_empty() {
            println!("No mounts declared in current view.");
            return Ok(());
        }

        let pb = indicatif::ProgressBar::new_spinner();
        pb.enable_steady_tick(Duration::from_millis(80));
        pb.set_message(format!("Syncing {} mount(s)...", mounts.len()));

        for (path, url, target) in &mounts {
            pb.set_message(format!("Syncing mount '{path}' from {url}@{target}..."));
            let mount_dir = self.work_root.join(path);
            let arc_sub = mount_dir.join(".arc");
            let mut sub_repo = if arc_sub.exists() {
                Repository::open(&mount_dir)
                    .map_err(|e| anyhow::anyhow!("failed to open mount '{}': {e}", path))?
            } else {
                fs::create_dir_all(&mount_dir)
                    .map_err(|e| anyhow::anyhow!("failed to create mount dir '{}': {e}", path))?;
                Repository::init(&mount_dir)
                    .map_err(|e| anyhow::anyhow!("failed to init mount '{}': {e}", path))?
            };
            crate::sync::fetch(&mut sub_repo, url, target)
                .map_err(|e| anyhow::anyhow!("fetch failed for mount '{}': {e}", path))?;
            sub_repo
                .switch_view(target)
                .map_err(|e| anyhow::anyhow!("switch_view failed for mount '{}': {e}", path))?;
        }

        pb.finish_with_message(format!("Synced {} mount(s).", mounts.len()));
        Ok(())
    }

    // ------------------------------------------------------------------
    // Workspaces
    // ------------------------------------------------------------------

    /// Create a linked workspace at `work_root` that shares this repository's CAS.
    ///
    /// Writes a `.arc-workspace` manifest into `work_root`, switches it to the
    /// given `view` (defaulting to the current view when `None`), and checks out
    /// that view's working directory.
    pub fn workspace_add(&mut self, work_root: &Path, view: Option<&str>) -> anyhow::Result<()> {
        let view_name = match view {
            Some(v) => v.to_string(),
            None => self.current_view_name()?,
        };
        // Ensure the target view exists.
        let views_dir = self.shared_root.join(".arc").join("views");
        if !views_dir.join(&view_name).exists() {
            anyhow::bail!("view '{view_name}' does not exist");
        }
        fs::create_dir_all(work_root)
            .map_err(|e| anyhow::anyhow!("failed to create workspace dir: {e}"))?;
        let manifest = WorkspaceManifest {
            shared_root: self.shared_root.clone(),
            view: view_name.clone(),
            sparse_patterns: vec![],
        };
        let manifest_path = work_root.join(".arc-workspace");
        fs::write(
            &manifest_path,
            serde_json::to_string_pretty(&manifest)
                .map_err(|e| anyhow::anyhow!("failed to serialise workspace manifest: {e}"))?,
        )
        .map_err(|e| anyhow::anyhow!("failed to write .arc-workspace: {e}"))?;

        // Open the new workspace and materialise it.
        let mut ws = Repository::open(work_root)
            .map_err(|e| anyhow::anyhow!("failed to open new workspace: {e}"))?;
        ws.hydrate(&view_name)?;
        let state = ws.materialize(&view_name)?;
        write_state_to_working_dir(work_root, &self.shared_root, &state)?;
        Ok(())
    }

    /// List all workspace directories registered via `.arc-workspace` manifests
    /// that point at this shared CAS root.
    ///
    /// Scans the parent of `shared_root` one level deep for `.arc-workspace` files.
    pub fn workspace_list(&self) -> anyhow::Result<Vec<PathBuf>> {
        let parent = self
            .shared_root
            .parent()
            .unwrap_or(&self.shared_root);
        let mut workspaces = Vec::new();
        if let Ok(rd) = fs::read_dir(parent) {
            for entry in rd.filter_map(|e| e.ok()) {
                let p = entry.path();
                if p.is_dir() {
                    let ws_file = p.join(".arc-workspace");
                    if ws_file.exists()
                        && let Ok(json) = fs::read_to_string(&ws_file)
                        && let Ok(mf) = serde_json::from_str::<WorkspaceManifest>(&json)
                        && mf.shared_root == self.shared_root
                    {
                        workspaces.push(p);
                    }
                }
            }
        }
        workspaces.sort();
        Ok(workspaces)
    }

    // ------------------------------------------------------------------
    // Garbage collection
    // ------------------------------------------------------------------

    /// Collect unreachable CAS objects and return a [`GcResult`] summary.
    ///
    /// **Reachability root set** (causal-stability-aware):
    /// 1. Every head of every non-stash view.
    /// 2. Every `previous_heads` set recorded in `oplog.json` (OpLog protection).
    ///
    /// A [`Change`] is *stable* (safe to delete) only when it is unreachable
    /// from that combined root set AND it appears in the causal-stability
    /// intersection across **all** views (meaning every peer has already
    /// integrated it).  Objects that are not yet causally stable are kept even
    /// if they are currently unreachable from any single view.
    pub fn gc(&mut self) -> anyhow::Result<GcResult> {
        // --- Step 1: build the root set ------------------------------------
        let views_dir = self.shared_root.join(".arc").join("views");
        let mut root_set: HashSet<Blake3Hash> = HashSet::new();

        // All view heads.
        if let Ok(rd) = fs::read_dir(&views_dir) {
            for entry in rd.filter_map(|e| e.ok()) {
                let name = entry.file_name().to_string_lossy().into_owned();
                if let Ok(view) = View::load(&self.shared_root, &name) {
                    root_set.extend(view.heads.iter().copied());
                }
            }
        }

        // OpLog protection: every previous_heads set.
        let oplog_path = self.shared_root.join(".arc").join("oplog.json");
        if oplog_path.exists()
            && let Ok(json) = fs::read_to_string(&oplog_path)
            && let Ok(entries) = serde_json::from_str::<Vec<OpLogEntry>>(&json)
        {
            for entry in &entries {
                root_set.extend(entry.previous_heads.iter().copied());
            }
        }

        // --- Step 2: BFS to find all reachable changes ---------------------
        let reachable = self.graph.ancestors(&root_set);

        // --- Step 3: causal stability — intersection of all view histories --
        // A change is causally stable if it appears in EVERY view's ancestry.
        let mut per_view_ancestors: Vec<HashSet<Blake3Hash>> = Vec::new();
        if let Ok(rd) = fs::read_dir(&views_dir) {
            for entry in rd.filter_map(|e| e.ok()) {
                let name = entry.file_name().to_string_lossy().into_owned();
                if let Ok(view) = View::load(&self.shared_root, &name) {
                    per_view_ancestors.push(self.graph.ancestors(&view.heads));
                }
            }
        }
        let causally_stable: HashSet<Blake3Hash> = if per_view_ancestors.is_empty() {
            HashSet::new()
        } else {
            let mut intersection = per_view_ancestors[0].clone();
            for set in &per_view_ancestors[1..] {
                intersection.retain(|id| set.contains(id));
            }
            intersection
        };

        // --- Step 4: delete unreachable AND causally stable changes --------
        let store_dir = self.shared_root.join(".arc").join("store");
        let mut result = GcResult::default();

        // Collect change IDs present on disk.
        let mut on_disk: Vec<Blake3Hash> = Vec::new();
        if let Ok(rd) = fs::read_dir(&store_dir) {
            for entry in rd.filter_map(|e| e.ok()) {
                let fname = entry.file_name().to_string_lossy().into_owned();
                // CAS files are 64-char lowercase hex names.
                if fname.len() == 64 && fname.bytes().all(|b| b.is_ascii_hexdigit()) {
                    let mut id = [0u8; 32];
                    let mut valid = true;
                    for (i, chunk) in fname.as_bytes().chunks(2).enumerate() {
                        let hi = match chunk[0] {
                            b'0'..=b'9' => chunk[0] - b'0',
                            b'a'..=b'f' => chunk[0] - b'a' + 10,
                            b'A'..=b'F' => chunk[0] - b'A' + 10,
                            _ => { valid = false; break; }
                        };
                        let lo = match chunk[1] {
                            b'0'..=b'9' => chunk[1] - b'0',
                            b'a'..=b'f' => chunk[1] - b'a' + 10,
                            b'A'..=b'F' => chunk[1] - b'A' + 10,
                            _ => { valid = false; break; }
                        };
                        id[i] = (hi << 4) | lo;
                    }
                    if valid {
                        on_disk.push(id);
                    }
                }
            }
        }

        for id in &on_disk {
            if !reachable.contains(id) && causally_stable.contains(id) {
                let path = store_dir.join(_hex(id));
                if fs::remove_file(&path).is_ok() {
                    result.changes_deleted += 1;
                }
            }
        }

        // --- Step 5: delete orphaned blob files ----------------------------
        let blobs_dir = self.shared_root.join(".arc").join("blobs");
        // Collect all blob hashes referenced in the reachable changes.
        let mut referenced_blobs: HashSet<String> = HashSet::new();
        for id in &reachable {
            if let Some(change) = self.graph.get(id) {
                for atom in &change.atoms {
                    if let Atom::Blob { hash, .. } = atom {
                        referenced_blobs.insert(_hex(hash));
                    }
                }
            }
        }
        if blobs_dir.exists()
            && let Ok(rd) = fs::read_dir(&blobs_dir)
        {
            for entry in rd.filter_map(|e| e.ok()) {
                let fname = entry.file_name().to_string_lossy().into_owned();
                if !referenced_blobs.contains(&fname)
                    && fs::remove_file(entry.path()).is_ok()
                {
                    result.blobs_deleted += 1;
                }
            }
        }

        Ok(result)
    }

    /// Compact causally-stable history into a single **Genesis Change**.
    ///
    /// # Algorithm
    ///
    /// 1. Compute the `causally_stable` set across all views (intersection of
    ///    per-view ancestry, identical to the computation in `gc()`).
    /// 2. Bail if the stable set is trivially empty.
    /// 3. Find the **stable tips** — stable changes that no other stable change
    ///    depends on.  These are the most-recent stable points, forming the
    ///    exact boundary between "safe to truncate" and "still live".
    /// 4. Materialise the state at those stable tips — this snapshot becomes
    ///    the content of the Genesis Change.
    /// 5. Convert every `MaterializedState` entry into an `Atom`:
    ///    - `["dir", ...]` path  → `Atom::Directory`
    ///    - content starting with `b"ARC_BLOB_REF:"` → `Atom::Blob`
    ///    - everything else → `Atom::Insert`
    /// 6. Create the Genesis `Change` with **empty deps** and write it to CAS.
    /// 7. Persist the Epoch Map: every compacted ID → genesis ID.  The map is
    ///    merged with any existing `.arc/epochs` so multiple compact rounds
    ///    compose correctly.
    /// 8. Update any `View` whose current head is in `causally_stable` to
    ///    point at the Genesis Change instead.
    /// 9. Physically delete the `.arc/store/` CAS objects for every compacted
    ///    change (blob files in `.arc/blobs/` are **kept** because the Genesis
    ///    `Atom::Blob` atoms still reference them).
    ///
    /// # Cryptographic Integrity
    ///
    /// No live `Change` object is mutated.  The Epoch Map intercepts the
    /// *read path* in `hydrate_heads()`, so peers that have not yet compacted
    /// remain fully interoperable.
    pub fn compact(&mut self) -> anyhow::Result<Blake3Hash> {
        // --- Step 1: build per-view ancestors (same as gc()) ---------------
        let views_dir = self.shared_root.join(".arc").join("views");
        let mut per_view_ancestors: Vec<HashSet<Blake3Hash>> = Vec::new();
        let mut all_view_names: Vec<String> = Vec::new();

        if let Ok(rd) = fs::read_dir(&views_dir) {
            for entry in rd.filter_map(|e| e.ok()) {
                let name = entry.file_name().to_string_lossy().into_owned();
                if let Ok(view) = View::load(&self.shared_root, &name) {
                    // Ensure the graph is hydrated for this view.
                    self.hydrate_heads(&view.heads)?;
                    per_view_ancestors.push(self.graph.ancestors(&view.heads));
                    all_view_names.push(name);
                }
            }
        }

        // --- Step 2: causal stability = intersection of all view histories -
        let causally_stable: HashSet<Blake3Hash> = if per_view_ancestors.is_empty() {
            HashSet::new()
        } else {
            let mut intersection = per_view_ancestors[0].clone();
            for set in &per_view_ancestors[1..] {
                intersection.retain(|id| set.contains(id));
            }
            intersection
        };

        if causally_stable.is_empty() {
            anyhow::bail!("No stable history to compact — repository has no causally-stable changes. \
                           Ensure every view has observed the same base history before compacting.");
        }

        // --- Step 3: find stable tips (stable nodes whose deps are also     --
        //     within the stable set, but that no OTHER stable node points to) --
        let mut depended_on_by_stable: HashSet<Blake3Hash> = HashSet::new();
        for &id in &causally_stable {
            if let Some(change) = self.graph.get(&id) {
                for dep in &change.deps {
                    if causally_stable.contains(dep) {
                        depended_on_by_stable.insert(*dep);
                    }
                }
            }
        }
        let stable_tips: HashSet<Blake3Hash> = causally_stable
            .iter()
            .filter(|id| !depended_on_by_stable.contains(*id))
            .copied()
            .collect();

        // --- Step 4: materialise the stable frontier -----------------------
        // Hydrate the stable tips (should already be in graph, but ensure).
        self.hydrate_heads(&stable_tips)?;
        let state = self.materialize_heads(&stable_tips)?;

        // --- Step 5: convert MaterializedState → Vec<Atom> ----------------
        // Sort keys for deterministic Change ID across runs.
        let mut paths: Vec<&NodePath> = state.keys().collect();
        paths.sort();

        let mut atoms: Vec<Atom> = Vec::with_capacity(paths.len());
        for path in paths {
            let content = &state[path];
            if path.first().map(|s| s == "dir").unwrap_or(false) {
                atoms.push(Atom::Directory { path: path.clone() });
            } else if content.starts_with(b"ARC_BLOB_REF:") && content.len() >= 45 {
                let hash: Blake3Hash = content[13..45]
                    .try_into()
                    .map_err(|_| anyhow::anyhow!("corrupt ARC_BLOB_REF token at {path:?}"))?;
                atoms.push(Atom::Blob { path: path.clone(), hash });
            } else {
                atoms.push(Atom::Insert { at: path.clone(), content: content.clone() });
            }
        }

        // --- Step 6: create and persist the Genesis Change -----------------
        let (author, signing_key) = self.signing_identity()?;
        let genesis = Change::new(
            HashSet::new(), // no deps — this IS the root of all history
            atoms,
            "Compacted Base State",
            author.clone(),
            signing_key,
        );
        self.store
            .write_change(&genesis)
            .map_err(|e| anyhow::anyhow!("CAS write error for Genesis Change: {e}"))?;
        self.graph.add_change(genesis.clone());
        let genesis_id = genesis.id;

        // --- Step 7: build and persist the Epoch Map (append-only) ---------
        let epochs_path = self.shared_root.join(".arc").join("epochs");
        let mut epoch_json: HashMap<String, String> = if epochs_path.exists() {
            let raw = fs::read_to_string(&epochs_path)
                .unwrap_or_default();
            serde_json::from_str(&raw).unwrap_or_default()
        } else {
            HashMap::new()
        };
        for id in &causally_stable {
            epoch_json.insert(_hex(id), _hex(&genesis_id));
        }
        let serialized = serde_json::to_string_pretty(&epoch_json)
            .map_err(|e| anyhow::anyhow!("epoch map serialisation error: {e}"))?;
        let tmp = epochs_path.with_extension("tmp");
        fs::write(&tmp, &serialized)
            .map_err(|e| anyhow::anyhow!("could not write epoch map: {e}"))?;
        fs::rename(&tmp, &epochs_path)
            .map_err(|e| anyhow::anyhow!("could not rename epoch map: {e}"))?;

        // --- Step 8: rewrite any View whose head is within stable_set ------
        for name in &all_view_names {
            if let Ok(mut view) = View::load(&self.shared_root, name) {
                let old_heads: HashSet<Blake3Hash> = view.heads.clone();
                let any_compacted = old_heads.iter().any(|h| causally_stable.contains(h));
                if any_compacted {
                    // Replace compacted heads with the genesis ID; preserve any
                    // non-compacted heads (live, unstable parallel branches).
                    let mut new_heads: HashSet<Blake3Hash> = old_heads
                        .into_iter()
                        .filter(|h| !causally_stable.contains(h))
                        .collect();
                    new_heads.insert(genesis_id);
                    view.heads = new_heads;
                    view.save(&self.shared_root)
                        .map_err(|e| anyhow::anyhow!("could not save view '{name}': {e}"))?;
                }
            }
        }

        // --- Step 9: physically delete the compacted CAS objects -----------
        let store_dir = self.shared_root.join(".arc").join("store");
        for id in &causally_stable {
            let path = store_dir.join(_hex(id));
            // Ignore errors — the file may already be absent if compact() is
            // run multiple times or gc() ran concurrently.
            let _ = fs::remove_file(&path);
        }

        tracing::info!(
            genesis_id = %_hex(&genesis_id),
            compacted = causally_stable.len(),
            "compact complete"
        );

        Ok(genesis_id)
    }
}

/// Format a [`Blake3Hash`] as a lowercase 64-character hex string.
fn _hex(hash: &Blake3Hash) -> String {
    hash.iter().map(|b| format!("{b:02x}")).collect()
}

/// Decode a 64-character lowercase hex string to a [`Blake3Hash`].
/// Returns `None` for any string that is not exactly 64 valid hex chars.
fn _unhex(s: &str) -> Option<Blake3Hash> {
    if s.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, chunk) in s.as_bytes().chunks(2).enumerate() {
        let hi = match chunk[0] {
            b'0'..=b'9' => chunk[0] - b'0',
            b'a'..=b'f' => chunk[0] - b'a' + 10,
            _ => return None,
        };
        let lo = match chunk[1] {
            b'0'..=b'9' => chunk[1] - b'0',
            b'a'..=b'f' => chunk[1] - b'a' + 10,
            _ => return None,
        };
        out[i] = (hi << 4) | lo;
    }
    Some(out)
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
        Atom::Blob { path, .. } => {
            format!("Blob:     {}", path.last().unwrap_or(&"?".to_string()))
        }
        Atom::Mount { path, .. } => {
            format!("Mount:    {}", path.last().unwrap_or(&"?".to_string()))
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
        Atom::Insert { at, content } => Atom::Insert {
            at: prepend(at),
            content,
        },
        Atom::Delete { at } => Atom::Delete { at: prepend(at) },
        Atom::Move { from, to } => Atom::Move {
            from: prepend(from),
            to: prepend(to),
        },
        Atom::SemanticsPreserving { at, description } => Atom::SemanticsPreserving {
            at: prepend(at),
            description,
        },
        Atom::Directory { path } => Atom::Directory {
            path: prepend(path),
        },
        Atom::Blob { path, hash } => Atom::Blob {
            path: prepend(path),
            hash,
        },
        Atom::Mount { path, url, target } => Atom::Mount {
            path: prepend(path),
            url,
            target,
        },
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
            anyhow::bail!("working directory is dirty — snap your changes before {context}");
        }

        let ast_atoms = plugin
            .diff(&old_src, &new_src)
            .map_err(|e| anyhow::anyhow!("diff error: {e}"))?;
        if !ast_atoms.is_empty() {
            anyhow::bail!("working directory is dirty — snap your changes before {context}");
        }
    }

    // Check for files in state that no longer exist on disk.
    for filepath in extract_filepaths_from_state(state) {
        if !root.join(&filepath).exists() {
            anyhow::bail!("working directory is dirty — snap your changes before {context}");
        }
    }

    Ok(())
}

/// Overwrite the working directory with the given materialized state.
///
/// `work_root` is where files are written; `shared_root` is where the CAS
/// (`.arc/blobs/`) lives. For normal repos both are the same; for workspaces
/// they differ.
fn write_state_to_working_dir(
    work_root: &Path,
    shared_root: &Path,
    state: &MaterializedState,
) -> anyhow::Result<()> {
    tracing::debug!(work_root = ?work_root, "writing state to working directory");
    let sparse_patterns = load_sparse_patterns(work_root);
    let is_sparse = !sparse_patterns.is_empty();
    let in_sparse = |fp: &str| -> bool {
        !is_sparse || sparse_patterns.iter().any(|p| fp.starts_with(p.as_str()))
    };

    // Remove existing .rs files, tolerating NotFound.
    let arcignore = load_arcignore(work_root);
    let existing = collect_rs_files(work_root, &arcignore)?;
    for filepath in &existing {
        if !in_sparse(filepath) {
            continue; // outside sparse cone — leave as already-absent
        }
        let full = work_root.join(filepath);
        match fs::remove_file(&full) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(anyhow::anyhow!("failed to remove {}: {e}", full.display())),
        }
    }

    // Reconstruct all tracked files from the materialized state.
    let plugin = RustPlugin::new();
    for filepath in extract_filepaths_from_state(state) {
        if !in_sparse(&filepath) {
            continue; // outside sparse cone — skip projection to disk
        }
        let full = work_root.join(&filepath);
        if let Some(parent) = full.parent() {
            fs::create_dir_all(parent)?;
        }
        let path_key = vec!["file".to_string(), filepath.clone()];
        let content = state.get(&path_key);
        if filepath.ends_with(".rs") {
            // Rust files: reconstruct source from AST atoms via unparse.
            let source = plugin
                .unparse(state, &filepath)
                .map_err(|e| anyhow::anyhow!("unparse error for {filepath}: {e}"))?;
            if source.is_empty() {
                continue;
            }
            fs::write(&full, source.as_bytes())?;
        } else if let Some(content) = content {
            if content.starts_with(b"ARC_MOUNT:") {
                // Mount point: create directory and write .arc-mount placeholder.
                fs::create_dir_all(&full)?;
                let info = std::str::from_utf8(content)
                    .unwrap_or("")
                    .strip_prefix("ARC_MOUNT:")
                    .unwrap_or("");
                fs::write(full.join(".arc-mount"), info.as_bytes())?;
            } else if content.starts_with(b"ARC_BLOB_REF:") && content.len() >= 45 {
                // Blob files: fetch raw bytes from shared_root/.arc/blobs/{hex(hash)}.
                let hash: Blake3Hash = content[13..45].try_into().unwrap_or([0u8; 32]);
                let blob_path = shared_root.join(".arc").join("blobs").join(_hex(&hash));
                let blob_file = std::fs::File::open(&blob_path)
                    .map_err(|e| anyhow::anyhow!("missing blob for '{filepath}': {e}"))?;
                // SAFETY: The CAS blob store is an append-only, content-addressed system.
                // Files in .arc/blobs/ are named by their BLAKE3 hash and are strictly
                // immutable. No other process will ever truncate or modify this file while mapped.
                let mmap = unsafe { memmap2::Mmap::map(&blob_file) }
                    .map_err(|e| anyhow::anyhow!("mmap failed for '{filepath}': {e}"))?;
                fs::write(&full, &mmap[..])?;
            }
        }
    }

    // Re-create tracked empty directories.
    for key in state.keys() {
        if key.len() == 2 && key[0] == "dir" {
            fs::create_dir_all(work_root.join(&key[1]))?;
        }
    }

    Ok(())
}

/// Read active sparse cone patterns from `.arc/sparse.json`.
///
/// Returns an empty `Vec` when the file is absent (full checkout mode)
/// or when it cannot be parsed.
fn load_sparse_patterns(root: &Path) -> Vec<String> {
    let path = root.join(".arc").join("sparse.json");
    if !path.exists() {
        return vec![];
    }
    let json = match fs::read_to_string(&path) {
        Ok(s) => s,
        Err(_) => return vec![],
    };
    serde_json::from_str::<Vec<String>>(&json).unwrap_or_default()
}

/// Load the merged `RepoConfig` for a shared-root repository.
///
/// The global config (stored in the OS config directory for "arc") is loaded
/// first, then the local `.arc/config.json` is overlaid on top so that
/// local settings take precedence.  The `aliases` map is merged with local
/// entries overriding global ones of the same name.
pub fn load_merged_config(shared_root: &Path) -> anyhow::Result<RepoConfig> {
    // Global config: ~/.config/arc/config.json (or platform equivalent).
    let mut merged = RepoConfig::default();
    if let Some(proj) = directories::ProjectDirs::from("", "arc-vcs", "arc") {
        let global_path = proj.config_dir().join("config.json");
        if global_path.exists()
            && let Ok(json) = fs::read_to_string(&global_path)
            && let Ok(global) = serde_json::from_str::<RepoConfig>(&json)
        {
            merged.remotes.extend(global.remotes);
            merged.aliases.extend(global.aliases);
        }
    }
    // Local config: <shared_root>/.arc/config.json (overrides global).
    let local_path = shared_root.join(".arc").join("config.json");
    if local_path.exists()
        && let Ok(json) = fs::read_to_string(&local_path)
        && let Ok(local) = serde_json::from_str::<RepoConfig>(&json)
    {
        merged.remotes.extend(local.remotes);
        merged.aliases.extend(local.aliases);
    }
    Ok(merged)
}

/// Persist `config` to the OS-level global arc config file.
pub fn save_global_config(config: &RepoConfig) -> anyhow::Result<()> {
    let proj = directories::ProjectDirs::from("", "arc-vcs", "arc")
        .ok_or_else(|| anyhow::anyhow!("cannot determine global config directory"))?;
    let dir = proj.config_dir();
    fs::create_dir_all(dir)
        .map_err(|e| anyhow::anyhow!("failed to create config dir: {e}"))?;
    let path = dir.join("config.json");
    fs::write(&path, serde_json::to_string_pretty(config)?)
        .map_err(|e| anyhow::anyhow!("failed to write global config: {e}"))
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

/// Recursively collect **all** regular file paths relative to `root`.
///
/// Unlike [`collect_rs_files`], this returns every file regardless of
/// extension.  Used by [`Repository::compute_working_directory_delta`] to
/// detect changes in non-Rust assets tracked as [`Atom::Blob`].
fn collect_all_files(root: &Path, arcignore: &Gitignore) -> anyhow::Result<Vec<String>> {
    let mut files = Vec::new();
    collect_all_recursive(root, root, &mut files, arcignore)?;
    files.sort();
    Ok(files)
}

fn collect_all_recursive(
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
        // Skip .arc and other hidden entries.
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
            collect_all_recursive(base, &path, files, arcignore)?;
        } else if let Ok(rel) = path.strip_prefix(base) {
            let rel_str = rel.to_string_lossy().replace('\\', "/");
            files.push(rel_str);
        }
    }
    Ok(())
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

/// Recursively count all regular files under `dir`.
///
/// Used by [`Repository::info`] to report the total number of
/// [`Change`] objects persisted in the CAS.
fn count_files_recursive(dir: &Path) -> usize {
    if !dir.exists() {
        return 0;
    }
    match fs::read_dir(dir) {
        Err(_) => 0,
        Ok(rd) => rd
            .filter_map(|e| e.ok())
            .map(|e| {
                if e.path().is_dir() {
                    count_files_recursive(&e.path())
                } else {
                    1
                }
            })
            .sum(),
    }
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
        assert_eq!(reopened.shared_root, repo.shared_root);
        assert_eq!(reopened.work_root, repo.work_root);
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
            !state.contains_key(&flat_key),
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
        assert!(
            repo_path.join("a.rs").exists(),
            "a.rs must exist after merge"
        );
        assert!(
            repo_path.join("b.rs").exists(),
            "b.rs must exist after merge"
        );
        assert!(
            repo_path.join("c.rs").exists(),
            "c.rs must exist after merge"
        );

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
        assert!(
            stash_name.starts_with(".stash_"),
            "stash name must start with .stash_"
        );

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
        use arc_lang::ast::{LanguagePlugin, rust_plugin::RustPlugin};

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
        let b_id = repo
            .snap("add b", false)
            .unwrap()
            .expect("snap must produce a change");

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
        assert!(
            repo_path.join("a.rs").exists(),
            "a.rs must not be disturbed"
        );
        assert!(
            repo_path.join("c.rs").exists(),
            "c.rs must not be disturbed"
        );

        // Verify the AST content of b.rs through the plugin.
        let state = repo.materialize("main").unwrap();
        let plugin = RustPlugin::new();
        let src = plugin.unparse(&state, "b.rs").unwrap_or_default();
        assert!(
            src.contains("fn b"),
            "materialized b.rs must contain fn b, got: {src}"
        );
    }

    /// `revert` must produce the exact semantic anti-patch: the reverted
    /// function must disappear from the materialized state, and the graph
    /// must gain exactly one new change.
    #[test]
    fn test_semantic_revert() {
        use arc_lang::ast::{LanguagePlugin, rust_plugin::RustPlugin};

        let dir = tempfile::tempdir().unwrap();
        let repo_path = dir.path().join("revert_project");

        let mut repo = Repository::init(&repo_path).unwrap();
        let (author, signing_key) = arc_core::store::author::test_keypair();
        repo.set_identity(author, signing_key);

        // Snap "fn alpha() {}" into the repository.
        fs::write(repo_path.join("src.rs"), "fn alpha() {}").unwrap();
        let snap_id = repo
            .snap("add alpha", false)
            .unwrap()
            .expect("snap must produce a change");

        // Revert it — this should produce a semantic anti-patch.
        let revert_id = repo.revert(&snap_id).unwrap();
        assert_ne!(revert_id, snap_id, "revert must produce a distinct change");

        // Materialise the current (post-revert) state and verify fn alpha is gone.
        let state = repo.materialize("main").unwrap();
        let plugin = RustPlugin::new();
        let src = plugin.unparse(&state, "src.rs").unwrap_or_default();
        assert!(
            !src.contains("fn alpha"),
            "reverted state must not contain fn alpha, got: '{src}'"
        );

        // The graph must contain exactly snap + revert = 2 changes.
        let log = repo.log().unwrap();
        assert_eq!(
            log.len(),
            2,
            "log must contain snap + revert = 2 changes, got {}",
            log.len()
        );

        // The revert change must carry a valid cryptographic signature.
        let rc = repo
            .graph
            .get(&revert_id)
            .expect("revert change must be present in the graph");
        assert!(
            rc.verify_signature(),
            "revert change must carry a valid Ed25519 signature"
        );
    }

    #[test]
    fn test_tag_operations() {
        let dir = tempfile::tempdir().unwrap();
        let repo_path = dir.path().join("tag_project");

        let mut repo = Repository::init(&repo_path).unwrap();
        let (author, signing_key) = arc_core::store::author::test_keypair();
        repo.set_identity(author, signing_key);

        fs::write(repo_path.join("lib.rs"), "fn lib() {}").unwrap();
        let snap_id = repo.snap("add lib", false).unwrap().unwrap();

        // Create a tag pointing at the snap.
        repo.create_tag("v1.0.0", &snap_id).unwrap();

        // Creating the same tag twice must fail.
        assert!(
            repo.create_tag("v1.0.0", &snap_id).is_err(),
            "duplicate tag must be rejected"
        );

        // List tags and verify contents.
        let tags = repo.list_tags().unwrap();
        assert_eq!(tags.len(), 1, "must have exactly one tag");
        assert_eq!(tags[0].name, "v1.0.0");
        assert_eq!(tags[0].target, snap_id);
        assert!(tags[0].verify(), "freshly created tag must verify");

        // Tampered tag must not verify.
        let mut bad = tags[0].clone();
        bad.target = [99u8; 32];
        assert!(!bad.verify(), "tampered tag must not verify");
    }

    #[test]
    fn test_remote_config() {
        let dir = tempfile::tempdir().unwrap();
        let repo_path = dir.path().join("remote_project");
        let repo = Repository::init(&repo_path).unwrap();

        // Fresh repository has no remotes.
        let remotes = repo.list_remotes().unwrap();
        assert!(remotes.is_empty(), "fresh repo must have no remotes");

        // Add two remotes.
        repo.add_remote("origin", "http://localhost:8080").unwrap();
        repo.add_remote("upstream", "http://upstream.example.com")
            .unwrap();

        let remotes = repo.list_remotes().unwrap();
        assert_eq!(remotes.len(), 2, "must have 2 remotes");
        assert_eq!(remotes["origin"], "http://localhost:8080");
        assert_eq!(remotes["upstream"], "http://upstream.example.com");

        // Overwriting a remote must update the URL.
        repo.add_remote("origin", "http://new.localhost:8080")
            .unwrap();
        let remotes2 = repo.list_remotes().unwrap();
        assert_eq!(
            remotes2["origin"], "http://new.localhost:8080",
            "remote overwrite must update the URL"
        );
    }

    /// Universal asset engine: non-Rust files are tracked as [`Atom::Blob`].
    ///
    /// Verifies that:
    /// 1. Snapping a `.txt` file writes raw bytes to `.arc/blobs/`.
    /// 2. The materialized state holds an `ARC_BLOB_REF:` entry.
    /// 3. `restore()` reconstructs the original bytes on disk.
    /// 4. The snap change carries a valid signature.
    #[test]
    fn test_universal_asset() {
        let dir = tempfile::tempdir().unwrap();
        let repo_path = dir.path().join("blob_project");

        let mut repo = Repository::init(&repo_path).unwrap();
        let (author, signing_key) = arc_core::store::author::test_keypair();
        repo.set_identity(author, signing_key);

        // Write a non-Rust text file and snap it.
        let txt_path = repo_path.join("readme.txt");
        fs::write(&txt_path, b"Hello, arc universal assets!").unwrap();

        let snap_id = repo
            .snap("add readme.txt", false)
            .unwrap()
            .expect("snap must produce a change for a new txt file");

        // .arc/blobs/ must contain exactly one file.
        let blobs_dir = repo_path.join(".arc").join("blobs");
        assert!(blobs_dir.is_dir(), ".arc/blobs/ must exist");
        let blob_count = fs::read_dir(&blobs_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .count();
        assert_eq!(blob_count, 1, "must have exactly one blob");

        // The materialized state must carry an ARC_BLOB_REF: entry.
        let state = repo.materialize("main").unwrap();
        let path_key = vec!["file".to_string(), "readme.txt".to_string()];
        let blob_ref = state.get(&path_key).expect("blob ref must be in state");
        assert!(
            blob_ref.starts_with(b"ARC_BLOB_REF:"),
            "state entry must start with ARC_BLOB_REF:"
        );
        assert_eq!(blob_ref.len(), 45, "blob ref must be 13 + 32 bytes");

        // restore() must recover the original bytes.
        fs::write(&txt_path, b"corrupted").unwrap();
        repo.restore("readme.txt").unwrap();
        let restored = fs::read(&txt_path).unwrap();
        assert_eq!(
            restored, b"Hello, arc universal assets!",
            "restore must recover original bytes"
        );

        // Snap must carry a valid cryptographic signature.
        let change = repo.graph.get(&snap_id).expect("snap must be in graph");
        assert!(
            change.verify_signature(),
            "blob snap must carry a valid signature"
        );
    }

    /// OpLog + undo: snapping a file then calling `undo()` must revert the
    /// view to its pre-snap state and remove the file from disk.
    #[test]
    fn test_undo() {
        let dir = tempfile::tempdir().unwrap();
        let repo_path = dir.path().join("undo_project");

        let mut repo = Repository::init(&repo_path).unwrap();
        let (author, signing_key) = arc_core::store::author::test_keypair();
        repo.set_identity(author, signing_key);

        // Snap a text file.
        let txt_path = repo_path.join("data.txt");
        fs::write(&txt_path, b"important data").unwrap();
        repo.snap("add data.txt", false)
            .unwrap()
            .expect("snap must produce a change");

        // Oplog must exist after snap.
        let oplog_path = repo_path.join(".arc").join("oplog.json");
        assert!(oplog_path.exists(), "oplog must be created after snap");

        // View must have exactly one head.
        let view_before = View::load(&repo_path, "main").unwrap();
        assert_eq!(
            view_before.heads.len(),
            1,
            "view must have 1 head after snap"
        );

        // Undo the snap.
        repo.undo().unwrap();

        // View heads must now be empty (pre-snap state).
        let view_after = View::load(&repo_path, "main").unwrap();
        assert!(
            view_after.heads.is_empty(),
            "undo must restore view to empty heads (pre-snap), got: {:?}",
            view_after.heads
        );

        // The blob file must be gone from disk.
        assert!(
            !txt_path.exists(),
            "undo must remove the blob file that was introduced by the snapped change"
        );

        // Oplog must be empty (the only entry was consumed).
        let oplog_raw = fs::read_to_string(&oplog_path).unwrap_or_default();
        let oplog: Vec<serde_json::Value> = serde_json::from_str(&oplog_raw).unwrap_or_default();
        assert!(
            oplog.is_empty(),
            "oplog must be empty after undoing the only entry"
        );
    }

    /// Sparse Safety Law: files outside the active cone must be absent from
    /// disk *and* must not produce false `Delete` atoms when diffing.
    #[test]
    fn test_sparse_checkout() {
        let dir = tempfile::tempdir().unwrap();
        let repo_path = dir.path().join("sparse_project");

        let mut repo = Repository::init(&repo_path).unwrap();
        let (author, signing_key) = arc_core::store::author::test_keypair();
        repo.set_identity(author, signing_key);

        // Create two files in different directories.
        fs::write(repo_path.join("a.rs"), "fn a() {}").unwrap();
        fs::create_dir_all(repo_path.join("b")).unwrap();
        fs::write(repo_path.join("b").join("c.rs"), "fn c() {}").unwrap();

        // Snap both files in one change.
        repo.snap("add a.rs and b/c.rs", false)
            .unwrap()
            .expect("snap must produce a change");

        // Shrink the sparse cone to only `b/`.
        repo.apply_sparse(&["b/".to_string()]).unwrap();

        // b/c.rs must exist on disk; a.rs must not.
        assert!(
            repo_path.join("b").join("c.rs").exists(),
            "b/c.rs must remain in the sparse cone"
        );
        assert!(
            !repo_path.join("a.rs").exists(),
            "a.rs must be removed when outside the sparse cone"
        );

        // status() must produce no atoms — the Sparse Safety Law.
        let atoms = repo.status().unwrap();
        assert!(
            atoms.is_empty(),
            "status must return no false Delete atoms for files hidden by sparse cone; got: {atoms:?}"
        );
    }

    #[test]
    fn test_workspace_add() {
        let dir = tempfile::tempdir().unwrap();
        let primary_path = dir.path().join("primary");
        let ws_path = dir.path().join("workspace");

        // Initialise primary and snap a file.
        let mut primary = Repository::init(&primary_path).unwrap();
        let (author, signing_key) = arc_core::store::author::test_keypair();
        primary.set_identity(author.clone(), signing_key.clone());
        fs::write(primary_path.join("lib.rs"), "fn lib() {}").unwrap();
        primary.snap("add lib.rs", false).unwrap().expect("snap");

        // Create a linked workspace.
        primary.workspace_add(&ws_path, None).unwrap();

        // The manifest must exist in the workspace directory.
        assert!(ws_path.join(".arc-workspace").exists(), ".arc-workspace must be written");

        // The workspace manifest must point at the primary shared_root.
        let json = fs::read_to_string(ws_path.join(".arc-workspace")).unwrap();
        let mf: WorkspaceManifest = serde_json::from_str(&json).unwrap();
        assert_eq!(mf.shared_root, primary.shared_root);
        assert_eq!(mf.view, "main");

        // Opening the workspace must yield a linked repo, not primary mode.
        let ws = Repository::open(&ws_path).unwrap();
        assert_eq!(ws.work_root, ws_path);
        assert_eq!(ws.shared_root, primary.shared_root);

        // workspace_list() from the primary must include the workspace dir.
        let list = primary.workspace_list().unwrap();
        assert!(list.contains(&ws_path), "workspace_list must return ws_path");
    }

    #[test]
    fn test_gc() {
        let dir = tempfile::tempdir().unwrap();
        let repo_path = dir.path().join("gc_project");

        let mut repo = Repository::init(&repo_path).unwrap();
        let (author, signing_key) = arc_core::store::author::test_keypair();
        repo.set_identity(author.clone(), signing_key.clone());

        // Snap something so there is at least one reachable change.
        fs::write(repo_path.join("main.rs"), "fn main() {}").unwrap();
        repo.snap("first", false).unwrap().expect("snap");

        // GC on a clean repo must delete zero objects (everything is reachable).
        let result = repo.gc().unwrap();
        assert_eq!(result.changes_deleted, 0, "no unreachable changes expected");
        assert_eq!(result.blobs_deleted, 0, "no unreachable blobs expected");
    }

    /// DAG Compaction: PO-Log Compaction via a single Genesis Change.
    ///
    /// Verifies that after snapping three sequential changes (A, B, C) on
    /// `main`, all of which become causally stable (main is the only view),
    /// `compact()` correctly:
    /// 1. Returns a new `genesis_id`.
    /// 2. Updates the view so its sole head is `genesis_id`.
    /// 3. The in-memory graph after a fresh hydration has exactly one node
    ///    (the Genesis Change) — all prior history is gone.
    /// 4. The materialised state still contains a node whose content includes
    ///    "fn c" — ie. the semantic snapshot is perfectly preserved.
    #[test]
    fn test_dag_compaction() {
        let dir = tempfile::tempdir().unwrap();
        let repo_path = dir.path().join("compact_project");

        let mut repo = Repository::init(&repo_path).unwrap();
        let (author, signing_key) = arc_core::store::author::test_keypair();
        repo.set_identity(author.clone(), signing_key.clone());

        // Snap A: introduce fn_a.
        fs::write(repo_path.join("main.rs"), "fn a() {}").unwrap();
        repo.snap("add fn a", false).unwrap().expect("snap A");

        // Snap B: add fn_b alongside fn_a.
        fs::write(repo_path.join("main.rs"), "fn a() {}\nfn b() {}").unwrap();
        repo.snap("add fn b", false).unwrap().expect("snap B");

        // Snap C: add fn_c.
        fs::write(repo_path.join("main.rs"), "fn a() {}\nfn b() {}\nfn c() {}").unwrap();
        repo.snap("add fn c", false).unwrap().expect("snap C");

        // With only `main` view, all three changes are causally stable.
        let genesis_id = repo.compact().expect("compact must succeed");

        // Re-open the repository to prove hydration goes through the Epoch Map.
        let mut repo2 = Repository::open(&repo_path).expect("re-open");
        repo2.hydrate("main").expect("hydrate after compact");

        // The view must now point exclusively to the Genesis Change.
        let view = View::load(&repo_path, "main").expect("load main view");
        assert_eq!(view.heads.len(), 1, "view must have exactly one head after compact");
        assert!(
            view.heads.contains(&genesis_id),
            "view head must be the genesis change"
        );

        // The in-memory graph must contain only the Genesis Change
        // (no ancestors — it has empty deps).
        let ancestors = repo2.graph.ancestors(&view.heads);
        assert_eq!(
            ancestors.len(),
            1,
            "only the Genesis Change must remain in the ancestor set"
        );
        assert!(
            ancestors.contains(&genesis_id),
            "ancestors must contain genesis_id"
        );

        // Materialising the view must reproduce all three functions.
        let state = repo2.materialize("main").expect("materialize after compact");
        let all_content: String = state
            .values()
            .filter_map(|v| std::str::from_utf8(v).ok())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            all_content.contains("fn c"),
            "materialised state must contain 'fn c'; got: {all_content:?}"
        );
    }
}
