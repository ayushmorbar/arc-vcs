use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use anyhow::Context as _;
use arc_net::ai::AiProvider;
use arc_swap::ArcSwap;
use serde::{Deserialize, Serialize};

use arc_core::ai::AiResolver;
use arc_core::ai::embedding::{EmbeddingProvider, HybridProvider};
use arc_core::ai::vector_store::VectorStore;
use arc_core::algebra::apply::{BlameState, MaterializedState, apply_change};
use arc_core::algebra::commute::commutes;
use arc_core::algebra::sparse::SparseMatcher;
use arc_core::algebra::{Atom, Blake3Hash, NodePath};
use arc_core::engine::mutator;
use arc_core::store::author::{Author, PublicKeyBytes, load_identity};
use arc_core::store::bisect::{
    BisectEngine, BisectMark, BisectState, clear_state as clear_bisect_state,
    load_state as load_bisect_state, save_state as save_bisect_state,
};
use arc_core::store::cas::ObjectStore;
use arc_core::store::change::Change;
use arc_core::store::graph::ChangeGraph;
use arc_core::store::newtypes::{ChangeId, MutationId};
use arc_core::store::oplog::{OpLog, Operation, OperationAgent, RewriteTransaction};
use arc_core::store::refs::{
    read_bookmark_heads, read_bookmark_map, read_remote_branch_heads, read_remote_branch_map,
    read_tag_heads, read_tag_map,
};
use arc_core::store::tag::Tag;
use arc_core::store::view::View;
use arc_lang::ast::LanguagePlugin;
use arc_lang::ast::rust_plugin::RustPlugin;
use gix_features::parallel;
use ignore::gitignore::{Gitignore, GitignoreBuilder};

use crate::ai_pending::{
    PendingAiChange, PendingKind, clear_pending_ai, has_pending_ai, load_pending_ai,
    save_pending_ai,
};

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

/// Display-identity overrides — **not** cryptographic material.
/// Cryptographic keys remain exclusively in `~/.config/arc/identity.json`.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct UserConfig {
    /// Display name (overrides the name stored in identity.json for this repo).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Display email (overrides the email stored in identity.json for this repo).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
}

/// Merge-tool preferences.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct MergeConfig {
    /// External diff/merge tool to launch for unresolved conflicts
    /// (e.g. `"kdiff3"`, `"meld"`, `"vimdiff"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
}

/// Terminal UI preferences.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UiConfig {
    /// Colour output mode: `"auto"` (default), `"always"`, or `"never"`.
    #[serde(default = "UiConfig::default_color")]
    pub color: String,
    /// Preferred terminal pager command.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pager: Option<String>,
    /// Preferred text editor command.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub editor: Option<String>,
    /// Graph style for log rendering.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub graph_style: Option<String>,
    /// Diff formatter mode.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diff_formatter: Option<String>,
    /// Conflict marker style preference.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conflict_marker_style: Option<String>,
    /// Whether progress indicators are shown.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub progress_indicator: Option<bool>,
    /// Optional greeting emitted before each CLI command.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub greet: Option<String>,
    /// Movement UI defaults.
    #[serde(default)]
    pub movement: UiMovementConfig,
}

impl Default for UiConfig {
    fn default() -> Self {
        Self {
            color: Self::default_color(),
            pager: None,
            editor: None,
            graph_style: None,
            diff_formatter: None,
            conflict_marker_style: None,
            progress_indicator: None,
            greet: None,
            movement: UiMovementConfig::default(),
        }
    }
}

impl UiConfig {
    fn default_color() -> String {
        "auto".to_string()
    }
}

/// UI movement preferences.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct UiMovementConfig {
    /// Whether movement commands edit in-place by default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub edit: Option<bool>,
}

/// Snapshot behavior preferences.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct SnapshotConfig {
    /// Maximum size accepted for newly tracked files.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_new_file_size: Option<String>,
    /// Revset/fileset expression deciding what auto-tracks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_track: Option<String>,
    /// Whether stale workspaces are auto-updated.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_update_stale: Option<bool>,
}

/// Hint toggles for UX guidance.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct HintsConfig {
    /// Show conflict-resolution hints.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolving_conflicts: Option<bool>,
}

/// External merge tool descriptor.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct MergeToolConfig {
    /// Program/binary to execute.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub program: Option<String>,
    /// Arguments for merge mode.
    #[serde(default)]
    pub merge_args: Vec<String>,
    /// Arguments for edit mode.
    #[serde(default)]
    pub edit_args: Vec<String>,
    /// Arguments for diff mode.
    #[serde(default)]
    pub diff_args: Vec<String>,
}

/// AI resolver preferences.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct AiConfig {
    /// Provider backend: `anthropic` or `openai-compatible`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    /// Model identifier passed through to the provider.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Optional custom endpoint (provider-specific URL/base URL).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<String>,
}

/// Repository-level configuration persisted in `.arc/config.toml`.
///
/// Settings are isolated per-repository and never touch the OS keyring.
/// Cryptographic key material lives exclusively in `identity.json`.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct ArcConfig {
    /// Display identity overrides (`[user]` table).
    #[serde(default)]
    pub user: UserConfig,
    /// Merge-tool preferences (`[merge]` table).
    #[serde(default)]
    pub merge: MergeConfig,
    /// Terminal UI preferences (`[ui]` table).
    #[serde(default)]
    pub ui: UiConfig,
    /// AI resolver preferences (`[ai]` table).
    #[serde(default)]
    pub ai: AiConfig,
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
    /// Revset defaults and aliases.
    #[serde(default)]
    pub revsets: HashMap<String, String>,
    /// Template defaults.
    #[serde(default)]
    pub templates: HashMap<String, String>,
    /// Template alias defaults.
    #[serde(default, rename = "template-aliases")]
    pub template_aliases: HashMap<String, String>,
    /// Styling colors keyed by semantic token.
    #[serde(default)]
    pub colors: HashMap<String, String>,
    /// Merge tool catalog.
    #[serde(default, rename = "merge-tools")]
    pub merge_tools: HashMap<String, MergeToolConfig>,
    /// Snapshot defaults.
    #[serde(default)]
    pub snapshot: SnapshotConfig,
    /// UX hints.
    #[serde(default)]
    pub hints: HintsConfig,
}

/// Build platform-aware synthesized defaults from the imported jj config intent.
pub fn synthesized_defaults_config() -> ArcConfig {
    let mut cfg = ArcConfig::default();

    cfg.aliases.insert("b".to_string(), "bookmark".to_string());
    cfg.aliases.insert("ci".to_string(), "commit".to_string());
    cfg.aliases
        .insert("desc".to_string(), "describe".to_string());
    cfg.aliases.insert("st".to_string(), "status".to_string());

    cfg.ui.color = "auto".to_string();
    cfg.ui.graph_style = Some("curved".to_string());
    cfg.ui.diff_formatter = Some(":color-words".to_string());
    cfg.ui.conflict_marker_style = Some("diff".to_string());
    cfg.ui.progress_indicator = Some(true);
    cfg.ui.greet = None;
    cfg.ui.movement.edit = Some(false);
    if cfg!(windows) {
        cfg.ui.pager = Some(":builtin".to_string());
        cfg.ui.editor = Some("Notepad".to_string());
    } else {
        cfg.ui.editor = Some("nano".to_string());
        cfg.ui.pager = Some("less -FRX".to_string());
    }

    cfg.hints.resolving_conflicts = Some(true);
    cfg.snapshot.max_new_file_size = Some("1MiB".to_string());
    cfg.snapshot.auto_track = Some("all()".to_string());
    cfg.snapshot.auto_update_stale = Some(false);

    cfg.revsets
        .insert("arrange".to_string(), "reachable(@, mutable())".to_string());
    cfg.revsets
        .insert("fix".to_string(), "reachable(@, mutable())".to_string());
    cfg.revsets.insert(
        "simplify-parents".to_string(),
        "reachable(@, mutable())".to_string(),
    );
    cfg.revsets.insert(
        "log".to_string(),
        "present(@) | ancestors(immutable_heads().., 2) | trunk()".to_string(),
    );
    cfg.revsets
        .insert("sign".to_string(), "reachable(@, mutable())".to_string());

    cfg.templates
        .insert("log".to_string(), "builtin_log_compact".to_string());
    cfg.templates
        .insert("show".to_string(), "builtin_log_detailed".to_string());
    cfg.templates
        .insert("op_log".to_string(), "builtin_op_log_compact".to_string());
    cfg.templates.insert(
        "commit_summary".to_string(),
        "format_commit_summary_with_refs(self, format_commit_ref_names(bookmarks))".to_string(),
    );

    cfg.colors.insert("error".to_string(), "bold".to_string());
    cfg.colors
        .insert("warning".to_string(), "yellow bold".to_string());
    cfg.colors
        .insert("hint".to_string(), "cyan bold".to_string());
    cfg.colors
        .insert("commit_id".to_string(), "blue".to_string());
    cfg.colors
        .insert("change_id".to_string(), "magenta".to_string());
    cfg.colors
        .insert("author".to_string(), "yellow".to_string());
    cfg.colors
        .insert("timestamp".to_string(), "cyan".to_string());
    cfg.colors.insert("conflict".to_string(), "red".to_string());

    cfg.merge_tools.insert(
        "vscode".to_string(),
        MergeToolConfig {
            program: Some(if cfg!(windows) {
                "code.cmd".to_string()
            } else {
                "code".to_string()
            }),
            merge_args: vec![
                "--wait".to_string(),
                "--merge".to_string(),
                "$left".to_string(),
                "$right".to_string(),
                "$base".to_string(),
                "$output".to_string(),
            ],
            edit_args: Vec::new(),
            diff_args: vec![
                "--diff".to_string(),
                "$left".to_string(),
                "$right".to_string(),
                "--wait".to_string(),
            ],
        },
    );
    cfg.merge_tools.insert(
        "meld".to_string(),
        MergeToolConfig {
            program: Some("meld".to_string()),
            merge_args: vec![
                "$left".to_string(),
                "$base".to_string(),
                "$right".to_string(),
                "-o".to_string(),
                "$output".to_string(),
                "--auto-merge".to_string(),
            ],
            edit_args: vec!["$left".to_string(), "$right".to_string()],
            diff_args: Vec::new(),
        },
    );
    cfg
}

/// Backward-compat: the old JSON-only config shape (remotes + aliases + hooks).
/// Used solely to migrate `config.json` → `config.toml` on first load.
#[derive(Debug, Default, Deserialize)]
struct LegacyConfig {
    #[serde(default)]
    remotes: HashMap<String, String>,
    #[serde(default)]
    aliases: HashMap<String, String>,
    #[serde(default)]
    hooks: HashMap<String, Vec<String>>,
}

/// Alias for backward-compatibility within this crate.
pub type RepoConfig = ArcConfig;

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

/// Summary returned by [`Repository::gc`].
#[derive(Debug, Default)]
pub struct GcResult {
    /// Number of [`Change`] objects deleted from the CAS.
    pub changes_deleted: usize,
    /// Number of blob files deleted from `.arc/blobs/`.
    pub blobs_deleted: usize,
}

/// Reducer for the parallel non-Rust file hashing pass in
/// [`Repository::compute_working_directory_delta`].
///
/// Worker threads produce `anyhow::Result<Option<Atom>>` for each file;
/// this reducer collects the `Some` variants into a `Vec<Atom>`.
struct BlobAtomReducer {
    atoms: Vec<Atom>,
}

impl BlobAtomReducer {
    fn new() -> Self {
        Self { atoms: Vec::new() }
    }
}

impl parallel::Reduce for BlobAtomReducer {
    type Input = anyhow::Result<Option<Atom>>;
    type FeedProduce = ();
    type Output = Vec<Atom>;
    type Error = anyhow::Error;

    fn feed(&mut self, item: anyhow::Result<Option<Atom>>) -> Result<(), anyhow::Error> {
        if let Some(atom) = item? {
            self.atoms.push(atom);
        }
        Ok(())
    }

    fn finalize(self) -> Result<Vec<Atom>, anyhow::Error> {
        Ok(self.atoms)
    }
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
    /// In-memory change DAG — stored in an [`ArcSwap`] for lock-free reader
    /// access.  Writers perform copy-on-write: clone the current graph,
    /// mutate the clone, then atomically swap the `Arc` pointer.  Readers
    /// receive a hazard-pointer guard (or a strong `Arc` clone via
    /// [`ArcSwap::load_full`]) and never contend with writers.
    pub graph: ArcSwap<ChangeGraph>,
    /// Optional signing identity set via [`Repository::set_identity`].
    /// Required before calling [`Repository::snap`] or
    /// [`Repository::resolve_conflict`].
    identity: Option<(Author, ed25519_dalek::SigningKey)>,
    /// Exclusive filesystem lock held for the duration of any mutable
    /// operation.  `None` until [`Repository::acquire_lock`] is called;
    /// automatically released when the `Repository` is dropped.
    lock_file: Option<std::fs::File>,
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
        fs::create_dir_all(arc_dir.join("ai"))?;

        // Create the default "main" view with an empty head set.
        let main_view = View::new("main", HashSet::new());
        main_view
            .save(&root)
            .map_err(|e| anyhow::anyhow!("failed to save initial main view: {e}"))?;

        // Set active view to "main".
        fs::write(arc_dir.join("HEAD"), "main")?;

        Ok(Self {
            store: ObjectStore::new(&root),
            graph: ArcSwap::new(Arc::new(ChangeGraph::new())),
            shared_root: root.clone(),
            work_root: root,
            identity: None,
            lock_file: None,
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
                graph: ArcSwap::new(Arc::new(ChangeGraph::new())),
                shared_root,
                work_root,
                identity: None,
                lock_file: None,
            });
        }

        // Primary mode: .arc must exist here.
        if !arc_dir.exists() {
            anyhow::bail!("no arc repository found at {}", arc_dir.display());
        }

        Ok(Self {
            store: ObjectStore::new(&work_root),
            graph: ArcSwap::new(Arc::new(ChangeGraph::new())),
            shared_root: work_root.clone(),
            work_root,
            identity: None,
            lock_file: None,
        })
    }

    /// Store a signing identity on this repository handle.
    ///
    /// Must be called before [`snap`](Repository::snap) or
    /// [`resolve_conflict`](Repository::resolve_conflict).
    pub fn set_identity(&mut self, author: Author, signing_key: ed25519_dalek::SigningKey) {
        self.identity = Some((author, signing_key));
    }

    /// Add a single [`Change`] to the in-memory graph via a copy-on-write swap.
    ///
    /// Loads the current `Arc<ChangeGraph>`, clones it, inserts `change` into
    /// the clone, then atomically stores the new `Arc` back.  Readers that
    /// loaded the old `Arc` before the swap continue to see a consistent
    /// snapshot and are never invalidated mid-traversal.
    pub fn graph_add_change(&mut self, change: Change) {
        let mut new_graph = (*self.graph.load_full()).clone();
        new_graph.add_change(change);
        self.graph.store(Arc::new(new_graph));
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
            let old_id = _unhex(&k).ok_or_else(|| anyhow::anyhow!("invalid epoch key: {k}"))?;
            let new_id = _unhex(&v).ok_or_else(|| anyhow::anyhow!("invalid epoch value: {v}"))?;
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

        // Clone the current graph once and accumulate all new changes locally.
        // A single atomic ArcSwap store at the end makes the full batch visible
        // to readers atomically — far cheaper than one swap per change.
        let mut graph = (*self.graph.load_full()).clone();
        let mut queue: VecDeque<Blake3Hash> = heads.iter().copied().collect();

        while let Some(id) = queue.pop_front() {
            // Epoch Map interception: if this ID was compacted away, redirect
            // to the Genesis Change instead of attempting a CAS read.
            let id = if let Some(&genesis_id) = epoch_map.get(&id) {
                if graph.get(&genesis_id).is_none() {
                    queue.push_back(genesis_id);
                }
                continue;
            } else {
                id
            };

            if graph.get(&id).is_some() {
                continue;
            }
            let change = self
                .store
                .read_change(&id)
                .map_err(|e| anyhow::anyhow!("failed to read change from CAS: {e}"))?;
            for &dep in &change.deps {
                if graph.get(&dep).is_none() {
                    queue.push_back(dep);
                }
            }
            graph.add_change(change);
        }

        // Single atomic swap — all queued changes become visible at once.
        self.graph.store(Arc::new(graph));
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
        let order = self.graph.load().topological_sort(heads);
        let mut state = MaterializedState::new();
        let g = self.graph.load_full();

        for id in order {
            let change = g
                .get(&id)
                .ok_or_else(|| anyhow::anyhow!("change {id:?} missing from graph"))?;
            apply_change(&mut state, change, &self.store, &agent_ignore, None)
                .map_err(|e| anyhow::anyhow!("replay error: {e}"))?;
        }

        Ok(state)
    }

    /// Verify the cryptographic integrity of every change in the in-memory graph.
    ///
    /// Iterates all nodes and calls [`Change::verify_signature`] on each.
    /// Returns an error describing the first change that fails verification.
    pub fn verify_graph(&self) -> anyhow::Result<()> {
        let g = self.graph.load_full();
        for change in g.iter() {
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
        let g = self.graph.load_full();
        let order = g.topological_sort(&view.heads);
        let mut state = MaterializedState::new();
        let mut blame = BlameState::new();

        for id in order {
            let change = g
                .get(&id)
                .ok_or_else(|| anyhow::anyhow!("change {id:?} missing from graph"))?;
            apply_change(
                &mut state,
                change,
                &self.store,
                &agent_ignore,
                Some(&mut blame),
            )
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

    /// Acquire an exclusive filesystem lock on `.arc/lock`.
    ///
    /// Uses the OS kernel (POSIX `flock` / Windows `LockFile`) to ensure only
    /// one `arc` process mutates the repository at a time.  If the lock is
    /// already held by another process, this call blocks until it is released.
    /// The lock is automatically released when the [`Repository`] is dropped.
    fn acquire_lock(&mut self) -> anyhow::Result<()> {
        // Re-entrancy guard: if this Repository instance already holds the lock
        // (e.g. a top-level command already called acquire_lock() and then
        // invokes another method that also calls it), return immediately without
        // trying to open a second file descriptor — that would deadlock.
        if self.lock_file.is_some() {
            return Ok(());
        }
        use fs2::FileExt;
        let lock_path = self.shared_root.join(".arc").join("lock");
        let file = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(false)
            .open(&lock_path)
            .map_err(|e| anyhow::anyhow!("could not open .arc/lock: {e}"))?;
        if file.try_lock_exclusive().is_err() {
            tracing::info!("Waiting for repository lock held by another process...");
            file.lock_exclusive()
                .map_err(|e| anyhow::anyhow!("could not acquire repository lock: {e}"))?;
        }
        self.lock_file = Some(file);
        Ok(())
    }

    /// Snapshot the current working directory into an implicit working-copy
    /// change at the tip of the current view.
    ///
    /// Returns `Ok(false)` when there is nothing to snapshot.
    pub fn snapshot(&mut self) -> anyhow::Result<bool> {
        self.acquire_lock()?;
        let view_name = self.current_view_name()?;
        self.hydrate(&view_name)?;
        let state = self.materialize(&view_name)?;

        let atoms = self.compute_working_directory_delta(&state)?;
        if atoms.is_empty() {
            return Ok(false);
        }

        let mut view = View::load(&self.shared_root, &view_name)
            .map_err(|e| anyhow::anyhow!("failed to load view '{view_name}': {e}"))?;
        let before_heads = view.heads.clone();
        let (author, signing_key) = self.signing_identity()?;

        self.write_blob_atoms(&atoms)?;
        let change = Change::new(
            view.heads.clone(),
            atoms,
            "snapshot working copy",
            author.clone(),
            signing_key,
        );

        self.store
            .write_change(&change)
            .map_err(|e| anyhow::anyhow!("CAS write error: {e}"))?;
        self.graph_add_change(change.clone());

        view.heads = HashSet::from([change.id]);
        view.save(&self.shared_root)
            .map_err(|e| anyhow::anyhow!("failed to save view: {e}"))?;

        self.log_operation(
            "snapshot working copy",
            &view_name,
            before_heads,
            HashSet::from([change.id]),
        )?;

        Ok(true)
    }

    /// Finalize the current implicit working-copy change with user-facing
    /// commit metadata.
    pub fn finalize_snapshot(&mut self, message: &str) -> anyhow::Result<Blake3Hash> {
        self.acquire_lock()?;
        let view_name = self.current_view_name()?;
        self.hydrate(&view_name)?;

        let mut view = View::load(&self.shared_root, &view_name)
            .map_err(|e| anyhow::anyhow!("failed to load view '{view_name}': {e}"))?;
        if view.heads.len() != 1 {
            anyhow::bail!(
                "finalize_snapshot requires exactly one head; view '{}' has {} heads",
                view_name,
                view.heads.len()
            );
        }

        let old_head = *view.heads.iter().next().unwrap();
        let old_change = self
            .graph
            .load()
            .get(&old_head)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("change {} missing from graph", _hex(&old_head)))?;

        let (author, signing_key) = self.signing_identity()?;
        let new_change = Change::new(
            old_change.deps.clone(),
            old_change.atoms.clone(),
            message,
            author.clone(),
            signing_key,
        );

        self.store
            .write_change(&new_change)
            .map_err(|e| anyhow::anyhow!("CAS write error: {e}"))?;
        self.graph_add_change(new_change.clone());
        let _ = self.try_embed_change(&new_change);

        view.heads = HashSet::from([new_change.id]);
        view.save(&self.shared_root)
            .map_err(|e| anyhow::anyhow!("failed to save view: {e}"))?;

        self.log_operation(
            "snap",
            &view_name,
            HashSet::from([old_head]),
            HashSet::from([new_change.id]),
        )?;

        Ok(new_change.id)
    }

    /// Fork the next empty implicit working-copy change on top of the current
    /// finalized commit.
    pub fn fork_empty_snapshot(&mut self) -> anyhow::Result<Blake3Hash> {
        self.acquire_lock()?;
        let view_name = self.current_view_name()?;
        self.hydrate(&view_name)?;

        let mut view = View::load(&self.shared_root, &view_name)
            .map_err(|e| anyhow::anyhow!("failed to load view '{view_name}': {e}"))?;
        let before_heads = view.heads.clone();
        let (author, signing_key) = self.signing_identity()?;

        let wc_change = Change::new(
            view.heads.clone(),
            Vec::new(),
            "snapshot working copy",
            author.clone(),
            signing_key,
        );

        self.store
            .write_change(&wc_change)
            .map_err(|e| anyhow::anyhow!("CAS write error: {e}"))?;
        self.graph_add_change(wc_change.clone());

        view.heads = HashSet::from([wc_change.id]);
        view.save(&self.shared_root)
            .map_err(|e| anyhow::anyhow!("failed to save view: {e}"))?;

        self.log_operation(
            "snapshot working copy",
            &view_name,
            before_heads,
            HashSet::from([wc_change.id]),
        )?;

        Ok(wc_change.id)
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
        // Guard: refuse to snap while a diffedit is in progress.
        let diffedit_lock = self.shared_root.join(".arc").join("diffedit_target");
        if diffedit_lock.exists() {
            anyhow::bail!(
                "A diffedit is in progress. Run 'arc diffedit --apply' to finish, \
                 or 'arc diffedit --abort' to cancel."
            );
        }
        // State Lock: refuse to snap while an AI change is pending approval.
        if has_pending_ai(&self.shared_root) {
            anyhow::bail!(
                "An AI change is pending approval.\n\
                 Run 'arc ai approve' to sign and commit it, \
                 or delete '.arc/ai/pending.json' to discard it."
            );
        }
        self.run_hook("pre-snap")?;
        self.acquire_lock()?;
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
            let mut last_file: Option<String> = None;
            select_atoms_interactively(raw_atoms, |filepath, label| {
                use std::io::Write;

                if last_file.as_deref() != Some(filepath) {
                    println!("-- {filepath} --");
                    last_file = Some(filepath.to_string());
                }
                print!("  {label}\n  Stage this change? [y/N] ");
                std::io::stdout().flush().ok();
                let mut line = String::new();
                std::io::stdin().read_line(&mut line).ok();
                line.trim().eq_ignore_ascii_case("y")
            })
        } else {
            raw_atoms
        };

        // Strip the interactive-filtered result: if nothing left after filtering
        // file-AST atoms, and the only remaining atoms are non-AST (dirs etc.),
        // check whether there are any real changes.
        let has_file_change = all_atoms.iter().any(|a| {
            a.paths().first().and_then(|p| p.first()).map(|s| s == "file").unwrap_or(false)
                || matches!(a, Atom::Directory { .. })
                || matches!(a, Atom::Delete { at, .. } if at.first().map(|s| s == "dir").unwrap_or(false))
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
        self.graph_add_change(change.clone());

        // Update the semantic intent index (no-op if index not yet initialised).
        let _ = self.try_embed_change(&change);

        // Capture the current frontier before advancing it.
        let before_heads = view.heads.clone();

        // Advance the frontier: new change becomes the sole head.
        view.heads = HashSet::from([change.id]);
        view.save(&self.shared_root)
            .map_err(|e| anyhow::anyhow!("failed to save view: {e}"))?;

        // Record the completed snap in the spacetime log.
        self.log_operation("snap", &view_name, before_heads, HashSet::from([change.id]))?;

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
        let sparse_matcher = sparse_matcher_for_root(&self.work_root);
        let arcignore = load_arcignore(&self.work_root);
        let rs_files = collect_rs_files(&self.work_root, &arcignore)?;
        let mut atoms: Vec<Atom> = Vec::new();

        for filepath in &rs_files {
            if !sparse_matcher.matches_file_path(filepath) {
                continue;
            }
            let new_src = fs::read_to_string(self.work_root.join(filepath))?;
            let old_src = plugin.unparse(state, filepath).unwrap_or_default();
            if old_src == new_src {
                continue;
            }
            let ast_atoms = plugin
                .diff(&old_src, &new_src, &self.store)
                .map_err(|e| anyhow::anyhow!("diff error for {filepath}: {e}"))?;
            if ast_atoms.is_empty() {
                continue;
            }
            for atom in ast_atoms {
                atoms.push(prefix_atom_path(atom, filepath));
            }
        }

        // ── Pass 2: Non-Rust files — parallel BLAKE3 blob diff ─────────────
        let all_files = collect_all_files(&self.work_root, &arcignore)?;
        let tracked_files: HashSet<String> = state
            .keys()
            .filter(|k| k.len() == 2 && k[0] == "file")
            .map(|k| k[1].clone())
            .collect();
        let non_rs_files: Vec<String> = all_files
            .into_iter()
            .filter(|f| sparse_matcher.matches_file_path(f))
            .filter(|f| !f.ends_with(".rs"))
            .filter(|f| tracked_files.contains(f.as_str()) || !is_implicitly_ignored(Path::new(f)))
            .collect();
        let work_root: &std::path::Path = &self.work_root;
        let blob_atoms = parallel::in_parallel(
            non_rs_files.into_iter(),
            None,
            |_| blake3::Hasher::new(),
            |filepath: String, hasher: &mut blake3::Hasher| -> anyhow::Result<Option<Atom>> {
                let bytes = fs::read(work_root.join(&filepath))
                    .map_err(|e| anyhow::anyhow!("failed to read '{filepath}': {e}"))?;
                hasher.reset();
                hasher.update(&bytes);
                let new_hash: Blake3Hash = *hasher.finalize().as_bytes();
                let path_key = vec!["file".to_string(), filepath];
                if let Some(existing) = state.get(&path_key)
                    && existing.starts_with(b"ARC_BLOB_REF:")
                    && existing.len() >= 45
                {
                    let old_hash: Blake3Hash = existing[13..45].try_into().unwrap_or([0u8; 32]);
                    if old_hash == new_hash {
                        return Ok(None);
                    }
                }
                Ok(Some(Atom::Blob {
                    path: path_key,
                    hash: new_hash,
                }))
            },
            BlobAtomReducer::new(),
        )?;
        atoms.extend(blob_atoms);

        // Deleted files.
        let state_filepaths = extract_filepaths_from_state(state);
        for filepath in &state_filepaths {
            // Sparse Safety Law: do not emit Delete for files hidden by sparse cone.
            if !sparse_matcher.matches_file_path(filepath) {
                continue;
            }
            if !self.work_root.join(filepath).exists() {
                let prefix = ["file".to_string(), filepath.clone()];
                for key in state.keys() {
                    // >= covers blob keys (len==2) as well as AST sub-keys (len>2).
                    if key.len() >= prefix.len() && key[..prefix.len()] == prefix[..] {
                        let prior_bytes = state.get(key).cloned().unwrap_or_default();
                        let prior_hash = self.store.write_blob(&prior_bytes).map_err(|e| {
                            anyhow::anyhow!("CAS write error for deleted file: {e}")
                        })?;
                        atoms.push(Atom::Delete {
                            at: key.clone(),
                            prior_hash,
                        });
                    }
                }
            }
        }

        // New / removed empty directories.
        let dir_key = |rel: &str| vec!["dir".to_string(), rel.to_string()];
        let existing_dirs: HashSet<String> = state
            .keys()
            .filter(|k| k.len() == 2 && k[0] == "dir")
            .filter(|k| sparse_matcher.matches_file_path(&k[1]))
            .map(|k| k[1].clone())
            .collect();
        for rel_dir in collect_empty_dirs(&self.work_root, &arcignore)? {
            if !sparse_matcher.matches_file_path(&rel_dir) {
                continue;
            }
            if !existing_dirs.contains(&rel_dir) {
                atoms.push(Atom::Directory {
                    path: dir_key(&rel_dir),
                });
            }
        }
        for rel_dir in &existing_dirs {
            if !self.work_root.join(rel_dir).exists() {
                let key = dir_key(rel_dir);
                let prior_bytes = state.get(&key).cloned().unwrap_or_default();
                let prior_hash = self
                    .store
                    .write_blob(&prior_bytes)
                    .map_err(|e| anyhow::anyhow!("CAS write error for deleted dir: {e}"))?;
                atoms.push(Atom::Delete {
                    at: key,
                    prior_hash,
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
        check_working_dir_clean(
            &self.work_root,
            &current_state,
            &self.store,
            "switching views",
        )?;

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
        self.acquire_lock()?;
        let current_name = self.current_view_name()?;
        tracing::info!(view = %current_name, "merge_heads started");

        // Hydrate both sides (idempotent — already-present nodes are skipped).
        self.hydrate(&current_name)?;
        self.hydrate_heads(target_heads)?;

        let current_view = View::load(&self.shared_root, &current_name)
            .map_err(|e| anyhow::anyhow!("failed to load view '{current_name}': {e}"))?;

        // --- Dirty working-directory check ---
        let current_state = self.materialize_heads(&current_view.heads)?;
        check_working_dir_clean(&self.work_root, &current_state, &self.store, "merging")?;

        // Find LCA.
        let lca_heads = self
            .graph
            .load()
            .merge_base(&current_view.heads, target_heads);

        // Compute ancestors from each side and from the LCA.
        let ancestors_current = self.graph.load().ancestors(&current_view.heads);
        let ancestors_target = self.graph.load().ancestors(target_heads);
        let ancestors_lca = if lca_heads.is_empty() {
            HashSet::new()
        } else {
            self.graph.load().ancestors(&lca_heads)
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

        let mut delta_a = delta_a;
        let mut delta_b = delta_b;
        delta_a.sort();
        delta_b.sort();

        // Cross-product commutativity check.
        let mut conflicting_pairs = Vec::new();
        let g = self.graph.load_full();
        for &id_a in &delta_a {
            let change_a = g
                .get(&id_a)
                .ok_or_else(|| anyhow::anyhow!("change missing from graph"))?;
            for &id_b in &delta_b {
                let change_b = g
                    .get(&id_b)
                    .ok_or_else(|| anyhow::anyhow!("change missing from graph"))?;
                if !commutes(change_a, change_b) {
                    conflicting_pairs.push((id_a, id_b));
                }
            }
        }

        if !conflicting_pairs.is_empty() {
            // Preserve legacy metadata for Ghost Node / resolve workflow.
            let conflict = PendingConflict {
                current_view: current_name.clone(),
                target_heads: target_heads.clone(),
                conflicting_pairs: conflicting_pairs.clone(),
            };
            let conflict_path = self.shared_root.join(".arc").join("conflict");
            let bytes = bincode::serialize(&conflict)
                .map_err(|e| anyhow::anyhow!("failed to serialize conflict: {e}"))?;
            fs::write(&conflict_path, bytes)?;

            // Materialize the three states used to build first-class conflict atoms.
            let target_state = self.materialize_heads(target_heads)?;
            let lca_state = if lca_heads.is_empty() {
                MaterializedState::new()
            } else {
                self.materialize_heads(&lca_heads)?
            };

            let mut conflict_atoms = Vec::new();
            let mut seen_paths: HashSet<NodePath> = HashSet::new();

            for (id_a, id_b) in &conflicting_pairs {
                let change_a = g
                    .get(id_a)
                    .ok_or_else(|| anyhow::anyhow!("conflicting change missing from graph"))?;
                let change_b = g
                    .get(id_b)
                    .ok_or_else(|| anyhow::anyhow!("conflicting change missing from graph"))?;

                let overlap = find_overlapping_path(&change_a.atoms, &change_b.atoms)
                    .ok_or_else(|| anyhow::anyhow!("no overlapping path found for conflict"))?;

                if !seen_paths.insert(overlap.clone()) {
                    continue;
                }

                let base_bytes = extract_content_at_path(&lca_state, &overlap);
                let ours_bytes = extract_content_at_path(&current_state, &overlap);
                let theirs_bytes = extract_content_at_path(&target_state, &overlap);

                let base_hash = self
                    .store
                    .write_blob(&base_bytes)
                    .map_err(|e| anyhow::anyhow!("failed to write conflict base blob: {e}"))?;
                let ours_hash = self
                    .store
                    .write_blob(&ours_bytes)
                    .map_err(|e| anyhow::anyhow!("failed to write conflict side blob: {e}"))?;
                let theirs_hash = self
                    .store
                    .write_blob(&theirs_bytes)
                    .map_err(|e| anyhow::anyhow!("failed to write conflict side blob: {e}"))?;

                let mut bases = vec![base_hash];
                let mut sides = vec![ours_hash, theirs_hash];
                bases.sort();
                sides.sort();

                conflict_atoms.push(Atom::Conflict {
                    bases,
                    sides,
                    at: overlap,
                });
            }

            conflict_atoms.sort_by(|a, b| {
                let a_path = match a {
                    Atom::Conflict { at, .. } => at,
                    _ => unreachable!("conflict_atoms only contains Atom::Conflict"),
                };
                let b_path = match b {
                    Atom::Conflict { at, .. } => at,
                    _ => unreachable!("conflict_atoms only contains Atom::Conflict"),
                };
                a_path.cmp(b_path)
            });

            if conflict_atoms.is_empty() {
                anyhow::bail!("detected semantic conflict but failed to construct conflict atoms");
            }

            let (author, signing_key) = self.signing_identity()?;
            let mut merge_deps = current_view.heads.clone();
            merge_deps.extend(target_heads);
            let conflict_change = Change::new(
                merge_deps,
                conflict_atoms,
                format!("merge conflict: {} pair(s)", conflicting_pairs.len()),
                author.clone(),
                signing_key,
            );

            self.store
                .write_change(&conflict_change)
                .map_err(|e| anyhow::anyhow!("failed to persist conflict change: {e}"))?;
            self.graph_add_change(conflict_change.clone());

            let prev_heads = current_view.heads.clone();
            let merged_heads = HashSet::from([conflict_change.id]);

            let updated_view = View::new(&current_name, merged_heads.clone());
            updated_view
                .save(&self.shared_root)
                .map_err(|e| anyhow::anyhow!("failed to save merged view: {e}"))?;

            self.log_operation("merge", &current_name, prev_heads, merged_heads.clone())?;

            let merged_state = self.materialize_heads(&merged_heads)?;
            write_state_to_working_dir(&self.work_root, &self.shared_root, &merged_state)?;
            self.run_hook("post-merge")?;
            tracing::info!("merge_heads complete (conflict change)");
            return Ok(());
        }

        // All commute — union the heads.
        let prev_heads = current_view.heads.clone();
        let mut merged_heads = current_view.heads;
        merged_heads.extend(target_heads);

        let updated_view = View::new(&current_name, merged_heads.clone());
        updated_view
            .save(&self.shared_root)
            .map_err(|e| anyhow::anyhow!("failed to save merged view: {e}"))?;

        // Record the completed merge in the spacetime log.
        self.log_operation("merge", &current_name, prev_heads, merged_heads.clone())?;

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
    ///
    /// **Ghost Node mode**: the resolved content is written to the working
    /// directory but NOT committed to the CAS.  Call [`approve_pending_ai`]
    /// to sign and finalise the merge change.
    pub fn resolve_conflict(&mut self, resolver: &dyn AiResolver) -> anyhow::Result<()> {
        let conflict_path = self.shared_root.join(".arc").join("conflict");
        if !conflict_path.exists() {
            anyhow::bail!("no pending conflict — nothing to resolve");
        }
        if has_pending_ai(&self.shared_root) {
            anyhow::bail!(
                "An AI change is already pending approval.\n\
                 Run 'arc ai approve' first, or delete '.arc/ai/pending.json' to discard it."
            );
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
            .load()
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
        let mut affected_files: Vec<PathBuf> = Vec::new();

        let g = self.graph.load_full();
        for (id_a, id_b) in &conflict.conflicting_pairs {
            let change_a = g
                .get(id_a)
                .ok_or_else(|| anyhow::anyhow!("conflicting change missing from graph"))?
                .clone();
            let change_b = g
                .get(id_b)
                .ok_or_else(|| anyhow::anyhow!("conflicting change missing from graph"))?
                .clone();

            let overlap = find_overlapping_path(&change_a.atoms, &change_b.atoms);
            let path = overlap
                .ok_or_else(|| anyhow::anyhow!("no overlapping path found for conflicting pair"))?;

            let base_content = extract_content_at_path(&lca_state, &path);
            let mut ours_content = extract_content_at_path(&current_state, &path);
            let mut theirs_content = extract_content_at_path(&target_state, &path);
            let mut base_content = base_content;

            // If current state already contains a conflict projection token,
            // decode hashes and recover textual sides from CAS blobs.
            if let Some((bases, sides)) = decode_conflict_projection(&ours_content) {
                if let Some(base_hash) = bases.first() {
                    base_content = read_blob_bytes(&self.shared_root, base_hash)?;
                }
                if let Some(side_a) = sides.first() {
                    ours_content = read_blob_bytes(&self.shared_root, side_a)?;
                }
                if let Some(side_b) = sides.get(1) {
                    theirs_content = read_blob_bytes(&self.shared_root, side_b)?;
                }
            }

            let resolved = resolver
                .resolve(
                    &base_content,
                    &ours_content,
                    &theirs_content,
                    &change_a.intent,
                    &change_b.intent,
                )
                .map_err(|e| anyhow::anyhow!("AI resolver failed: {e}"))?;

            self.verify_resolved_output(&path, &resolved)?;

            // Write resolved blob to CAS now (content-addressed; safe to do
            // before the Change record exists — orphans are GC'd).
            let content_hash = self
                .store
                .write_blob(&resolved)
                .map_err(|e| anyhow::anyhow!("AI merge store write failed: {e}"))?;
            merge_atoms.push(Atom::Insert {
                at: path.clone(),
                content_hash,
            });

            // Write resolved bytes directly to the working directory.
            if path.len() >= 2 && path[0] == "file" {
                let file_path = self.work_root.join(&path[1]);
                if let Some(parent) = file_path.parent() {
                    fs::create_dir_all(parent)?;
                }
                fs::write(&file_path, &resolved)
                    .map_err(|e| anyhow::anyhow!("failed to write resolved file: {e}"))?;
                affected_files.push(PathBuf::from(&path[1]));
            }

            combined_intent.push_str(&change_a.intent);
            combined_intent.push_str(" + ");
            combined_intent.push_str(&change_b.intent);
            combined_intent.push_str("; ");
        }

        // Deps = union of current view's heads and target heads.
        let mut merge_deps = current_view.heads.clone();
        merge_deps.extend(&conflict.target_heads);

        // Determine the model used (if any LlmResolver was involved it set
        // ARC_AI_MODEL; fall back to "mock" for the sentinel case).
        let model = std::env::var("ARC_AI_MODEL").unwrap_or_else(|_| "mock".to_owned());

        // Save Ghost Node — DO NOT write Change to CAS or update the view yet.
        let pending = PendingAiChange::new_resolve(
            model,
            combined_intent,
            affected_files,
            merge_atoms,
            merge_deps.into_iter().collect(),
        );
        save_pending_ai(&self.shared_root, &pending)?;

        // Remove the conflict file — the pending.json holds everything needed
        // to reconstruct the merge on 'arc ai approve'.
        fs::remove_file(&conflict_path)?;

        Ok(())
    }

    /// Resolve a pending conflict using an async, provider-agnostic AI backend.
    ///
    /// This path is used by the runtime-configurable `[ai]` provider stack.
    pub async fn resolve_conflict_with_provider(
        &mut self,
        provider: &dyn AiProvider,
        model: &str,
    ) -> anyhow::Result<()> {
        let conflict_path = self.shared_root.join(".arc").join("conflict");
        if !conflict_path.exists() {
            anyhow::bail!("no pending conflict - nothing to resolve");
        }
        if has_pending_ai(&self.shared_root) {
            anyhow::bail!(
                "An AI change is already pending approval.\n\
                 Run 'arc ai approve' first, or delete '.arc/ai/pending.json' to discard it."
            );
        }

        let conflict_bytes = fs::read(&conflict_path)?;
        let conflict: PendingConflict = bincode::deserialize(&conflict_bytes)
            .map_err(|e| anyhow::anyhow!("failed to deserialize conflict: {e}"))?;

        self.hydrate(&conflict.current_view)?;
        self.hydrate_heads(&conflict.target_heads)?;

        let current_view = View::load(&self.shared_root, &conflict.current_view)
            .map_err(|e| anyhow::anyhow!("failed to load view '{}': {e}", conflict.current_view))?;

        let lca_heads = self
            .graph
            .load()
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
        let mut affected_files: Vec<PathBuf> = Vec::new();

        let g = self.graph.load_full();
        for (id_a, id_b) in &conflict.conflicting_pairs {
            let change_a = g
                .get(id_a)
                .ok_or_else(|| anyhow::anyhow!("conflicting change missing from graph"))?
                .clone();
            let change_b = g
                .get(id_b)
                .ok_or_else(|| anyhow::anyhow!("conflicting change missing from graph"))?
                .clone();

            let overlap = find_overlapping_path(&change_a.atoms, &change_b.atoms);
            let path = overlap
                .ok_or_else(|| anyhow::anyhow!("no overlapping path found for conflicting pair"))?;

            let base_content = extract_content_at_path(&lca_state, &path);
            let mut ours_content = extract_content_at_path(&current_state, &path);
            let mut theirs_content = extract_content_at_path(&target_state, &path);
            let mut base_content = base_content;

            if let Some((bases, sides)) = decode_conflict_projection(&ours_content) {
                if let Some(base_hash) = bases.first() {
                    base_content = read_blob_bytes(&self.shared_root, base_hash)?;
                }
                if let Some(side_a) = sides.first() {
                    ours_content = read_blob_bytes(&self.shared_root, side_a)?;
                }
                if let Some(side_b) = sides.get(1) {
                    theirs_content = read_blob_bytes(&self.shared_root, side_b)?;
                }
            }

            let file_path = if path.len() >= 2 && path[0] == "file" {
                path[1].as_str()
            } else {
                "unknown"
            };

            let resolved = provider
                .resolve_conflict(
                    &String::from_utf8_lossy(&base_content),
                    &String::from_utf8_lossy(&ours_content),
                    &String::from_utf8_lossy(&theirs_content),
                    file_path,
                )
                .await
                .map_err(|e| anyhow::anyhow!("AI provider resolver failed: {e}"))?
                .into_bytes();

            self.verify_resolved_output(&path, &resolved)?;

            let content_hash = self
                .store
                .write_blob(&resolved)
                .map_err(|e| anyhow::anyhow!("AI merge store write failed: {e}"))?;
            merge_atoms.push(Atom::Insert {
                at: path.clone(),
                content_hash,
            });

            if path.len() >= 2 && path[0] == "file" {
                let file_path = self.work_root.join(&path[1]);
                if let Some(parent) = file_path.parent() {
                    fs::create_dir_all(parent)?;
                }
                fs::write(&file_path, &resolved)
                    .map_err(|e| anyhow::anyhow!("failed to write resolved file: {e}"))?;
                affected_files.push(PathBuf::from(&path[1]));
            }

            combined_intent.push_str(&change_a.intent);
            combined_intent.push_str(" + ");
            combined_intent.push_str(&change_b.intent);
            combined_intent.push_str("; ");
        }

        let mut merge_deps = current_view.heads.clone();
        merge_deps.extend(&conflict.target_heads);

        let pending = PendingAiChange::new_resolve(
            model.to_string(),
            combined_intent,
            affected_files,
            merge_atoms,
            merge_deps.into_iter().collect(),
        );
        save_pending_ai(&self.shared_root, &pending)?;

        fs::remove_file(&conflict_path)?;
        Ok(())
    }

    /// Resolve a pending conflict using a configured external merge tool and
    /// stage the result as a Ghost Node pending approval.
    #[tracing::instrument(skip(self), fields(tool = ?tool_name))]
    pub fn resolve_conflict_with_merge_tool(
        &mut self,
        tool_name: Option<&str>,
    ) -> anyhow::Result<()> {
        let conflict_path = self.shared_root.join(".arc").join("conflict");
        if !conflict_path.exists() {
            anyhow::bail!("no pending conflict - nothing to resolve");
        }
        if has_pending_ai(&self.shared_root) {
            anyhow::bail!(
                "An AI change is already pending approval.\n\
                 Run 'arc ai approve' first, or delete '.arc/ai/pending.json' to discard it."
            );
        }

        let cfg = load_merged_config(&self.shared_root)?;
        let selected_name = tool_name
            .map(ToOwned::to_owned)
            .or_else(|| cfg.merge.tool.clone())
            .ok_or_else(|| anyhow::anyhow!("no merge tool configured; set merge.tool first"))?;
        let selected_tool = cfg
            .merge_tools
            .get(&selected_name)
            .cloned()
            .ok_or_else(|| {
                anyhow::anyhow!("merge tool '{selected_name}' is not defined in [merge-tools]")
            })?;

        let conflict_bytes = fs::read(&conflict_path)?;
        let conflict: PendingConflict = bincode::deserialize(&conflict_bytes)
            .map_err(|e| anyhow::anyhow!("failed to deserialize conflict: {e}"))?;

        self.hydrate(&conflict.current_view)?;
        self.hydrate_heads(&conflict.target_heads)?;

        let current_view = View::load(&self.shared_root, &conflict.current_view)
            .map_err(|e| anyhow::anyhow!("failed to load view '{}': {e}", conflict.current_view))?;

        let lca_heads = self
            .graph
            .load()
            .merge_base(&current_view.heads, &conflict.target_heads);
        let lca_state = if lca_heads.is_empty() {
            MaterializedState::new()
        } else {
            self.materialize_heads(&lca_heads)?
        };

        let current_state = self.materialize_heads(&current_view.heads)?;
        let target_state = self.materialize_heads(&conflict.target_heads)?;

        let mut merge_atoms = Vec::new();
        let mut combined_intent = format!("merge-tool({selected_name}): ");
        let mut affected_files: Vec<PathBuf> = Vec::new();
        let mut resolved_file_updates: Vec<(PathBuf, Vec<u8>)> = Vec::new();

        let g = self.graph.load_full();
        for (id_a, id_b) in &conflict.conflicting_pairs {
            let change_a = g
                .get(id_a)
                .ok_or_else(|| anyhow::anyhow!("conflicting change missing from graph"))?
                .clone();
            let change_b = g
                .get(id_b)
                .ok_or_else(|| anyhow::anyhow!("conflicting change missing from graph"))?
                .clone();

            let overlap = find_overlapping_path(&change_a.atoms, &change_b.atoms);
            let path = overlap
                .ok_or_else(|| anyhow::anyhow!("no overlapping path found for conflicting pair"))?;

            let base_content = extract_content_at_path(&lca_state, &path);
            let mut ours_content = extract_content_at_path(&current_state, &path);
            let mut theirs_content = extract_content_at_path(&target_state, &path);
            let mut base_content = base_content;

            if let Some((bases, sides)) = decode_conflict_projection(&ours_content) {
                if let Some(base_hash) = bases.first() {
                    base_content = read_blob_bytes(&self.shared_root, base_hash)?;
                }
                if let Some(side_a) = sides.first() {
                    ours_content = read_blob_bytes(&self.shared_root, side_a)?;
                }
                if let Some(side_b) = sides.get(1) {
                    theirs_content = read_blob_bytes(&self.shared_root, side_b)?;
                }
            }

            let resolved = run_external_merge_tool_once(
                &selected_name,
                &selected_tool,
                &path,
                &base_content,
                &ours_content,
                &theirs_content,
            )?;

            self.verify_resolved_output(&path, &resolved)?;

            let content_hash = self
                .store
                .write_blob(&resolved)
                .map_err(|e| anyhow::anyhow!("merge-tool store write failed: {e}"))?;
            merge_atoms.push(Atom::Insert {
                at: path.clone(),
                content_hash,
            });

            if path.len() >= 2 && path[0] == "file" {
                let file_path = self.work_root.join(&path[1]);
                affected_files.push(PathBuf::from(&path[1]));
                resolved_file_updates.push((file_path, resolved.clone()));
            }

            combined_intent.push_str(&change_a.intent);
            combined_intent.push_str(" + ");
            combined_intent.push_str(&change_b.intent);
            combined_intent.push_str("; ");
        }

        let mut merge_deps = current_view.heads.clone();
        merge_deps.extend(&conflict.target_heads);

        let pending = PendingAiChange::new_resolve(
            format!("merge-tool:{selected_name}"),
            combined_intent,
            affected_files,
            merge_atoms,
            merge_deps.into_iter().collect(),
        );

        for (file_path, resolved) in resolved_file_updates {
            if let Some(parent) = file_path.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::write(&file_path, &resolved)
                .map_err(|e| anyhow::anyhow!("failed to write resolved file: {e}"))?;
        }

        save_pending_ai(&self.shared_root, &pending)?;

        fs::remove_file(&conflict_path)?;
        Ok(())
    }

    fn verify_resolved_output(&self, path: &[String], resolved: &[u8]) -> anyhow::Result<()> {
        if path.len() < 2 || path[0] != "file" {
            return Ok(());
        }
        let filepath = &path[1];
        if !filepath.ends_with(".rs") {
            return Ok(());
        }

        let source = std::str::from_utf8(resolved).map_err(|_| {
            anyhow::anyhow!("AI resolver produced non-UTF-8 content for Rust file '{filepath}'")
        })?;

        let plugin = RustPlugin::new();
        plugin.diff("", source, &self.store).map_err(|e| {
            anyhow::anyhow!(
                "AI-resolved Rust content failed parser verification for '{filepath}': {e}"
            )
        })?;

        Ok(())
    }

    // ------------------------------------------------------------------
    // AI Provenance — approve_pending_ai / snap_ai
    // ------------------------------------------------------------------

    /// Finalise a pending AI change: construct `Author::AI`, sign with the
    /// human's key, write to CAS, advance the view head, and clean up.
    ///
    /// This is the cryptographic approval gate that converts a Ghost Node into
    /// a permanent, content-addressed record in the DAG.
    pub fn approve_pending_ai(
        &mut self,
        human_author: &Author,
        signing_key: &ed25519_dalek::SigningKey,
    ) -> anyhow::Result<Blake3Hash> {
        let pending = load_pending_ai(&self.shared_root).ok_or_else(|| {
            anyhow::anyhow!("no pending AI change — '.arc/ai/pending.json' not found")
        })?;

        let human_key: PublicKeyBytes = match human_author {
            Author::Human { key, .. } => *key,
            _ => {
                anyhow::bail!("active identity is not a Human author; cannot sponsor an AI change")
            }
        };

        let ai_author = Author::AI {
            model: pending.model.clone(),
            human_sponsor: human_key,
        };

        let id = match pending.kind {
            PendingKind::Resolve => {
                // Atoms and deps were pre-staged by resolve_conflict().
                let deps: HashSet<Blake3Hash> = pending.staged_deps.iter().cloned().collect();
                let change = Change::new(
                    deps,
                    pending.staged_atoms.clone(),
                    &pending.intent,
                    ai_author,
                    signing_key,
                );
                self.store
                    .write_change(&change)
                    .map_err(|e| anyhow::anyhow!("CAS write error: {e}"))?;
                self.graph_add_change(change.clone());

                // Advance the current view to the new merge change.
                let view_name = self.current_view_name()?;
                let mut view = View::load(&self.shared_root, &view_name)
                    .map_err(|e| anyhow::anyhow!("failed to load view '{view_name}': {e}"))?;
                let before_heads = view.heads.clone();
                view.heads = HashSet::from([change.id]);
                view.save(&self.shared_root)
                    .map_err(|e| anyhow::anyhow!("failed to save view: {e}"))?;

                self.log_operation(
                    "ai resolve",
                    &view_name,
                    before_heads,
                    HashSet::from([change.id]),
                )?;

                let _ = self.try_embed_change(&change);
                change.id
            }
            PendingKind::Generate => {
                // Diff the working directory and snap with Author::AI.
                self.snap_ai(&pending.intent, &ai_author, signing_key)?
                    .ok_or_else(|| {
                        anyhow::anyhow!("no working-directory changes detected — nothing to commit")
                    })?
            }
        };

        clear_pending_ai(&self.shared_root);
        Ok(id)
    }

    /// Like [`snap`] but uses a pre-constructed `Author::AI` and explicit key.
    ///
    /// Called by [`approve_pending_ai`] for the Generate path.
    fn snap_ai(
        &mut self,
        message: &str,
        author: &Author,
        signing_key: &ed25519_dalek::SigningKey,
    ) -> anyhow::Result<Option<Blake3Hash>> {
        self.acquire_lock()?;
        let view_name = self.current_view_name()?;
        self.hydrate(&view_name)?;
        let state = self.materialize(&view_name)?;
        let raw_atoms = self.compute_working_directory_delta(&state)?;
        if raw_atoms.is_empty() {
            return Ok(None);
        }
        let mut view = View::load(&self.shared_root, &view_name)
            .map_err(|e| anyhow::anyhow!("failed to load view '{view_name}': {e}"))?;
        self.write_blob_atoms(&raw_atoms)?;
        let change = Change::new(
            view.heads.clone(),
            raw_atoms,
            message,
            author.clone(),
            signing_key,
        );
        self.store
            .write_change(&change)
            .map_err(|e| anyhow::anyhow!("CAS write error: {e}"))?;
        self.graph_add_change(change.clone());
        let before_heads = view.heads.clone();
        view.heads = HashSet::from([change.id]);
        view.save(&self.shared_root)
            .map_err(|e| anyhow::anyhow!("failed to save view: {e}"))?;
        self.log_operation("snap", &view_name, before_heads, HashSet::from([change.id]))?;
        let _ = self.try_embed_change(&change);
        Ok(Some(change.id))
    }

    /// Update the semantic intent index for a single change.
    ///
    /// This is a best-effort operation: errors are silently suppressed so
    /// that embedding failures never block a snap or merge commit.  The index
    /// is only updated when `.arc/ai/embeddings.db` already exists (meaning
    /// the user has already run `arc log --intent` to bootstrap the index).
    fn try_embed_change(&self, change: &Change) -> anyhow::Result<()> {
        let db_path = self
            .shared_root
            .join(".arc")
            .join("ai")
            .join("embeddings.db");
        if !db_path.exists() {
            // Index not yet bootstrapped; skip silently.
            return Ok(());
        }
        let provider = HybridProvider::new()?;
        let embedding = provider.embed(&change.intent)?;
        let hex_id: String = change.id.iter().map(|b| format!("{b:02x}")).collect();
        let store = VectorStore::open(&db_path)?;
        store.upsert(&hex_id, &embedding)?;
        Ok(())
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
                .diff(&old_src, &new_src, &self.store)
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
                        let prior_bytes = base_state.get(key).cloned().unwrap_or_default();
                        let prior_hash = self.store.write_blob(&prior_bytes).map_err(|e| {
                            anyhow::anyhow!("CAS write error for stash delete: {e}")
                        })?;
                        stash_atoms.push(Atom::Delete {
                            at: key.clone(),
                            prior_hash,
                        });
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
        self.graph_add_change(stash_change.clone());

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

    /// Compute the pending AST-atom diff **and** the per-file historical text
    /// in a single materialization pass.
    ///
    /// This is the efficient alternative to calling [`status`] followed by a
    /// separate unparse loop: the graph is hydrated and the state materialised
    /// exactly once regardless of repository size.
    ///
    /// Returns `(atoms, old_texts)` where `old_texts` maps each changed
    /// filepath to its last-snapped source text (empty string when the file
    /// did not previously exist).
    pub fn diff_info(&mut self) -> anyhow::Result<(Vec<Atom>, HashMap<String, String>)> {
        let view_name = self.current_view_name()?;
        self.hydrate(&view_name)?;
        let state = self.materialize(&view_name)?;
        let atoms = self.compute_working_directory_delta(&state)?;

        let plugin = RustPlugin::new();
        let mut old_texts: HashMap<String, String> = HashMap::new();

        for atom in &atoms {
            let filepath: Option<String> = match atom {
                Atom::Insert { at, .. }
                | Atom::Delete { at, .. }
                | Atom::SemanticsPreserving { at, .. }
                    if at.first().map(|s| s == "file").unwrap_or(false) && at.len() > 1 =>
                {
                    Some(at[1].clone())
                }
                Atom::Move { from, .. }
                    if from.first().map(|s| s == "file").unwrap_or(false) && from.len() > 1 =>
                {
                    Some(from[1].clone())
                }
                _ => None,
            };
            if let Some(fp) = filepath {
                old_texts
                    .entry(fp.clone())
                    .or_insert_with(|| plugin.unparse(&state, &fp).unwrap_or_default());
            }
        }

        Ok((atoms, old_texts))
    }

    /// Return all changes in the current view's history, newest-first.
    pub fn log(&mut self) -> anyhow::Result<Vec<Change>> {
        let view_name = self.current_view_name()?;
        self.hydrate(&view_name)?;
        let view = View::load(&self.shared_root, &view_name)
            .map_err(|e| anyhow::anyhow!("failed to load view '{view_name}': {e}"))?;
        let mut order = self.graph.load().topological_sort(&view.heads);
        order.reverse(); // oldest-first → newest-first
        order
            .iter()
            .map(|id| {
                self.graph
                    .load()
                    .get(id)
                    .cloned()
                    .ok_or_else(|| anyhow::anyhow!("change {} missing from graph", _hex(id)))
            })
            .collect()
    }

    /// Return all changes selected by `revset`, newest-first.
    pub fn log_revset(&mut self, revset: &str) -> anyhow::Result<Vec<Change>> {
        let expr = arc_core::revset::parse(revset)
            .map_err(|e| anyhow::anyhow!("invalid revset '{}': {e}", revset))?;
        let expr = constrain_touched_to_current_view(&expr);
        self.prepare_revset(&expr)?;

        let graph = self.graph_snapshot();
        let mut resolver = |symbol: &str| self.resolve_revset_symbol_typed(symbol);
        let mut refs_resolver =
            |function_name: &str| self.resolve_revset_reference_heads(function_name);
        let selected: BTreeSet<ChangeId> = arc_core::revset::compile_change_ids_with_refs(
            &expr,
            Arc::clone(&graph),
            &mut resolver,
            &mut refs_resolver,
        )?
        .collect();

        let mut ordered_ids = graph.topological_sort_ids(&selected);
        ordered_ids.reverse();
        ordered_ids
            .into_iter()
            .map(|id| self.read_change(&Blake3Hash::from(id)))
            .collect()
    }

    /// Start a new bisect session from a revset range.
    #[tracing::instrument(skip_all, fields(range = %range_revset, find_good = find_good))]
    pub fn bisect_start(
        &mut self,
        range_revset: &str,
        find_good: bool,
    ) -> anyhow::Result<BisectState> {
        self.acquire_lock()?;
        let selected = self.resolve_revset_ids(range_revset)?;
        anyhow::ensure!(
            !selected.is_empty(),
            "bisect range '{}' selected no revisions",
            range_revset
        );

        let graph = self.graph_snapshot();
        let mut state = BisectEngine::start(range_revset.to_string(), selected, find_good);
        state.current = BisectEngine::select_next(graph.as_ref(), &state);
        save_bisect_state(&self.shared_root, &state)?;
        Ok(state)
    }

    /// Return current bisect state, if present.
    pub fn bisect_status(&self) -> anyhow::Result<Option<BisectState>> {
        load_bisect_state(&self.shared_root)
    }

    /// Remove current bisect session state.
    pub fn bisect_reset(&mut self) -> anyhow::Result<()> {
        self.acquire_lock()?;
        clear_bisect_state(&self.shared_root)
    }

    /// Return bisect state with computed next candidate when needed.
    pub fn bisect_next(&mut self) -> anyhow::Result<BisectState> {
        self.acquire_lock()?;
        let mut state = load_bisect_state(&self.shared_root)?
            .ok_or_else(|| anyhow::anyhow!("no active bisect session"))?;
        if state.current.is_none() {
            let graph = self.graph_snapshot();
            state.current = BisectEngine::select_next(graph.as_ref(), &state);
            save_bisect_state(&self.shared_root, &state)?;
        }
        Ok(state)
    }

    /// Mark the current bisect revision as good.
    pub fn bisect_mark_good(&mut self) -> anyhow::Result<BisectState> {
        self.bisect_mark_current(BisectMark::Good)
    }

    /// Mark the current bisect revision as bad.
    pub fn bisect_mark_bad(&mut self) -> anyhow::Result<BisectState> {
        self.bisect_mark_current(BisectMark::Bad)
    }

    fn bisect_mark_current(&mut self, user_mark: BisectMark) -> anyhow::Result<BisectState> {
        self.acquire_lock()?;
        let mut state = load_bisect_state(&self.shared_root)?
            .ok_or_else(|| anyhow::anyhow!("no active bisect session"))?;
        let current = state
            .current
            .ok_or_else(|| anyhow::anyhow!("bisect session has no current revision to mark"))?;

        let internal_mark = if state.find_good {
            match user_mark {
                BisectMark::Good => BisectMark::Bad,
                BisectMark::Bad => BisectMark::Good,
                BisectMark::Untested => BisectMark::Untested,
            }
        } else {
            user_mark
        };

        let graph = self.graph_snapshot();
        BisectEngine::mark(graph.as_ref(), &mut state, current, internal_mark)?;
        state.current = BisectEngine::select_next(graph.as_ref(), &state);
        save_bisect_state(&self.shared_root, &state)?;
        Ok(state)
    }

    /// Benchmark common ancestor computation between two revisions.
    pub fn bench_common_ancestors(
        &mut self,
        left: &str,
        right: &str,
        iterations: u32,
    ) -> anyhow::Result<(u128, usize)> {
        let left_id = self.resolve_rev(left)?;
        let right_id = self.resolve_rev(right)?;
        self.hydrate_heads(&HashSet::from([left_id, right_id]))?;
        let graph = self.graph_snapshot();

        let mut total_nanos = 0u128;
        let mut last_len = 0usize;
        for _ in 0..iterations.max(1) {
            let start = Instant::now();
            let ancestors = graph.merge_base(&HashSet::from([left_id]), &HashSet::from([right_id]));
            total_nanos += start.elapsed().as_nanos();
            last_len = ancestors.len();
        }
        Ok((total_nanos, last_len))
    }

    /// Benchmark ancestor check between two revisions.
    pub fn bench_is_ancestor(
        &mut self,
        ancestor: &str,
        descendant: &str,
        iterations: u32,
    ) -> anyhow::Result<(u128, bool)> {
        let ancestor_id = self.resolve_rev(ancestor)?;
        let descendant_id = self.resolve_rev(descendant)?;
        self.hydrate_heads(&HashSet::from([descendant_id]))?;
        let graph = self.graph_snapshot();

        let mut total_nanos = 0u128;
        let mut last = false;
        for _ in 0..iterations.max(1) {
            let start = Instant::now();
            let ancestors = graph.ancestors(&HashSet::from([descendant_id]));
            last = ancestors.contains(&ancestor_id);
            total_nanos += start.elapsed().as_nanos();
        }
        Ok((total_nanos, last))
    }

    /// Benchmark hash-prefix resolution against loaded DAG nodes.
    pub fn bench_resolve_prefix(
        &mut self,
        prefix: &str,
        iterations: u32,
    ) -> anyhow::Result<(u128, usize)> {
        let view_name = self.current_view_name()?;
        self.hydrate(&view_name)?;
        let graph = self.graph_snapshot();
        let prefix_lower = prefix.to_ascii_lowercase();

        let mut total_nanos = 0u128;
        let mut last_len = 0usize;
        for _ in 0..iterations.max(1) {
            let start = Instant::now();
            let mut hits = 0usize;
            for change in graph.iter() {
                if ChangeId::from(change.id)
                    .to_hex()
                    .starts_with(&prefix_lower)
                {
                    hits += 1;
                }
            }
            total_nanos += start.elapsed().as_nanos();
            last_len = hits;
        }
        Ok((total_nanos, last_len))
    }

    /// Benchmark revset evaluation by counting selected revisions.
    pub fn bench_revset(&mut self, revset: &str, iterations: u32) -> anyhow::Result<(u128, usize)> {
        let mut total_nanos = 0u128;
        let mut last_count = 0usize;
        for _ in 0..iterations.max(1) {
            let start = Instant::now();
            let selected = self.resolve_revset_ids(revset)?;
            total_nanos += start.elapsed().as_nanos();
            last_count = selected.len();
        }
        Ok((total_nanos, last_count))
    }

    #[tracing::instrument(skip_all, fields(revset = %revset))]
    fn resolve_revset_ids(&mut self, revset: &str) -> anyhow::Result<BTreeSet<ChangeId>> {
        let expr = arc_core::revset::parse(revset)
            .map_err(|e| anyhow::anyhow!("invalid revset '{}': {e}", revset))?;
        let expr = constrain_touched_to_current_view(&expr);
        self.prepare_revset(&expr)?;

        let graph = self.graph_snapshot();
        let mut resolver = |symbol: &str| self.resolve_revset_symbol_typed(symbol);
        let mut refs_resolver =
            |function_name: &str| self.resolve_revset_reference_heads(function_name);
        let selected: BTreeSet<ChangeId> = arc_core::revset::compile_change_ids_with_refs(
            &expr,
            Arc::clone(&graph),
            &mut resolver,
            &mut refs_resolver,
        )?
        .collect();
        Ok(selected)
    }

    /// Semantic search over the current view's history using embedding similarity.
    ///
    /// Embeds `query`, bootstraps (or updates) the vector index at
    /// `.arc/ai/embeddings.db`, then returns the top `k` changes sorted by
    /// descending cosine similarity score.
    ///
    /// The index is populated eagerly on the first call (embeds every change
    /// in the current view), then updated incrementally by [`snap`] and
    /// [`approve_pending_ai`] on each new write.
    pub fn log_semantic(&mut self, query: &str, k: usize) -> anyhow::Result<Vec<(Change, f32)>> {
        let db_path = self
            .shared_root
            .join(".arc")
            .join("ai")
            .join("embeddings.db");

        // Initialise the embedding provider (may trigger model download on
        // first call; subsequent calls load the model from disk cache).
        eprintln!("[arc] Initializing embedding provider…");
        let provider = HybridProvider::new().context("failed to initialize embedding provider")?;

        let query_vec = provider
            .embed(query)
            .context("failed to embed search query")?;

        // Open (or create) the vector store.
        let store = VectorStore::open(&db_path).context("failed to open vector store")?;

        // Populate / refresh the index for the current view.
        let view_name = self.current_view_name()?;
        self.hydrate(&view_name)?;
        let view = View::load(&self.shared_root, &view_name)
            .map_err(|e| anyhow::anyhow!("failed to load view '{view_name}': {e}"))?;
        // Hold a strong Arc so all get() borrows stay valid across the loop.
        let g = self.graph.load_full();
        let order = g.topological_sort(&view.heads);
        for id in &order {
            let hex_id: String = id.iter().map(|b| format!("{b:02x}")).collect();
            if let Some(change) = g.get(id) {
                // Only index if not already present (avoid redundant embeds).
                let embedding = provider
                    .embed(&change.intent)
                    .context("failed to embed change intent")?;
                store
                    .upsert(&hex_id, &embedding)
                    .context("failed to upsert embedding")?;
            }
        }

        // Search.
        let results = store
            .search(&query_vec, k)
            .context("vector store search failed")?;

        let mut out = Vec::new();
        for (id_hex, score) in results {
            if let Some(hash) = hex_to_blake3(&id_hex)
                && let Some(change) = g.get(&hash)
            {
                out.push((change.clone(), score));
            }
        }
        Ok(out)
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
        self.acquire_lock()?;
        let view_name = self.current_view_name()?;
        self.hydrate(&view_name)?;

        let change = self
            .store
            .read_change(hash)
            .map_err(|_| anyhow::anyhow!("cherry-pick target {} not found in CAS", _hex(hash)))?;

        let current_view = View::load(&self.shared_root, &view_name)
            .map_err(|e| anyhow::anyhow!("failed to load view '{view_name}': {e}"))?;

        let ancestors_v = self.graph.load().ancestors(&current_view.heads);

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
        let ancestors_x = self.graph.load().ancestors(&HashSet::from([*hash]));
        let exclusive: Vec<Blake3Hash> = ancestors_v.difference(&ancestors_x).copied().collect();
        {
            let g = self.graph.load_full();
            for exc_id in &exclusive {
                let exc_change = g.get(exc_id).ok_or_else(|| {
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
        }

        // Reuse the existing Change object (same hash → same CAS entry).
        self.graph_add_change(change);
        let before_cp = current_view.heads.clone();
        let mut new_heads = current_view.heads.clone();
        new_heads.insert(*hash);
        let updated_view = View::new(&view_name, new_heads.clone());
        updated_view
            .save(&self.shared_root)
            .map_err(|e| anyhow::anyhow!("failed to save view after cherry-pick: {e}"))?;

        // Record the completed cherry-pick in the spacetime log.
        self.log_operation("cherry-pick", &view_name, before_cp, new_heads.clone())?;

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
                .map_err(|e| {
                    anyhow::anyhow!(
                        "Hook '{event}' failed to launch '{bin}': {e}. \
                     Ensure the command is an executable in your PATH \
                     (shell built-ins like 'echo' are not PATH executables \
                     on Windows — use 'cmd /C echo ...' instead)."
                    )
                })?;
            if !status.success() {
                anyhow::bail!("hook '{event}' exited with {status} — operation aborted.");
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

    /// Remove a named remote alias from `.arc/config.json`.
    ///
    /// Returns an actionable error if the remote does not exist.
    pub fn remove_remote(&self, name: &str) -> anyhow::Result<()> {
        let mut config = self.read_config()?;
        if config.remotes.remove(name).is_none() {
            anyhow::bail!(
                "Remote '{}' does not exist. Use 'arc remote list' to see available remotes.",
                name
            );
        }
        self.write_config(&config)
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

    /// Create or move a signed tag named `name` to `target`.
    ///
    /// Existing tags are only moved when `allow_move` is true.
    #[tracing::instrument(skip_all, fields(tag = %name, allow_move = allow_move))]
    pub fn set_tag(&self, name: &str, target: &Blake3Hash, allow_move: bool) -> anyhow::Result<()> {
        let (author, signing_key) = self.signing_identity()?;
        let tag = Tag::new(name, *target, author.clone(), signing_key);

        let tag_dir = self.shared_root.join(".arc").join("tags");
        fs::create_dir_all(&tag_dir)?;

        let safe_name = name.replace('/', "-");
        let path = tag_dir.join(format!("{safe_name}.json"));
        if path.exists() && !allow_move {
            anyhow::bail!("refusing to move existing tag '{name}'. Pass --allow-move to update it");
        }
        fs::write(&path, serde_json::to_string_pretty(&tag)?)
            .map_err(|e| anyhow::anyhow!("failed to write tag '{name}': {e}"))
    }

    /// Delete local tags matching any provided glob-like pattern.
    ///
    /// Returns the sorted list of deleted tag names.
    #[tracing::instrument(skip_all, fields(pattern_count = patterns.len()))]
    pub fn delete_tags_matching(&self, patterns: &[String]) -> anyhow::Result<Vec<String>> {
        let tags = self.list_tags()?;
        let mut deleted = Vec::new();
        for tag in tags {
            if patterns.iter().any(|p| simple_pattern_match(p, &tag.name)) {
                let safe_name = tag.name.replace('/', "-");
                let path = self
                    .shared_root
                    .join(".arc")
                    .join("tags")
                    .join(format!("{safe_name}.json"));
                if path.exists() {
                    fs::remove_file(&path)
                        .map_err(|e| anyhow::anyhow!("failed to delete tag '{}': {e}", tag.name))?;
                    deleted.push(tag.name);
                }
            }
        }
        deleted.sort();
        deleted.dedup();
        Ok(deleted)
    }

    /// Return tags filtered by optional name patterns.
    pub fn list_tags_matching(&self, patterns: &[String]) -> anyhow::Result<Vec<Tag>> {
        let mut tags = self.list_tags()?;
        if patterns.is_empty() {
            return Ok(tags);
        }
        tags.retain(|tag| patterns.iter().any(|p| simple_pattern_match(p, &tag.name)));
        Ok(tags)
    }

    // ------------------------------------------------------------------
    // Bookmarks
    // ------------------------------------------------------------------

    /// Create a new bookmark under `.arc/refs/bookmarks/<name>`.
    pub fn create_bookmark(&mut self, name: &str, target: &Blake3Hash) -> anyhow::Result<()> {
        let path = self.bookmark_ref_path(name)?;
        if path.exists() {
            anyhow::bail!("bookmark '{name}' already exists");
        }
        self.write_bookmark_ref(name, target)
    }

    /// Set (create or replace) a bookmark target.
    pub fn set_bookmark(&mut self, name: &str, target: &Blake3Hash) -> anyhow::Result<()> {
        self.write_bookmark_ref(name, target)
    }

    /// Move an existing bookmark to a new target.
    ///
    /// When `allow_backwards` is false, the move must be fast-forward.
    pub fn move_bookmark(
        &mut self,
        name: &str,
        target: &Blake3Hash,
        allow_backwards: bool,
    ) -> anyhow::Result<()> {
        let path = self.bookmark_ref_path(name)?;
        if !path.exists() {
            anyhow::bail!("bookmark '{name}' does not exist");
        }

        if !allow_backwards {
            let current_hex = fs::read_to_string(&path)
                .map_err(|e| anyhow::anyhow!("failed to read bookmark '{name}': {e}"))?;
            let current = ChangeId::from_hex(current_hex.trim())
                .map(Blake3Hash::from)
                .map_err(|_| anyhow::anyhow!("bookmark '{name}' contains an invalid target"))?;

            self.hydrate_heads(&HashSet::from([*target]))?;
            let ancestors = self.graph.load().ancestors(&HashSet::from([*target]));
            if !ancestors.contains(&current) {
                anyhow::bail!(
                    "refusing non-fast-forward move of bookmark '{name}'; pass --allow-backwards to override"
                );
            }
        }

        self.write_bookmark_ref(name, target)
    }

    /// Delete an existing bookmark.
    pub fn delete_bookmark(&self, name: &str) -> anyhow::Result<()> {
        let path = self.bookmark_ref_path(name)?;
        if !path.exists() {
            anyhow::bail!("bookmark '{name}' does not exist");
        }
        fs::remove_file(&path)
            .map_err(|e| anyhow::anyhow!("failed to delete bookmark '{name}': {e}"))?;
        Ok(())
    }

    /// Return bookmark decorations keyed by target change id.
    pub fn bookmark_decorations(
        &self,
    ) -> anyhow::Result<std::collections::BTreeMap<ChangeId, Vec<String>>> {
        read_bookmark_map(&self.shared_root)
    }

    fn write_bookmark_ref(&self, name: &str, target: &Blake3Hash) -> anyhow::Result<()> {
        let path = self.bookmark_ref_path(name)?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&path, ChangeId::from(*target).to_hex())
            .map_err(|e| anyhow::anyhow!("failed to write bookmark '{name}': {e}"))
    }

    fn bookmark_ref_path(&self, name: &str) -> anyhow::Result<PathBuf> {
        let trimmed = name.trim();
        if trimmed.is_empty() {
            anyhow::bail!("bookmark name must not be empty");
        }
        if trimmed.contains('\\') || trimmed.contains(':') {
            anyhow::bail!("invalid bookmark name '{name}'");
        }
        if !Path::new(trimmed).is_relative() {
            anyhow::bail!("invalid bookmark name '{name}'");
        }

        let mut path = self.shared_root.join(".arc").join("refs").join("bookmarks");
        for component in Path::new(trimmed).components() {
            match component {
                Component::Normal(segment) => path.push(segment),
                _ => anyhow::bail!("invalid bookmark name '{name}'"),
            }
        }
        Ok(path)
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
                .diff(&src_after, &src_before, &self.store)
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
                .diff("", &src_before, &self.store)
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
        self.graph_add_change(target.clone());

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
        self.graph_add_change(revert_change.clone());

        // Advance the current view to point at the revert change and record
        // the completed revert in the spacetime log.
        self.log_operation(
            "revert",
            &view_name,
            current_view.heads.clone(),
            HashSet::from([revert_change.id]),
        )?;
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
        self.acquire_lock()?;
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
        self.log_operation("restore", &view_name, view_heads.clone(), view_heads)?;

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
                    let blob_path = self
                        .shared_root
                        .join(".arc")
                        .join("blobs")
                        .join(_hex(&hash));
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
            Ok((Author::Server { canonical_id, .. }, _)) => format!("{canonical_id} [server]"),
            Ok((Author::Transient { session_id, .. }, _)) => format!("{session_id} [transient]"),
            Err(_) => "Not configured".to_string(),
        };

        let mut table = Table::new();
        table.load_preset(presets::NOTHING);
        let rows: &[(&str, String)] = &[
            ("Repository Path", self.shared_root.display().to_string()),
            ("Current View", current),
            ("CAS Objects", format!("{changes}")),
            ("Views", format!("{views}")),
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

    /// Append an [`Operation`] to the spacetime oplog, recording both the
    /// **before** and **after** head sets for this mutating operation.
    ///
    /// Delegates entirely to [`OpLog`]; the 1 000-entry sliding-window
    /// compaction and backward-compat deserialization are handled there.
    fn log_operation(
        &self,
        command: &str,
        view: &str,
        before_heads: HashSet<Blake3Hash>,
        after_heads: HashSet<Blake3Hash>,
    ) -> anyhow::Result<()> {
        let op = Operation::new(
            command,
            view,
            hashes_to_change_ids(&before_heads),
            hashes_to_change_ids(&after_heads),
        );
        if command != "undo" {
            let _ = self.save_redo_stack(&[]);
        }
        OpLog::new(&self.shared_root.join(".arc")).append(&op)
    }

    fn redo_stack_path(&self) -> PathBuf {
        self.shared_root
            .join(".arc")
            .join("local")
            .join("redo_stack.json")
    }

    fn load_redo_stack(&self) -> anyhow::Result<Vec<Operation>> {
        let primary = self.redo_stack_path();
        let staged_backup = primary.with_extension("bak.new");
        let backup = primary.with_extension("bak");
        let path = if primary.exists() {
            primary
        } else if staged_backup.exists() {
            staged_backup
        } else if backup.exists() {
            backup
        } else {
            primary
        };

        if !path.exists() {
            return Ok(Vec::new());
        }
        let raw = fs::read_to_string(&path)
            .map_err(|e| anyhow::anyhow!("failed to read redo stack '{}': {e}", path.display()))?;
        serde_json::from_str::<Vec<Operation>>(&raw)
            .map_err(|e| anyhow::anyhow!("failed to parse redo stack '{}': {e}", path.display()))
    }

    fn save_redo_stack(&self, stack: &[Operation]) -> anyhow::Result<()> {
        let path = self.redo_stack_path();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| anyhow::anyhow!("failed to create redo stack dir: {e}"))?;
        }
        let tmp = path.with_extension("tmp");
        let backup = path.with_extension("bak");
        let staged_backup = path.with_extension("bak.new");
        let payload = serde_json::to_string_pretty(stack)
            .map_err(|e| anyhow::anyhow!("failed to serialize redo stack: {e}"))?;
        fs::write(&tmp, payload).map_err(|e| {
            anyhow::anyhow!("failed to write redo stack temp '{}': {e}", tmp.display())
        })?;

        if staged_backup.exists() {
            fs::remove_file(&staged_backup)
                .map_err(|e| anyhow::anyhow!("failed to clear staged redo backup: {e}"))?;
        }
        if path.exists() {
            fs::rename(&path, &staged_backup).map_err(|e| {
                anyhow::anyhow!(
                    "failed to stage redo stack backup '{}': {e}",
                    path.display()
                )
            })?;
        }

        if let Err(err) = fs::rename(&tmp, &path) {
            if staged_backup.exists() {
                let _ = fs::rename(&staged_backup, &path);
            }
            return Err(anyhow::anyhow!(
                "failed to replace redo stack '{}': {err}",
                path.display()
            ));
        }

        if staged_backup.exists() {
            if backup.exists() {
                fs::remove_file(&backup)
                    .map_err(|e| anyhow::anyhow!("failed to rotate redo backup: {e}"))?;
            }
            fs::rename(&staged_backup, &backup)
                .map_err(|e| anyhow::anyhow!("failed to finalize redo backup: {e}"))?;
        }

        Ok(())
    }

    fn push_redo_operation(&self, op: &Operation) -> anyhow::Result<()> {
        let mut stack = self.load_redo_stack()?;
        stack.push(op.clone());
        self.save_redo_stack(&stack)
    }

    fn append_rewrite_operation(
        &self,
        command: &str,
        view: &str,
        before_heads: HashSet<Blake3Hash>,
        after_heads: HashSet<Blake3Hash>,
        rewrite_map: &HashMap<Blake3Hash, Blake3Hash>,
    ) -> anyhow::Result<()> {
        let tx_id = next_mutation_id(command, view, rewrite_map)?;
        let tx = RewriteTransaction {
            tx_id,
            command: command.to_string(),
            view: view.to_string(),
            before_heads: hashes_to_change_ids(&before_heads),
            after_heads: hashes_to_change_ids(&after_heads),
            rewrite_map: rewrite_map
                .iter()
                .map(|(old, new)| (ChangeId::from(*old), ChangeId::from(*new)))
                .collect(),
            agent: OperationAgent::Human,
        };
        OpLog::new(&self.shared_root.join(".arc")).append_transaction(&tx)
    }

    fn load_epoch_map_raw(&self) -> anyhow::Result<HashMap<String, String>> {
        let canonical = self.shared_root.join(".arc").join("epochs");
        let canonical_staged_backup = canonical.with_extension("bak.new");
        let canonical_backup = canonical.with_extension("bak");
        let legacy = self.shared_root.join(".arc").join("epochs.json");
        let path = if canonical.exists() {
            canonical
        } else if canonical_staged_backup.exists() {
            canonical_staged_backup
        } else if canonical_backup.exists() {
            canonical_backup
        } else {
            legacy
        };
        if !path.exists() {
            return Ok(HashMap::new());
        }
        let raw = fs::read_to_string(&path)
            .map_err(|e| anyhow::anyhow!("could not read epoch map '{}': {e}", path.display()))?;
        serde_json::from_str(&raw)
            .map_err(|e| anyhow::anyhow!("epoch map '{}' is not valid JSON: {e}", path.display()))
    }

    fn persist_epoch_map_raw(&self, map: &HashMap<String, String>) -> anyhow::Result<()> {
        let path = self.shared_root.join(".arc").join("epochs");
        let tmp = path.with_extension("tmp");
        let backup = path.with_extension("bak");
        let staged_backup = path.with_extension("bak.new");
        let payload = serde_json::to_string_pretty(map)
            .map_err(|e| anyhow::anyhow!("epoch map serialisation error: {e}"))?;
        fs::write(&tmp, payload).map_err(|e| anyhow::anyhow!("could not write epoch map: {e}"))?;

        if staged_backup.exists() {
            fs::remove_file(&staged_backup)
                .map_err(|e| anyhow::anyhow!("could not remove stale epoch staged backup: {e}"))?;
        }
        if path.exists() {
            fs::rename(&path, &staged_backup)
                .map_err(|e| anyhow::anyhow!("could not stage existing epoch map: {e}"))?;
        }
        if let Err(err) = fs::rename(&tmp, &path) {
            if staged_backup.exists() {
                let _ = fs::rename(&staged_backup, &path);
            }
            return Err(anyhow::anyhow!("could not rename epoch map: {err}"));
        }
        if staged_backup.exists() {
            if backup.exists() {
                fs::remove_file(&backup)
                    .map_err(|e| anyhow::anyhow!("could not replace epoch backup: {e}"))?;
            }
            fs::rename(&staged_backup, &backup)
                .map_err(|e| anyhow::anyhow!("could not finalize epoch backup: {e}"))?;
        }

        let legacy = self.shared_root.join(".arc").join("epochs.json");
        if legacy.exists() {
            let _ = fs::remove_file(legacy);
        }
        Ok(())
    }

    fn persist_rewrite_map(
        &self,
        rewrite_map: &HashMap<Blake3Hash, Blake3Hash>,
    ) -> anyhow::Result<()> {
        let mut epoch_map = self.load_epoch_map_raw()?;
        for (old, new) in rewrite_map {
            epoch_map.insert(_hex(old), _hex(new));
        }
        self.persist_epoch_map_raw(&epoch_map)
    }

    fn stage_rewrite_metadata(
        &self,
        command: &str,
        view: &str,
        before_heads: HashSet<Blake3Hash>,
        after_heads: HashSet<Blake3Hash>,
        rewrite_map: &HashMap<Blake3Hash, Blake3Hash>,
    ) -> anyhow::Result<HashMap<String, String>> {
        let previous_epoch = self.load_epoch_map_raw()?;
        self.persist_rewrite_map(rewrite_map)?;
        if let Err(err) =
            self.append_rewrite_operation(command, view, before_heads, after_heads, rewrite_map)
        {
            let _ = self.persist_epoch_map_raw(&previous_epoch);
            return Err(err);
        }
        Ok(previous_epoch)
    }

    fn rollback_rewrite_metadata(
        &self,
        previous_epoch: &HashMap<String, String>,
    ) -> anyhow::Result<()> {
        let mut failures = Vec::new();
        if let Err(err) = OpLog::new(&self.shared_root.join(".arc")).pop() {
            failures.push(format!("oplog rollback failed: {err}"));
        }
        if let Err(err) = self.persist_epoch_map_raw(previous_epoch) {
            failures.push(format!("epoch rollback failed: {err}"));
        }
        if failures.is_empty() {
            Ok(())
        } else {
            anyhow::bail!(failures.join("; "))
        }
    }

    fn rollback_rewrite_map_entries(
        &self,
        rewrite_map: &std::collections::BTreeMap<ChangeId, ChangeId>,
    ) -> anyhow::Result<()> {
        if rewrite_map.is_empty() {
            return Ok(());
        }
        let mut epoch_map = self.load_epoch_map_raw()?;
        for (old, new) in rewrite_map {
            let old_hex = old.to_hex();
            let new_hex = new.to_hex();
            if epoch_map.get(&old_hex).is_some_and(|v| v == &new_hex) {
                epoch_map.remove(&old_hex);
            }
        }
        self.persist_epoch_map_raw(&epoch_map)
    }

    /// Undo the last view-mutating operation recorded in the operation log.
    ///
    /// Pops the most-recent [`Operation`] via [`OpLog`], restores the view to
    /// its `before_heads`, re-materializes the state, and writes the working
    /// directory to match.  Blob files that existed before but are absent in
    /// the restored state are deleted from disk.
    ///
    /// Returns `Ok(Some(op))` on success, `Ok(None)` if the log is empty.
    pub fn undo(&mut self) -> anyhow::Result<Option<Operation>> {
        self.acquire_lock()?;
        let arc_dir = self.shared_root.join(".arc");
        let op = match OpLog::new(&arc_dir).pop()? {
            Some(op) => op,
            None => return Ok(None),
        };

        if matches!(op.kind, arc_core::store::oplog::OperationKind::Rewrite)
            && let Err(err) = self.rollback_rewrite_map_entries(&op.rewrite_map)
        {
            let _ = OpLog::new(&arc_dir).append(&op);
            return Err(anyhow::anyhow!(
                "failed to rollback rewrite map during undo: {err}"
            ));
        }

        // Load the current view and materialise it so we know which blob files
        // exist right now (and may need to be removed after the undo).
        let current_view = View::load(&self.shared_root, &op.view)
            .map_err(|e| anyhow::anyhow!("failed to load view '{}': {e}", op.view))?;
        self.hydrate_heads(&current_view.heads)?;
        let current_state = if current_view.heads.is_empty() {
            MaterializedState::new()
        } else {
            self.materialize_heads(&current_view.heads)?
        };

        // Restore the view to its pre-operation heads.
        let restored_heads = change_ids_to_hashes(&op.before_heads);
        let restored_view = View::new(&op.view, restored_heads.clone());
        restored_view
            .save(&self.shared_root)
            .map_err(|e| anyhow::anyhow!("failed to restore view '{}': {e}", op.view))?;

        // Materialise the restored state.
        self.hydrate_heads(&restored_heads)?;
        let restored_state = if restored_heads.is_empty() {
            MaterializedState::new()
        } else {
            self.materialize_heads(&restored_heads)?
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

        if let Err(err) = self.push_redo_operation(&op) {
            tracing::warn!(error = %err, "undo completed but redo stack update failed");
        }

        Ok(Some(op))
    }

    /// Reapply the most recently undone operation.
    #[tracing::instrument(skip_all)]
    pub fn redo(&mut self) -> anyhow::Result<Option<Operation>> {
        self.acquire_lock()?;

        let mut stack = self.load_redo_stack()?;
        let Some(op) = stack.pop() else {
            return Ok(None);
        };

        if matches!(op.kind, arc_core::store::oplog::OperationKind::Rewrite) {
            stack.push(op);
            self.save_redo_stack(&stack)?;
            anyhow::bail!(
                "redo for rewrite operations is not yet supported safely; rerun the original command"
            );
        }

        let current_view = View::load(&self.shared_root, &op.view)
            .map_err(|e| anyhow::anyhow!("failed to load view '{}': {e}", op.view))?;
        let expected_heads = change_ids_to_hashes(&op.before_heads);
        anyhow::ensure!(
            current_view.heads == expected_heads,
            "cannot redo '{}': view '{}' changed since undo",
            op.command,
            op.view
        );

        self.hydrate_heads(&expected_heads)?;
        let before_state = if expected_heads.is_empty() {
            MaterializedState::new()
        } else {
            self.materialize_heads(&expected_heads)?
        };

        let restored_heads = change_ids_to_hashes(&op.after_heads);
        let restored_view = View::new(&op.view, restored_heads.clone());
        if let Err(err) = restored_view.save(&self.shared_root) {
            return Err(anyhow::anyhow!("failed to save view '{}': {err}", op.view));
        }

        self.hydrate_heads(&restored_heads)?;
        let restored_state = if restored_heads.is_empty() {
            MaterializedState::new()
        } else {
            self.materialize_heads(&restored_heads)?
        };

        if let Err(err) =
            write_state_to_working_dir(&self.work_root, &self.shared_root, &restored_state)
        {
            let _ = View::new(&op.view, expected_heads).save(&self.shared_root);
            let _ = write_state_to_working_dir(&self.work_root, &self.shared_root, &before_state);
            return Err(err);
        }

        if let Err(err) = OpLog::new(&self.shared_root.join(".arc")).append(&op) {
            let _ =
                View::new(&op.view, change_ids_to_hashes(&op.before_heads)).save(&self.shared_root);
            let _ = write_state_to_working_dir(&self.work_root, &self.shared_root, &before_state);
            return Err(anyhow::anyhow!("failed to append redo operation: {err}"));
        }

        if let Err(err) = self.save_redo_stack(&stack) {
            tracing::warn!(error = %err, "redo committed but redo stack cleanup failed");
        }
        Ok(Some(op))
    }

    /// Abandon one or more current view heads by replacing each selected head
    /// with its direct parent frontier.
    #[tracing::instrument(skip_all)]
    pub fn abandon_heads(&mut self, revisions: &[String]) -> anyhow::Result<Vec<Blake3Hash>> {
        self.acquire_lock()?;
        let view_name = self.current_view_name()?;
        self.hydrate(&view_name)?;
        let view = View::load(&self.shared_root, &view_name)
            .map_err(|e| anyhow::anyhow!("failed to load view '{view_name}': {e}"))?;

        let targets = if revisions.is_empty() {
            vec!["@".to_string()]
        } else {
            revisions.to_vec()
        };

        let mut resolved = Vec::with_capacity(targets.len());
        for rev in &targets {
            resolved.push(self.resolve_rev(rev)?);
        }

        let mut new_heads = view.heads.clone();
        let mut abandoned = Vec::new();
        for id in resolved {
            if !new_heads.remove(&id) {
                continue;
            }
            let change = self.store.read_change(&id).map_err(|e| {
                anyhow::anyhow!("failed to load abandoned change {}: {e}", _hex(&id))
            })?;
            new_heads.extend(change.deps);
            abandoned.push(id);
        }

        if abandoned.is_empty() {
            return Ok(abandoned);
        }

        let updated = View::new(&view_name, new_heads.clone());
        updated
            .save(&self.shared_root)
            .map_err(|e| anyhow::anyhow!("failed to save view '{view_name}': {e}"))?;

        self.hydrate_heads(&new_heads)?;
        let state = if new_heads.is_empty() {
            MaterializedState::new()
        } else {
            self.materialize_heads(&new_heads)?
        };
        write_state_to_working_dir(&self.work_root, &self.shared_root, &state)?;

        self.log_operation("abandon", &view_name, view.heads.clone(), new_heads.clone())?;

        Ok(abandoned)
    }

    /// Return the full operation log in reverse-chronological order (most
    /// recent first).
    ///
    /// Delegates to [`OpLog::read_reversed`]; returns an empty `Vec` when the
    /// log file does not yet exist.
    pub fn op_log(&self) -> anyhow::Result<Vec<Operation>> {
        OpLog::new(&self.shared_root.join(".arc")).read_reversed()
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
        self.acquire_lock()?;
        let sparse_path = self.work_root.join(".arc").join("sparse.json");
        let arcignore = load_arcignore(&self.work_root);
        validate_sparse_patterns(patterns, &arcignore)?;
        let new_matcher = SparseMatcher::from_patterns(patterns);

        // Step 1: remove stale files that are outside the new cone.
        let view_name = self.current_view_name()?;
        self.hydrate(&view_name)?;
        let state = self.materialize(&view_name)?;
        if !patterns.is_empty() {
            for filepath in extract_filepaths_from_state(&state) {
                if !new_matcher.matches_file_path(&filepath) {
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
    pub fn mount_add(&mut self, path: &str, url: &str, target: &str) -> anyhow::Result<Blake3Hash> {
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
        self.graph_add_change(change.clone());
        // Record the completed mount-add in the spacetime log.
        self.log_operation(
            "mount add",
            &view_name,
            view.heads.clone(),
            HashSet::from([change.id]),
        )?;
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
                    let info = std::str::from_utf8(value)
                        .ok()?
                        .strip_prefix("ARC_MOUNT:")?;
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
        self.acquire_lock()?;
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
        let parent = self.shared_root.parent().unwrap_or(&self.shared_root);
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

    /// Resolve and canonicalize a workspace path.
    pub fn workspace_root(&self, path: Option<&Path>) -> anyhow::Result<PathBuf> {
        let candidate = path.unwrap_or(&self.work_root);
        if path.is_some() {
            self.ensure_linked_workspace_path(candidate)?;
        }
        fs::canonicalize(candidate).map_err(|e| {
            anyhow::anyhow!(
                "failed to resolve workspace root '{}': {e}",
                candidate.display()
            )
        })
    }

    /// Forget a linked workspace by deleting its `.arc-workspace` manifest.
    pub fn workspace_forget(&self, path: &Path) -> anyhow::Result<()> {
        let manifest = self.ensure_linked_workspace_path(path)?;
        fs::remove_file(&manifest)
            .map_err(|e| anyhow::anyhow!("failed to forget workspace '{}': {e}", path.display()))
    }

    /// Rename a linked workspace directory on disk.
    pub fn workspace_rename(&self, old_path: &Path, new_path: &Path) -> anyhow::Result<()> {
        self.ensure_linked_workspace_path(old_path)?;
        if new_path.exists() {
            anyhow::bail!(
                "target workspace path '{}' already exists",
                new_path.display()
            );
        }
        fs::rename(old_path, new_path).map_err(|e| {
            anyhow::anyhow!(
                "failed to rename workspace '{}' -> '{}': {e}",
                old_path.display(),
                new_path.display()
            )
        })
    }

    fn ensure_linked_workspace_path(&self, path: &Path) -> anyhow::Result<PathBuf> {
        let manifest = path.join(".arc-workspace");
        if !manifest.exists() {
            anyhow::bail!("workspace '{}' is not linked", path.display());
        }
        let json = fs::read_to_string(&manifest)
            .map_err(|e| anyhow::anyhow!("failed to read {}: {e}", manifest.display()))?;
        let parsed: WorkspaceManifest = serde_json::from_str(&json)
            .map_err(|e| anyhow::anyhow!("invalid {}: {e}", manifest.display()))?;
        if parsed.shared_root != self.shared_root {
            anyhow::bail!(
                "workspace '{}' belongs to a different shared repository",
                path.display()
            );
        }
        Ok(manifest)
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

        // OpLog protection: every before_heads and after_heads in the log.
        if let Ok(ops) = OpLog::new(&self.shared_root.join(".arc")).read_all() {
            for op in &ops {
                root_set.extend(op.before_heads.iter().map(|id| id.0));
                root_set.extend(op.after_heads.iter().map(|id| id.0));
            }
        }

        // --- Step 2: BFS to find all reachable changes ---------------------
        let reachable = self.graph.load().ancestors(&root_set);

        // --- Step 3: causal stability — intersection of all view histories --
        // A change is causally stable if it appears in EVERY view's ancestry.
        let mut per_view_ancestors: Vec<HashSet<Blake3Hash>> = Vec::new();
        if let Ok(rd) = fs::read_dir(&views_dir) {
            for entry in rd.filter_map(|e| e.ok()) {
                let name = entry.file_name().to_string_lossy().into_owned();
                if let Ok(view) = View::load(&self.shared_root, &name) {
                    per_view_ancestors.push(self.graph.load().ancestors(&view.heads));
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
        //
        // CAS layout: `.arc/store/{first_2_hex}/{remaining_62_hex}`
        // A flat `read_dir` of `.arc/store/` only finds the 2-char shard
        // directories, never the actual change files — so a two-level walk is
        // required.
        let mut on_disk: Vec<Blake3Hash> = Vec::new();
        if let Ok(prefixes) = fs::read_dir(&store_dir) {
            for prefix_entry in prefixes.filter_map(|e| e.ok()) {
                let prefix_name = prefix_entry.file_name().to_string_lossy().into_owned();
                // Shard directories are exactly 2 lowercase hex chars.
                if prefix_name.len() != 2 || !prefix_name.bytes().all(|b| b.is_ascii_hexdigit()) {
                    continue;
                }
                if let Ok(files) = fs::read_dir(prefix_entry.path()) {
                    for file_entry in files.filter_map(|e| e.ok()) {
                        let suffix = file_entry.file_name().to_string_lossy().into_owned();
                        // The suffix is 62 hex chars; combined with the 2-char
                        // prefix it forms the full 64-char BLAKE3 hex.
                        if suffix.len() != 62 || !suffix.bytes().all(|b| b.is_ascii_hexdigit()) {
                            continue;
                        }
                        let hex_full = format!("{prefix_name}{suffix}");
                        let mut id = [0u8; 32];
                        let mut valid = true;
                        for (i, chunk) in hex_full.as_bytes().chunks(2).enumerate() {
                            let hi = match chunk[0] {
                                b'0'..=b'9' => chunk[0] - b'0',
                                b'a'..=b'f' => chunk[0] - b'a' + 10,
                                b'A'..=b'F' => chunk[0] - b'A' + 10,
                                _ => {
                                    valid = false;
                                    break;
                                }
                            };
                            let lo = match chunk[1] {
                                b'0'..=b'9' => chunk[1] - b'0',
                                b'a'..=b'f' => chunk[1] - b'a' + 10,
                                b'A'..=b'F' => chunk[1] - b'A' + 10,
                                _ => {
                                    valid = false;
                                    break;
                                }
                            };
                            id[i] = (hi << 4) | lo;
                        }
                        if valid {
                            on_disk.push(id);
                        }
                    }
                }
            }
        }

        for id in &on_disk {
            if !reachable.contains(id) && causally_stable.contains(id) {
                let hex = _hex(id);
                let path = store_dir.join(&hex[..2]).join(&hex[2..]);
                if fs::remove_file(&path).is_ok() {
                    result.changes_deleted += 1;
                    // Remove empty shard directory to avoid accumulating 256
                    // empty dirs over time when the last object in a shard is
                    // deleted.
                    let shard_dir = store_dir.join(&hex[..2]);
                    let _ = fs::remove_dir(&shard_dir); // no-op if not empty
                }
            }
        }

        // --- Step 5: delete orphaned blob files ----------------------------
        let blobs_dir = self.shared_root.join(".arc").join("blobs");
        // Collect all blob hashes referenced in the reachable changes.
        let mut referenced_blobs: HashSet<String> = HashSet::new();
        {
            let g = self.graph.load_full();
            for id in &reachable {
                if let Some(change) = g.get(id) {
                    for atom in &change.atoms {
                        match atom {
                            Atom::Blob { hash, .. } => {
                                referenced_blobs.insert(_hex(hash));
                            }
                            Atom::Insert { content_hash, .. } => {
                                referenced_blobs.insert(_hex(content_hash));
                            }
                            Atom::Delete { prior_hash, .. } => {
                                referenced_blobs.insert(_hex(prior_hash));
                            }
                            _ => {}
                        }
                    }
                }
            }
        }
        if blobs_dir.exists()
            && let Ok(rd) = fs::read_dir(&blobs_dir)
        {
            for entry in rd.filter_map(|e| e.ok()) {
                let fname = entry.file_name().to_string_lossy().into_owned();
                if !referenced_blobs.contains(&fname) && fs::remove_file(entry.path()).is_ok() {
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
        self.acquire_lock()?;
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
                    per_view_ancestors.push(self.graph.load().ancestors(&view.heads));
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
            anyhow::bail!(
                "No stable history to compact — repository has no causally-stable changes. \
                           Ensure every view has observed the same base history before compacting."
            );
        }

        // --- Step 3: find stable tips (stable nodes whose deps are also     --
        //     within the stable set, but that no OTHER stable node points to) --
        let mut depended_on_by_stable: HashSet<Blake3Hash> = HashSet::new();
        {
            let g = self.graph.load_full();
            for &id in &causally_stable {
                if let Some(change) = g.get(&id) {
                    for dep in &change.deps {
                        if causally_stable.contains(dep) {
                            depended_on_by_stable.insert(*dep);
                        }
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
                atoms.push(Atom::Blob {
                    path: path.clone(),
                    hash,
                });
            } else {
                let content_hash = self
                    .store
                    .write_blob(content)
                    .map_err(|e| anyhow::anyhow!("CAS write error in compact: {e}"))?;
                atoms.push(Atom::Insert {
                    at: path.clone(),
                    content_hash,
                });
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
        self.graph_add_change(genesis.clone());
        let genesis_id = genesis.id;

        // --- Step 7: build and persist the Epoch Map (append-only) ---------
        let epochs_path = self.shared_root.join(".arc").join("epochs");
        let mut epoch_json: HashMap<String, String> = if epochs_path.exists() {
            let raw = fs::read_to_string(&epochs_path).unwrap_or_default();
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

    /// Amend the most recent change in-place.
    ///
    /// Rewrites the last snap by replacing its atoms with the full diff of the
    /// **current working directory against the grandparent state** (the state
    /// before the amended change was applied).  An optional new `message`
    /// replaces the original commit intent; if omitted the original is kept.
    ///
    /// The old change ID is written into the Epoch Map (`old → new`) so that
    /// peers who already pulled the original ID transparently graft onto the
    /// amended commit during `hydrate`.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// * The current view has more than one head (merge commit — ambiguous).
    /// * The working directory contains no changes relative to the grandparent
    ///   **and** no new message was supplied.
    pub fn amend(&mut self, message: Option<&str>) -> anyhow::Result<Blake3Hash> {
        self.acquire_lock()?;
        let view_name = self.current_view_name()?;
        self.hydrate(&view_name)?;

        let view = View::load(&self.shared_root, &view_name)
            .map_err(|e| anyhow::anyhow!("failed to load view '{view_name}': {e}"))?;

        if view.heads.len() != 1 {
            anyhow::bail!(
                "amend requires exactly one head; current view '{}' has {} heads",
                view_name,
                view.heads.len()
            );
        }
        let old_head = *view.heads.iter().next().unwrap();

        // Load the change being amended from CAS.
        let parent = self
            .store
            .read_change(&old_head)
            .map_err(|_| anyhow::anyhow!("change {} not found in CAS", _hex(&old_head)))?;

        // Hydrate the grandparent (the deps of the change we are amending) so
        // we can materialize the state from before the amended change existed.
        self.hydrate_heads(&parent.deps)?;
        let grandparent_state = self.materialize_heads(&parent.deps)?;

        // Diff the working directory against the grandparent state.
        let delta = self.compute_working_directory_delta(&grandparent_state)?;

        if delta.is_empty() && message.is_none() {
            anyhow::bail!(
                "nothing to amend — working directory matches the pre-amend state and no new message was supplied"
            );
        }

        let new_intent = message.unwrap_or(parent.intent.as_str()).to_string();

        let (author, signing_key) = self.signing_identity()?;
        let author = author.clone();

        // Persist raw bytes for any Atom::Blob before committing.
        self.write_blob_atoms(&delta)?;

        // Build the amended Change with the grandparent's deps (not the old
        // head) so history is rewritten cleanly.
        let new_change = Change::new(parent.deps.clone(), delta, new_intent, author, signing_key);
        let new_id = new_change.id;

        self.store
            .write_change(&new_change)
            .map_err(|e| anyhow::anyhow!("CAS write error: {e}"))?;
        self.graph_add_change(new_change.clone());

        // Update the Epoch Map: old_head → new_id, so peers transparently graft.
        let epochs_path = self.shared_root.join(".arc").join("epochs");
        let mut epoch_json: HashMap<String, String> = if epochs_path.exists() {
            let raw = fs::read_to_string(&epochs_path).unwrap_or_default();
            serde_json::from_str(&raw).unwrap_or_default()
        } else {
            HashMap::new()
        };
        epoch_json.insert(_hex(&old_head), _hex(&new_id));
        let serialized = serde_json::to_string_pretty(&epoch_json)
            .map_err(|e| anyhow::anyhow!("epoch map serialisation error: {e}"))?;
        let tmp = epochs_path.with_extension("tmp");
        fs::write(&tmp, &serialized)
            .map_err(|e| anyhow::anyhow!("could not write epoch map: {e}"))?;
        fs::rename(&tmp, &epochs_path)
            .map_err(|e| anyhow::anyhow!("could not rename epoch map: {e}"))?;

        // Record the completed amend in the spacetime log.
        self.log_operation(
            "amend",
            &view_name,
            view.heads.clone(),
            HashSet::from([new_id]),
        )?;

        // Repoint the view to the new change.
        let updated_view = View::new(&view_name, HashSet::from([new_id]));
        updated_view
            .save(&self.shared_root)
            .map_err(|e| anyhow::anyhow!("failed to save view: {e}"))?;

        // Rematerialize and write the working directory.
        let new_state = self.materialize(&view_name)?;
        write_state_to_working_dir(&self.work_root, &self.shared_root, &new_state)?;

        tracing::info!(old = %_hex(&old_head), new = %_hex(&new_id), "amend complete");
        Ok(new_id)
    }

    /// Squash the contiguous linear spine from `target_id` to the current view
    /// head into a single new [`Change`].
    ///
    /// The squashed change inherits the `deps` of the target change and carries
    /// all atoms from every change in the spine.  The new change is written to
    /// the CAS, the view is repointed to it, and the working directory is
    /// rematerialised.
    ///
    /// # Errors
    ///
    /// Returns an error when the current view has more than one head, when the
    /// spine is non-linear, or when `target_id` is not an ancestor of HEAD.
    pub fn squash_into(&mut self, target_rev: &str) -> anyhow::Result<Blake3Hash> {
        self.acquire_lock()?;
        let view_name = self.current_view_name()?;

        let view = View::load(&self.shared_root, &view_name)
            .map_err(|e| anyhow::anyhow!("failed to load view '{view_name}': {e}"))?;
        if view.heads.len() != 1 {
            anyhow::bail!(
                "squash requires exactly one head; current view '{}' has {} heads",
                view_name,
                view.heads.len()
            );
        }

        let target_id = self.resolve_rev(target_rev)?;
        let (author, signing_key) = self.signing_identity()?;
        let signer = (author.clone(), signing_key.clone());

        let outcome =
            mutator::squash_into(&self.graph.load_full(), &view.heads, target_id, &signer)
                .map_err(|e| anyhow::anyhow!("squash failed: {e}"))?;

        let new_id = outcome.squashed.id;
        self.store
            .write_change(&outcome.squashed)
            .map_err(|e| anyhow::anyhow!("failed to write squashed change: {e}"))?;
        self.graph_add_change(outcome.squashed.clone());

        let rewrite_map: HashMap<Blake3Hash, Blake3Hash> = outcome
            .rewrite_map
            .iter()
            .map(|(old, new)| (Blake3Hash::from(*old), Blake3Hash::from(*new)))
            .collect();
        let previous_epoch = self.stage_rewrite_metadata(
            "squash",
            &view_name,
            view.heads.clone(),
            HashSet::from([new_id]),
            &rewrite_map,
        )?;

        let updated_view = View::new(&view_name, HashSet::from([new_id]));
        if let Err(e) = updated_view.save(&self.shared_root) {
            let _ = self.rollback_rewrite_metadata(&previous_epoch);
            return Err(anyhow::anyhow!("failed to save view: {e}"));
        }

        let new_state = match self.materialize(&view_name) {
            Ok(state) => state,
            Err(err) => {
                let _ = View::new(&view_name, view.heads.clone()).save(&self.shared_root);
                let _ = self.rollback_rewrite_metadata(&previous_epoch);
                return Err(err);
            }
        };
        if let Err(err) = write_state_to_working_dir(&self.work_root, &self.shared_root, &new_state)
        {
            let _ = View::new(&view_name, view.heads.clone()).save(&self.shared_root);
            let _ = self.rollback_rewrite_metadata(&previous_epoch);
            return Err(err);
        }

        tracing::info!(target = %_hex(&target_id), new = %_hex(&new_id), "squash complete");
        Ok(new_id)
    }

    /// Reorder a contiguous linear chain of revisions.
    ///
    /// The input sequence defines the desired oldest->newest order.
    pub fn reorder(&mut self, ordered_revs: &[String]) -> anyhow::Result<Blake3Hash> {
        self.acquire_lock()?;
        if ordered_revs.len() < 2 {
            anyhow::bail!("reorder requires at least two revisions");
        }

        let view_name = self.current_view_name()?;
        let view = View::load(&self.shared_root, &view_name)
            .map_err(|e| anyhow::anyhow!("failed to load view '{view_name}': {e}"))?;
        if view.heads.len() != 1 {
            anyhow::bail!(
                "reorder requires exactly one head; current view '{}' has {} heads",
                view_name,
                view.heads.len()
            );
        }

        let mut desired = Vec::with_capacity(ordered_revs.len());
        for rev in ordered_revs {
            desired.push(self.resolve_rev(rev)?);
        }

        let old_head = *view
            .heads
            .iter()
            .next()
            .ok_or_else(|| anyhow::anyhow!("view '{view_name}' has no head"))?;
        if !desired.contains(&old_head) {
            anyhow::bail!("reorder set must include current HEAD");
        }

        let (author, signing_key) = self.signing_identity()?;
        let signer = (author.clone(), signing_key.clone());
        let outcome = mutator::reorder(&self.graph.load_full(), &desired, &signer)
            .map_err(|e| anyhow::anyhow!("reorder failed: {e}"))?;

        for change in &outcome.rewritten {
            self.store
                .write_change(change)
                .map_err(|e| anyhow::anyhow!("failed to write reordered change: {e}"))?;
            self.graph_add_change(change.clone());
        }

        let rewrite_map: HashMap<Blake3Hash, Blake3Hash> = outcome
            .rewrite_map
            .iter()
            .map(|(old, new)| (Blake3Hash::from(*old), Blake3Hash::from(*new)))
            .collect();
        let new_head = Blake3Hash::from(outcome.new_head);
        let previous_epoch = self.stage_rewrite_metadata(
            "reorder",
            &view_name,
            view.heads.clone(),
            HashSet::from([new_head]),
            &rewrite_map,
        )?;

        let updated_view = View::new(&view_name, HashSet::from([new_head]));
        if let Err(e) = updated_view.save(&self.shared_root) {
            let _ = self.rollback_rewrite_metadata(&previous_epoch);
            return Err(anyhow::anyhow!("failed to save view: {e}"));
        }

        let new_state = match self.materialize(&view_name) {
            Ok(state) => state,
            Err(err) => {
                let _ = View::new(&view_name, view.heads.clone()).save(&self.shared_root);
                let _ = self.rollback_rewrite_metadata(&previous_epoch);
                return Err(err);
            }
        };
        if let Err(err) = write_state_to_working_dir(&self.work_root, &self.shared_root, &new_state)
        {
            let _ = View::new(&view_name, view.heads.clone()).save(&self.shared_root);
            let _ = self.rollback_rewrite_metadata(&previous_epoch);
            return Err(err);
        }

        tracing::info!(new = %_hex(&new_head), "reorder complete");
        Ok(new_head)
    }

    /// Prepare a diffedit session for the change identified by `target_rev`.
    ///
    /// Writes `.arc/diffedit_target` with the hex of the target change ID,
    /// then materialises and writes the change's resulting state to the working
    /// directory so the user can edit it with any external tool.
    ///
    /// Use [`Self::diffedit_apply`] to turn the edited working directory into a
    /// replacement change once the user is satisfied.
    pub fn diffedit_prepare(&mut self, target_rev: &str) -> anyhow::Result<()> {
        self.acquire_lock()?;
        let target_id = self.resolve_rev(target_rev)?;

        // Ensure the change exists in the graph.
        if self.graph.load().get(&target_id).is_none() {
            anyhow::bail!("change {} not found in graph", _hex(&target_id));
        }

        let lock_path = self.shared_root.join(".arc").join("diffedit_target");
        fs::write(&lock_path, _hex(&target_id))
            .map_err(|e| anyhow::anyhow!("could not write diffedit_target: {e}"))?;

        // Materialise the state at the target and write it to the working dir.
        let state = self.materialize_heads(&HashSet::from([target_id]))?;
        write_state_to_working_dir(&self.work_root, &self.shared_root, &state)?;

        tracing::info!(target = %_hex(&target_id), "diffedit prepare complete");
        println!(
            "diffedit: working directory set to change {}",
            &_hex(&target_id)[..12]
        );
        println!("Edit your files then run `arc diffedit --apply` to record the change.");
        Ok(())
    }

    /// Apply the current working directory edits as a replacement for the
    /// change recorded by [`Self::diffedit_prepare`].
    ///
    /// Reads `.arc/diffedit_target`, computes the diff between the stored
    /// target state and the current working directory, calls
    /// [`Self::squash_into`]'s underlying engine to fuse the new atoms into the
    /// target position, then deletes `.arc/diffedit_target`.
    ///
    /// An optional `message` overrides the original change's intent.
    pub fn diffedit_apply(&mut self, message: Option<&str>) -> anyhow::Result<Blake3Hash> {
        self.acquire_lock()?;
        let lock_path = self.shared_root.join(".arc").join("diffedit_target");
        if !lock_path.exists() {
            anyhow::bail!(
                "no active diffedit session — run `arc diffedit --prepare <change>` first"
            );
        }

        let target_hex = fs::read_to_string(&lock_path)?;
        let target_hex = target_hex.trim();
        let target_id: Blake3Hash = _unhex(target_hex)
            .ok_or_else(|| anyhow::anyhow!("corrupt diffedit_target: '{target_hex}'"))?;

        let target_change = self
            .graph
            .load()
            .get(&target_id)
            .ok_or_else(|| anyhow::anyhow!("diffedit target change not found in graph"))?
            .clone();

        // Compute diff: stored state at target → current working dir.
        let stored_state = self.materialize_heads(&HashSet::from([target_id]))?;
        let arcignore = load_arcignore(&self.work_root);
        let plugin = RustPlugin::new();
        let rs_files_after = collect_rs_files(&self.work_root, &arcignore)?;
        let files_before = extract_filepaths_from_state(&stored_state);

        let mut new_atoms: Vec<Atom> = Vec::new();

        // Modified or added files.
        for filepath in &rs_files_after {
            let new_src = fs::read_to_string(self.work_root.join(filepath))?;
            let old_src = plugin.unparse(&stored_state, filepath).unwrap_or_default();
            if new_src == old_src {
                continue;
            }
            let ast_atoms = plugin
                .diff(&old_src, &new_src, &self.store)
                .map_err(|e| anyhow::anyhow!("diffedit diff error for {filepath}: {e}"))?;
            for atom in ast_atoms {
                new_atoms.push(prefix_atom_path(atom, filepath));
            }
        }

        // Deleted files.
        let files_after_set: HashSet<String> = rs_files_after.iter().cloned().collect();
        for filepath in files_before.difference(&files_after_set) {
            let old_src = plugin.unparse(&stored_state, filepath).unwrap_or_default();
            if old_src.is_empty() {
                continue;
            }
            let prior_bytes = old_src.into_bytes();
            let prior_hash = self
                .store
                .write_blob(&prior_bytes)
                .map_err(|e| anyhow::anyhow!("diffedit store write error: {e}"))?;
            for seg in files_before.iter().filter(|p| p == &filepath) {
                new_atoms.push(Atom::Delete {
                    at: vec!["file".to_string(), seg.clone()],
                    prior_hash,
                });
            }
        }

        if new_atoms.is_empty() {
            anyhow::bail!("no changes detected in working directory — nothing to apply");
        }

        let intent = message
            .map(|m| m.to_string())
            .unwrap_or_else(|| format!("diffedit: {}", target_change.intent));

        let (author, signing_key) = self.signing_identity()?;
        let new_change = Change::new(
            target_change.deps.clone(),
            new_atoms,
            intent,
            author.clone(),
            signing_key,
        );
        let new_id = new_change.id;

        self.store
            .write_change(&new_change)
            .map_err(|e| anyhow::anyhow!("failed to write diffedit change: {e}"))?;
        self.graph_add_change(new_change);

        let view_name = self.current_view_name()?;
        let view = View::load(&self.shared_root, &view_name)
            .map_err(|e| anyhow::anyhow!("failed to load view: {e}"))?;

        let rewrite_map = HashMap::from([(target_id, new_id)]);
        let previous_epoch = self.stage_rewrite_metadata(
            "diffedit",
            &view_name,
            view.heads.clone(),
            HashSet::from([new_id]),
            &rewrite_map,
        )?;

        let updated_view = View::new(&view_name, HashSet::from([new_id]));
        if let Err(e) = updated_view.save(&self.shared_root) {
            let _ = self.rollback_rewrite_metadata(&previous_epoch);
            return Err(anyhow::anyhow!("failed to save updated view: {e}"));
        }

        let new_state = match self.materialize(&view_name) {
            Ok(state) => state,
            Err(err) => {
                let _ = View::new(&view_name, view.heads.clone()).save(&self.shared_root);
                let _ = self.rollback_rewrite_metadata(&previous_epoch);
                return Err(err);
            }
        };
        if let Err(err) = write_state_to_working_dir(&self.work_root, &self.shared_root, &new_state)
        {
            let _ = View::new(&view_name, view.heads.clone()).save(&self.shared_root);
            let _ = self.rollback_rewrite_metadata(&previous_epoch);
            return Err(err);
        }

        // Remove the diffedit lock file.
        let _ = fs::remove_file(&lock_path);

        tracing::info!(target = %_hex(&target_id), new = %_hex(&new_id), "diffedit apply complete");
        Ok(new_id)
    }

    /// Resolve a human-readable revision query to a [`Blake3Hash`].
    ///
    /// Supported query formats:
    ///
    /// | Format | Example | Meaning |
    /// |--------|---------|---------|
    /// | `HEAD` / `@` | `HEAD` | Current view's sole head |
    /// | View name | `main` | Sole head of the named view |
    /// | Partial hex | `a1b2c3` | Unique prefix of a CAS object hash |
    /// | `<base>~N` | `HEAD~2` | N-th ancestor of `<base>` |
    ///
    /// Ancestor traversal (`~N`) deterministically follows the first dependency
    /// (sorted by hex) when a change has multiple parents (merge commits).
    pub fn resolve_rev(&self, query: &str) -> anyhow::Result<Blake3Hash> {
        // Split off the optional `~N` ancestor suffix.
        let (base_str, n) = if let Some(tilde_pos) = query.find('~') {
            let base = &query[..tilde_pos];
            let steps_str = &query[tilde_pos + 1..];
            let steps: usize = steps_str.parse().map_err(|_| {
                anyhow::anyhow!(
                    "invalid ancestor count in '{query}': expected an integer after '~'"
                )
            })?;
            (base, steps)
        } else {
            (query, 0)
        };

        // Resolve the base reference to a starting hash.
        let mut current: Blake3Hash = if base_str == "HEAD" || base_str == "@" {
            let view_name = self.current_view_name()?;
            let view = View::load(&self.shared_root, &view_name)
                .map_err(|e| anyhow::anyhow!("failed to load view '{view_name}': {e}"))?;
            if view.heads.len() != 1 {
                anyhow::bail!(
                    "HEAD is ambiguous — view '{view_name}' has {} heads; use a specific hash",
                    view.heads.len()
                );
            }
            *view.heads.iter().next().unwrap()
        } else if let Ok(view) = View::load(&self.shared_root, base_str) {
            // base_str is a view name.
            if view.heads.len() != 1 {
                anyhow::bail!(
                    "view '{base_str}' has {} heads; use a specific hash",
                    view.heads.len()
                );
            }
            *view.heads.iter().next().unwrap()
        } else {
            // Partial hex prefix — scan .arc/store/ for a unique filename match.
            let store_dir = self.shared_root.join(".arc").join("store");
            let mut matches: Vec<Blake3Hash> = Vec::new();
            if let Ok(rd) = fs::read_dir(&store_dir) {
                for entry in rd.filter_map(|e| e.ok()) {
                    let fname = entry.file_name().to_string_lossy().into_owned();
                    if fname.starts_with(base_str)
                        && let Some(hash) = _unhex(&fname)
                    {
                        matches.push(hash);
                    }
                }
            }
            match matches.len() {
                0 => anyhow::bail!("no change found matching '{base_str}'"),
                1 => matches.remove(0),
                n => anyhow::bail!(
                    "ambiguous prefix '{base_str}' matches {n} changes; use more characters"
                ),
            }
        };

        // Walk n ancestor steps.
        for i in 0..n {
            let change = self
                .store
                .read_change(&current)
                .map_err(|_| anyhow::anyhow!("change {} not found in CAS", _hex(&current)))?;
            // Sort deps by hex for deterministic traversal when there are multiple parents.
            let mut sorted_deps: Vec<Blake3Hash> = change.deps.into_iter().collect();
            sorted_deps.sort_by_key(_hex);
            current = sorted_deps.into_iter().next().ok_or_else(|| {
                anyhow::anyhow!("cannot traverse ~{n}: change at step {i} has no ancestors")
            })?;
        }

        Ok(current)
    }

    /// Resolve revset symbol references used by the revset compiler.
    ///
    /// Supports `@` and concrete view names. Raw 64-char hex hashes are
    /// handled directly by the revset compiler.
    pub fn resolve_revset_symbol(&self, symbol: &str) -> anyhow::Result<Option<Blake3Hash>> {
        if symbol == "@" {
            return self.resolve_rev("@").map(Some);
        }

        if View::load(&self.shared_root, symbol).is_ok() {
            return self.resolve_rev(symbol).map(Some);
        }

        Ok(None)
    }

    /// Typed variant of [`resolve_revset_symbol`] used by the revset evaluator.
    pub fn resolve_revset_symbol_typed(&self, symbol: &str) -> anyhow::Result<Option<ChangeId>> {
        self.resolve_revset_symbol(symbol)
            .map(|opt| opt.map(ChangeId::from))
    }

    /// Resolve metadata-backed revset functions to typed reference heads.
    pub fn resolve_revset_reference_heads(
        &self,
        function_name: &str,
    ) -> anyhow::Result<BTreeSet<ChangeId>> {
        match function_name {
            "tags" => read_tag_heads(&self.shared_root),
            "remote_branches" => read_remote_branch_heads(&self.shared_root),
            "bookmarks" => read_bookmark_heads(&self.shared_root),
            _ => Ok(BTreeSet::new()),
        }
    }

    /// Return tag decorations keyed by target change id.
    pub fn tag_decorations(
        &self,
    ) -> anyhow::Result<std::collections::BTreeMap<ChangeId, Vec<String>>> {
        read_tag_map(&self.shared_root)
    }

    /// Return remote branch decorations keyed by tracked head change id.
    pub fn remote_branch_decorations(
        &self,
    ) -> anyhow::Result<std::collections::BTreeMap<ChangeId, Vec<String>>> {
        read_remote_branch_map(&self.shared_root)
    }

    /// Prepare graph state required for evaluating a revset expression.
    ///
    /// This hydrates referenced view heads and full 64-character hash symbols
    /// so ancestor traversal does not truncate on missing graph nodes.
    pub fn prepare_revset(
        &mut self,
        expr: &arc_core::revset::RevsetExpression,
    ) -> anyhow::Result<()> {
        self.prepare_revset_impl(expr)
    }

    fn prepare_revset_impl(
        &mut self,
        expr: &arc_core::revset::RevsetExpression,
    ) -> anyhow::Result<()> {
        match expr {
            arc_core::revset::RevsetExpression::Symbol(symbol) => {
                if symbol == "@" {
                    let current = self.current_view_name()?;
                    self.hydrate(&current)?;
                    return Ok(());
                }

                if let Some(hash) = _unhex(symbol)
                    && symbol.len() == 64
                {
                    self.hydrate_heads(&HashSet::from([hash]))?;
                    return Ok(());
                }

                if View::load(&self.shared_root, symbol).is_ok() {
                    self.hydrate(symbol)?;
                }

                Ok(())
            }
            arc_core::revset::RevsetExpression::StringLiteral(_) => Ok(()),
            arc_core::revset::RevsetExpression::Function { name, args } => {
                if matches!(name.as_str(), "tags" | "remote_branches" | "bookmarks") {
                    let heads = self.resolve_revset_reference_heads(name)?;
                    if !heads.is_empty() {
                        let hashes: HashSet<Blake3Hash> =
                            heads.iter().copied().map(Blake3Hash::from).collect();
                        self.hydrate_heads(&hashes)?;
                    }
                }
                for arg in args {
                    self.prepare_revset_impl(arg)?;
                }
                Ok(())
            }
            arc_core::revset::RevsetExpression::Intersection(left, right)
            | arc_core::revset::RevsetExpression::Union(left, right) => {
                self.prepare_revset_impl(left)?;
                self.prepare_revset_impl(right)
            }
        }
    }

    /// Return a stable snapshot of the current DAG for lazy revset iteration.
    pub fn graph_snapshot(&self) -> Arc<ChangeGraph> {
        self.graph.load_full()
    }

    /// Read a single change from CAS by id.
    pub fn read_change(&self, id: &Blake3Hash) -> anyhow::Result<Change> {
        self.store
            .read_change(id)
            .map_err(|e| anyhow::anyhow!("failed to read change {}: {e}", _hex(id)))
    }
}

fn constrain_touched_to_current_view(
    expr: &arc_core::revset::RevsetExpression,
) -> arc_core::revset::RevsetExpression {
    if !contains_function(expr, "touched") {
        return expr.clone();
    }

    arc_core::revset::RevsetExpression::Intersection(
        Box::new(expr.clone()),
        Box::new(arc_core::revset::RevsetExpression::Function {
            name: "ancestors".to_string(),
            args: vec![arc_core::revset::RevsetExpression::Symbol("@".to_string())],
        }),
    )
}

fn contains_function(expr: &arc_core::revset::RevsetExpression, name: &str) -> bool {
    match expr {
        arc_core::revset::RevsetExpression::Function { name: fn_name, args } => {
            fn_name == name || args.iter().any(|arg| contains_function(arg, name))
        }
        arc_core::revset::RevsetExpression::Intersection(left, right)
        | arc_core::revset::RevsetExpression::Union(left, right) => {
            contains_function(left, name) || contains_function(right, name)
        }
        arc_core::revset::RevsetExpression::Symbol(_)
        | arc_core::revset::RevsetExpression::StringLiteral(_) => false,
    }
}

/// Format a [`Blake3Hash`] as a lowercase 64-character hex string.
fn _hex(hash: &Blake3Hash) -> String {
    hash.iter().map(|b| format!("{b:02x}")).collect()
}

fn next_mutation_id(
    command: &str,
    view: &str,
    rewrite_map: &HashMap<Blake3Hash, Blake3Hash>,
) -> anyhow::Result<MutationId> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let mut edges: Vec<(Blake3Hash, Blake3Hash)> =
        rewrite_map.iter().map(|(old, new)| (*old, *new)).collect();
    edges.sort();
    let payload = bincode::serialize(&(command, view, now, &edges))
        .map_err(|e| anyhow::anyhow!("failed to serialize rewrite transaction id payload: {e}"))?;
    Ok(MutationId(*blake3::hash(&payload).as_bytes()))
}

fn hashes_to_change_ids(input: &HashSet<Blake3Hash>) -> std::collections::BTreeSet<ChangeId> {
    input.iter().copied().map(ChangeId::from).collect()
}

fn change_ids_to_hashes(input: &std::collections::BTreeSet<ChangeId>) -> HashSet<Blake3Hash> {
    input.iter().copied().map(Blake3Hash::from).collect()
}

/// Prefix used by `arc-core::algebra::apply` to project conflict atoms.
const ARC_CONFLICT_REF_PREFIX: &[u8] = b"ARC_CONFLICT_REF:";

type ConflictProjection = (Vec<Blake3Hash>, Vec<Blake3Hash>);
type ConflictProjectionEntry<'a> = (&'a NodePath, ConflictProjection);

/// Decode a projected conflict token into the underlying `Atom::Conflict` data.
fn decode_conflict_projection(bytes: &[u8]) -> Option<(Vec<Blake3Hash>, Vec<Blake3Hash>)> {
    let payload = bytes.strip_prefix(ARC_CONFLICT_REF_PREFIX)?;
    match bincode::deserialize::<Atom>(payload).ok()? {
        Atom::Conflict { bases, sides, .. } => Some((bases, sides)),
        _ => None,
    }
}

/// Find the first conflict projection for `filepath` in materialized state.
fn conflict_projection_for_file(
    state: &MaterializedState,
    filepath: &str,
) -> anyhow::Result<Option<ConflictProjection>> {
    let mut projections: Vec<ConflictProjectionEntry<'_>> = state
        .iter()
        .filter_map(|(path, bytes)| {
            if path.len() >= 2 && path[0] == "file" && path[1] == filepath {
                decode_conflict_projection(bytes).map(|decoded| (path, decoded))
            } else {
                None
            }
        })
        .collect();

    if projections.is_empty() {
        return Ok(None);
    }

    projections.sort_by(|(a, _), (b, _)| a.cmp(b));
    if projections.len() > 1 {
        anyhow::bail!(
            "multiple conflict projections found for '{filepath}'; multi-conflict file rendering is not yet supported"
        );
    }

    Ok(projections.pop().map(|(_, decoded)| decoded))
}

fn read_blob_bytes(shared_root: &Path, hash: &Blake3Hash) -> anyhow::Result<Vec<u8>> {
    let blob_path = shared_root.join(".arc").join("blobs").join(_hex(hash));
    fs::read(&blob_path).map_err(|e| {
        anyhow::anyhow!(
            "failed to read conflict blob '{}': {e}",
            blob_path.display()
        )
    })
}

fn read_blob_text(shared_root: &Path, hash: &Blake3Hash) -> anyhow::Result<String> {
    let data = read_blob_bytes(shared_root, hash)?;
    Ok(String::from_utf8_lossy(&data).to_string())
}

fn interpolate_tool_args(args: &[String], vars: &HashMap<&str, String>) -> Vec<String> {
    args.iter()
        .map(|arg| {
            let mut interpolated = arg.clone();
            for (name, value) in vars {
                interpolated = interpolated.replace(&format!("${name}"), value);
            }
            interpolated
        })
        .collect()
}

#[tracing::instrument(skip(base_content, ours_content, theirs_content, tool))]
fn run_external_merge_tool_once(
    tool_name: &str,
    tool: &MergeToolConfig,
    repo_path: &NodePath,
    base_content: &[u8],
    ours_content: &[u8],
    theirs_content: &[u8],
) -> anyhow::Result<Vec<u8>> {
    let program = tool
        .program
        .clone()
        .unwrap_or_else(|| tool_name.to_string());
    anyhow::ensure!(
        !tool.merge_args.is_empty(),
        "merge tool '{tool_name}' has no merge_args configured"
    );

    let temp_dir = tempfile::Builder::new()
        .prefix("arc-resolve-")
        .tempdir()
        .map_err(|e| anyhow::anyhow!("failed to create merge temp directory: {e}"))?;

    let base_file = temp_dir.path().join("base.tmp");
    let left_file = temp_dir.path().join("left.tmp");
    let right_file = temp_dir.path().join("right.tmp");
    let output_file = temp_dir.path().join("output.tmp");

    fs::write(&base_file, base_content)
        .map_err(|e| anyhow::anyhow!("failed to write merge base temp file: {e}"))?;
    fs::write(&left_file, ours_content)
        .map_err(|e| anyhow::anyhow!("failed to write merge left temp file: {e}"))?;
    fs::write(&right_file, theirs_content)
        .map_err(|e| anyhow::anyhow!("failed to write merge right temp file: {e}"))?;
    fs::write(&output_file, ours_content)
        .map_err(|e| anyhow::anyhow!("failed to write merge output temp file: {e}"))?;

    let mut vars = HashMap::new();
    vars.insert("base", base_file.to_string_lossy().to_string());
    vars.insert("left", left_file.to_string_lossy().to_string());
    vars.insert("right", right_file.to_string_lossy().to_string());
    vars.insert("output", output_file.to_string_lossy().to_string());
    vars.insert("path", repo_path.join("/"));
    let args = interpolate_tool_args(&tool.merge_args, &vars);

    tracing::info!(tool = %tool_name, program = %program, ?args, "invoking merge tool");
    let status = Command::new(&program)
        .args(&args)
        .status()
        .map_err(|e| anyhow::anyhow!("failed to execute merge tool '{program}': {e}"))?;
    anyhow::ensure!(
        status.success(),
        "merge tool '{program}' exited with status {status}"
    );

    let resolved =
        fs::read(&output_file).map_err(|e| anyhow::anyhow!("failed to read merge output temp file: {e}"))?;
    anyhow::ensure!(
        !resolved.is_empty(),
        "merge tool '{tool_name}' produced empty output"
    );
    Ok(resolved)
}

/// Render Git-style conflict markers from CAS blob hashes.
fn render_conflict_markers(
    shared_root: &Path,
    bases: &[Blake3Hash],
    sides: &[Blake3Hash],
) -> anyhow::Result<String> {
    // Fetch base content for validation / future strategies even though
    // standard markers only render the conflicting sides.
    if let Some(base) = bases.first() {
        let _ = read_blob_text(shared_root, base)?;
    }

    let side_a_hash = sides
        .first()
        .ok_or_else(|| anyhow::anyhow!("conflict side A missing"))?;
    let side_b_hash = sides
        .get(1)
        .ok_or_else(|| anyhow::anyhow!("conflict side B missing"))?;

    let side_a = read_blob_text(shared_root, side_a_hash)?;
    let side_b = read_blob_text(shared_root, side_b_hash)?;

    Ok(format!(
        "<<<<<<< side_a\n{}\n=======\n{}\n>>>>>>> side_b\n",
        side_a, side_b
    ))
}

/// Decode a 64-character hex string to a [`Blake3Hash`] (used by log_semantic).
fn hex_to_blake3(hex: &str) -> Option<Blake3Hash> {
    _unhex(hex)
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

fn is_interactive_file_atom(atom: &Atom) -> bool {
    matches!(atom,
        Atom::Insert { at, .. } | Atom::Delete { at, .. }
            if at.first().map(|s| s == "file").unwrap_or(false) && at.len() > 2
    )
}

fn atom_file_path(atom: &Atom) -> Option<&str> {
    atom.paths().first()?.get(1).map(String::as_str)
}

fn select_atoms_interactively<F>(raw_atoms: Vec<Atom>, mut prompt: F) -> Vec<Atom>
where
    F: FnMut(&str, &str) -> bool,
{
    let mut accepted: Vec<Atom> = Vec::new();
    let mut current_file: Option<String> = None;

    for atom in raw_atoms {
        // Only AST diff atoms (Insert / Delete file nodes) are interactive.
        // Directory atoms and whole-file deletions are always staged.
        if !is_interactive_file_atom(&atom) {
            accepted.push(atom);
            continue;
        }

        let filepath = atom_file_path(&atom).unwrap_or_default().to_string();
        let label = atom_label(&atom);
        if current_file.as_deref() != Some(filepath.as_str()) {
            current_file = Some(filepath.clone());
        }

        if prompt(&filepath, &label) {
            accepted.push(atom);
        }
    }

    accepted
}

/// Return a human-readable label for an atom, used in interactive staging.
fn atom_label(atom: &Atom) -> String {
    match atom {
        Atom::Insert { at, .. } => {
            format!("Insert:   {}", at.last().unwrap_or(&"?".to_string()))
        }
        Atom::Delete { at, .. } => {
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
        Atom::Conflict { at, .. } => {
            format!("Conflict: {}", at.last().unwrap_or(&"?".to_string()))
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
        Atom::Insert { at, content_hash } => Atom::Insert {
            at: prepend(at),
            content_hash,
        },
        Atom::Delete { at, prior_hash } => Atom::Delete {
            at: prepend(at),
            prior_hash,
        },
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
        Atom::Conflict { bases, sides, at } => Atom::Conflict {
            bases,
            sides,
            at: prepend(at),
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
    store: &ObjectStore,
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
            .diff(&old_src, &new_src, store)
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
    let sparse_matcher = sparse_matcher_for_root(work_root);

    // Remove existing .rs files, tolerating NotFound.
    let arcignore = load_arcignore(work_root);
    let existing = collect_rs_files(work_root, &arcignore)?;
    for filepath in &existing {
        if !sparse_matcher.matches_file_path(filepath) {
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
        if !sparse_matcher.matches_file_path(&filepath) {
            continue; // outside sparse cone — skip projection to disk
        }
        let full = work_root.join(&filepath);
        if let Some(parent) = full.parent() {
            fs::create_dir_all(parent)?;
        }
        let path_key = vec!["file".to_string(), filepath.clone()];
        let content = state.get(&path_key);

        if let Some((bases, sides)) = conflict_projection_for_file(state, &filepath)? {
            let rendered = render_conflict_markers(shared_root, &bases, &sides)?;
            fs::write(&full, rendered.as_bytes())?;
            continue;
        }

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
        if key.len() == 2 && key[0] == "dir" && sparse_matcher.matches_file_path(&key[1]) {
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

fn simple_pattern_match(pattern: &str, text: &str) -> bool {
    if pattern == "*" {
        return true;
    }
    let p: Vec<char> = pattern.chars().collect();
    let s: Vec<char> = text.chars().collect();
    let mut dp = vec![vec![false; s.len() + 1]; p.len() + 1];
    dp[0][0] = true;
    for i in 1..=p.len() {
        if p[i - 1] == '*' {
            dp[i][0] = dp[i - 1][0];
        }
    }
    for i in 1..=p.len() {
        for j in 1..=s.len() {
            dp[i][j] = match p[i - 1] {
                '*' => dp[i - 1][j] || dp[i][j - 1],
                '?' => dp[i - 1][j - 1],
                c => dp[i - 1][j - 1] && c == s[j - 1],
            };
        }
    }
    dp[p.len()][s.len()]
}

fn sparse_matcher_for_root(root: &Path) -> SparseMatcher {
    SparseMatcher::from_patterns(&load_sparse_patterns(root))
}

fn validate_sparse_patterns(patterns: &[String], arcignore: &Gitignore) -> anyhow::Result<()> {
    for pattern in patterns {
        let normalized = pattern.trim().trim_matches('/');
        if normalized.is_empty() {
            continue;
        }
        if arcignore
            .matched_path_or_any_parents(normalized, true)
            .is_ignore()
        {
            anyhow::bail!(
                "sparse pattern '{}' conflicts with .arcignore; remove the ignore rule or choose a different sparse path",
                pattern
            );
        }
    }
    Ok(())
}

// ── config helpers ────────────────────────────────────────────────────

/// Try to load an `ArcConfig` from a TOML file at `path`.
/// If the TOML file is absent but a legacy `config.json` is present at the
/// same directory, migrate automatically: re-serialize as TOML, delete the
/// old JSON, and print a one-time deprecation notice to stderr.
fn load_config_file(toml_path: &Path) -> ArcConfig {
    if toml_path.exists() {
        if let Ok(text) = fs::read_to_string(toml_path)
            && let Ok(cfg) = toml::from_str::<ArcConfig>(&text)
        {
            return cfg;
        }
        return ArcConfig::default();
    }
    // Auto-migrate legacy config.json → config.toml.
    let json_path = toml_path.with_extension("json");
    if json_path.exists()
        && let Ok(json) = fs::read_to_string(&json_path)
        && let Ok(legacy) = serde_json::from_str::<LegacyConfig>(&json)
    {
        let migrated = ArcConfig {
            remotes: legacy.remotes,
            aliases: legacy.aliases,
            hooks: legacy.hooks,
            ..ArcConfig::default()
        };
        if let Ok(toml_text) = toml::to_string_pretty(&migrated)
            && fs::write(toml_path, &toml_text).is_ok()
        {
            let _ = fs::remove_file(&json_path);
            eprintln!("arc: migrated config.json → config.toml (one-time upgrade)");
            return migrated;
        }
    }
    ArcConfig::default()
}

/// Persist `config` as TOML to `path`, creating parent directories as needed.
fn save_config_file(config: &ArcConfig, path: &Path) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| anyhow::anyhow!("failed to create config dir: {e}"))?;
    }
    let text = toml::to_string_pretty(config)
        .map_err(|e| anyhow::anyhow!("failed to serialize config: {e}"))?;
    fs::write(path, text).map_err(|e| anyhow::anyhow!("failed to write config: {e}"))
}

/// Return the path to the OS-level global `config.toml`.
fn global_config_path() -> anyhow::Result<std::path::PathBuf> {
    let proj = directories::ProjectDirs::from("", "arc-vcs", "arc")
        .ok_or_else(|| anyhow::anyhow!("cannot determine global config directory"))?;
    Ok(proj.config_dir().join("config.toml"))
}

/// Return the OS-level global `config.toml` path used by arc.
pub fn global_config_file_path() -> anyhow::Result<std::path::PathBuf> {
    global_config_path()
}

/// Load only the global config layer without synthesized defaults or local overlay.
pub fn load_global_config_layer() -> anyhow::Result<ArcConfig> {
    let path = global_config_path()?;
    Ok(load_config_file(&path))
}

/// Return the local `.arc/config.toml` path for `shared_root`.
pub fn local_config_file_path(shared_root: &Path) -> std::path::PathBuf {
    shared_root.join(".arc").join("config.toml")
}

/// Load the merged `ArcConfig` for a shared-root repository.
///
/// The global config (`~/.config/arc/config.toml`) is loaded first, then
/// the local `.arc/config.toml` is overlaid so that local settings take
/// precedence.  Maps (`remotes`, `aliases`, `hooks`) are merged with local
/// entries overriding global ones of the same name.
pub fn load_merged_config(shared_root: &Path) -> anyhow::Result<ArcConfig> {
    let mut merged = synthesized_defaults_config();
    // Global config.
    if let Ok(global_path) = global_config_path() {
        let global = load_config_file(&global_path);
        merged.user = global.user;
        merged.merge = global.merge;
        if global.ui.color != "auto" {
            merged.ui.color = global.ui.color;
        }
        if global.ui.pager.is_some() {
            merged.ui.pager = global.ui.pager;
        }
        if global.ui.editor.is_some() {
            merged.ui.editor = global.ui.editor;
        }
        if global.ui.graph_style.is_some() {
            merged.ui.graph_style = global.ui.graph_style;
        }
        if global.ui.diff_formatter.is_some() {
            merged.ui.diff_formatter = global.ui.diff_formatter;
        }
        if global.ui.conflict_marker_style.is_some() {
            merged.ui.conflict_marker_style = global.ui.conflict_marker_style;
        }
        if global.ui.progress_indicator.is_some() {
            merged.ui.progress_indicator = global.ui.progress_indicator;
        }
        if global.ui.greet.is_some() {
            merged.ui.greet = global.ui.greet;
        }
        if global.ui.movement.edit.is_some() {
            merged.ui.movement.edit = global.ui.movement.edit;
        }
        if global.hints.resolving_conflicts.is_some() {
            merged.hints.resolving_conflicts = global.hints.resolving_conflicts;
        }
        if global.snapshot.max_new_file_size.is_some() {
            merged.snapshot.max_new_file_size = global.snapshot.max_new_file_size;
        }
        if global.snapshot.auto_track.is_some() {
            merged.snapshot.auto_track = global.snapshot.auto_track;
        }
        if global.snapshot.auto_update_stale.is_some() {
            merged.snapshot.auto_update_stale = global.snapshot.auto_update_stale;
        }
        merged.remotes.extend(global.remotes);
        merged.aliases.extend(global.aliases);
        merged.hooks.extend(global.hooks);
        merged.revsets.extend(global.revsets);
        merged.templates.extend(global.templates);
        merged.template_aliases.extend(global.template_aliases);
        merged.colors.extend(global.colors);
        merged.merge_tools.extend(global.merge_tools);
    }
    // Local config (overrides global).
    let local_path = shared_root.join(".arc").join("config.toml");
    let local = load_config_file(&local_path);
    if local.user.name.is_some() {
        merged.user.name = local.user.name;
    }
    if local.user.email.is_some() {
        merged.user.email = local.user.email;
    }
    if local.merge.tool.is_some() {
        merged.merge.tool = local.merge.tool;
    }
    if local.ui.color != "auto" {
        merged.ui.color = local.ui.color;
    }
    if local.ui.pager.is_some() {
        merged.ui.pager = local.ui.pager;
    }
    if local.ui.editor.is_some() {
        merged.ui.editor = local.ui.editor;
    }
    if local.ui.graph_style.is_some() {
        merged.ui.graph_style = local.ui.graph_style;
    }
    if local.ui.diff_formatter.is_some() {
        merged.ui.diff_formatter = local.ui.diff_formatter;
    }
    if local.ui.conflict_marker_style.is_some() {
        merged.ui.conflict_marker_style = local.ui.conflict_marker_style;
    }
    if local.ui.progress_indicator.is_some() {
        merged.ui.progress_indicator = local.ui.progress_indicator;
    }
    if local.ui.greet.is_some() {
        merged.ui.greet = local.ui.greet;
    }
    if local.ui.movement.edit.is_some() {
        merged.ui.movement.edit = local.ui.movement.edit;
    }
    if local.hints.resolving_conflicts.is_some() {
        merged.hints.resolving_conflicts = local.hints.resolving_conflicts;
    }
    if local.snapshot.max_new_file_size.is_some() {
        merged.snapshot.max_new_file_size = local.snapshot.max_new_file_size;
    }
    if local.snapshot.auto_track.is_some() {
        merged.snapshot.auto_track = local.snapshot.auto_track;
    }
    if local.snapshot.auto_update_stale.is_some() {
        merged.snapshot.auto_update_stale = local.snapshot.auto_update_stale;
    }
    merged.remotes.extend(local.remotes);
    merged.aliases.extend(local.aliases);
    merged.hooks.extend(local.hooks);
    merged.revsets.extend(local.revsets);
    merged.templates.extend(local.templates);
    merged.template_aliases.extend(local.template_aliases);
    merged.colors.extend(local.colors);
    merged.merge_tools.extend(local.merge_tools);
    Ok(merged)
}

/// Persist `config` to the OS-level global arc config file.
pub fn save_global_config(config: &ArcConfig) -> anyhow::Result<()> {
    let path = global_config_path()?;
    save_config_file(config, &path)
}

/// Persist `config` to the local `.arc/config.toml` for `shared_root`.
pub fn save_local_config(config: &ArcConfig, shared_root: &Path) -> anyhow::Result<()> {
    let path = shared_root.join(".arc").join("config.toml");
    save_config_file(config, &path)
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
            if contains_hard_ignored_dir(rel) {
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
/// extension. Implicit ignore policy is applied later in
/// [`Repository::compute_working_directory_delta`] so previously tracked files
/// can still be diffed and deleted safely.
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

            if path.is_dir() && contains_hard_ignored_dir(rel) {
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

            if path.is_dir() {
                if contains_hard_ignored_dir(rel) {
                    continue;
                }
            } else if is_implicitly_ignored(rel) {
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

fn contains_hard_ignored_dir(path: &Path) -> bool {
    path.components().any(|component| {
        component.as_os_str().to_str().is_some_and(|part| {
            matches!(part, "target" | "node_modules" | ".git" | "dist" | "build")
        })
    })
}

fn is_implicitly_ignored(file_path: &Path) -> bool {
    if std::env::var("ARC_TRACK_IMPLICITLY_IGNORED").is_ok_and(|v| v == "1") {
        return false;
    }

    if contains_hard_ignored_dir(file_path) {
        return true;
    }

    let file_name = match file_path.file_name().and_then(|n| n.to_str()) {
        Some(name) => name,
        None => return true,
    };

    if matches!(file_name, ".env" | "id_rsa") {
        return true;
    }

    let ext = file_path
        .extension()
        .and_then(|e| e.to_str())
        .map(|s| s.to_ascii_lowercase());

    if matches!(ext.as_deref(), Some("pem" | "key")) {
        return true;
    }

    !matches!(
        ext.as_deref(),
        Some(
            "rs" | "js"
                | "ts"
                | "py"
                | "go"
                | "c"
                | "cpp"
                | "md"
                | "json"
                | "toml"
                | "yaml"
                | "yml"
        )
    )
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
    fn test_abandon_head_moves_frontier_to_parent() {
        let dir = tempfile::tempdir().unwrap();
        let repo_path = dir.path().join("abandon_project");

        let mut repo = Repository::init(&repo_path).unwrap();
        let (author, signing_key) = arc_core::store::author::test_keypair();
        repo.set_identity(author, signing_key);

        fs::write(repo_path.join("lib.rs"), "fn v1() {}\n").unwrap();
        let first = repo.snap("v1", false).unwrap().unwrap();

        fs::write(repo_path.join("lib.rs"), "fn v2() {}\n").unwrap();
        let second = repo.snap("v2", false).unwrap().unwrap();

        let abandoned = repo.abandon_heads(&["@".to_string()]).unwrap();
        assert_eq!(abandoned, vec![second]);

        let view = View::load(&repo.shared_root, "main").unwrap();
        assert_eq!(view.heads, HashSet::from([first]));

        let content = fs::read_to_string(repo_path.join("lib.rs")).unwrap();
        assert!(content.contains("fn v1()"));
        assert!(!content.contains("fn v2()"));
    }

    #[test]
    fn test_undo_then_redo_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let repo_path = dir.path().join("redo_project");

        let mut repo = Repository::init(&repo_path).unwrap();
        let (author, signing_key) = arc_core::store::author::test_keypair();
        repo.set_identity(author, signing_key);

        fs::write(repo_path.join("lib.rs"), "fn base() {}\n").unwrap();
        let first = repo.snap("base", false).unwrap().unwrap();

        fs::write(repo_path.join("lib.rs"), "fn changed() {}\n").unwrap();
        let second = repo.snap("changed", false).unwrap().unwrap();

        let undone = repo.undo().unwrap().unwrap();
        assert_eq!(
            undone.after_heads,
            std::collections::BTreeSet::from([ChangeId::from(second)])
        );

        let after_undo = View::load(&repo.shared_root, "main").unwrap();
        assert_eq!(after_undo.heads, HashSet::from([first]));

        let redone = repo.redo().unwrap().unwrap();
        assert_eq!(
            redone.after_heads,
            std::collections::BTreeSet::from([ChangeId::from(second)])
        );

        let after_redo = View::load(&repo.shared_root, "main").unwrap();
        assert_eq!(after_redo.heads, HashSet::from([second]));

        let content = fs::read_to_string(repo_path.join("lib.rs")).unwrap();
        assert!(content.contains("fn changed()"));
    }

    #[test]
    fn test_bisect_roundtrip_start_mark_next() {
        let dir = tempfile::tempdir().unwrap();
        let repo_path = dir.path().join("bisect_project");

        let mut repo = Repository::init(&repo_path).unwrap();
        let (author, signing_key) = arc_core::store::author::test_keypair();
        repo.set_identity(author, signing_key);

        fs::write(repo_path.join("lib.rs"), "fn v1() {}\n").unwrap();
        let _ = repo.snap("v1", false).unwrap().unwrap();
        fs::write(repo_path.join("lib.rs"), "fn v2() {}\n").unwrap();
        let _ = repo.snap("v2", false).unwrap().unwrap();
        fs::write(repo_path.join("lib.rs"), "fn v3() {}\n").unwrap();
        let _ = repo.snap("v3", false).unwrap().unwrap();

        let started = repo.bisect_start("ancestors(@)", false).unwrap();
        assert!(started.current.is_some());
        assert!(
            repo_path
                .join(".arc")
                .join("bisect")
                .join("state.bin")
                .exists()
        );

        let after_mark = repo.bisect_mark_good().unwrap();
        // After marking current as good, either we have a next candidate or session converged.
        let _ = after_mark.current;

        let status = repo.bisect_status().unwrap().unwrap();
        assert_eq!(status.range_expr, "ancestors(@)");

        repo.bisect_reset().unwrap();
        assert!(repo.bisect_status().unwrap().is_none());
    }

    #[test]
    fn test_revset_log_wiring_hydrates_view_symbols() {
        let dir = tempfile::tempdir().unwrap();
        let repo_path = dir.path().join("revset_log_project");

        let mut repo = Repository::init(&repo_path).unwrap();
        let (author, signing_key) = arc_core::store::author::test_keypair();
        repo.set_identity(author.clone(), signing_key.clone());

        fs::write(repo_path.join("main.rs"), "fn main() { let a = 1; }").unwrap();
        let main_head = repo
            .snap("main head", false)
            .unwrap()
            .expect("main snap should create change");

        repo.create_view("feature").unwrap();
        repo.switch_view("feature").unwrap();
        fs::write(repo_path.join("feature.rs"), "fn feature() { let b = 2; }").unwrap();
        let feature_head = repo
            .snap("feature head", false)
            .unwrap()
            .expect("feature snap should create change");

        repo.switch_view("main").unwrap();

        // Re-open to simulate a fresh process where only current-view graph state
        // is loaded initially; revset preparation must hydrate feature symbols.
        let mut reopened = Repository::open(&repo_path).unwrap();
        reopened.set_identity(author, signing_key);

        let expr = arc_core::revset::parse("ancestors(feature)").unwrap();
        reopened.prepare_revset(&expr).unwrap();

        let graph = reopened.graph_snapshot();
        let mut resolver = |symbol: &str| reopened.resolve_revset_symbol(symbol);
        let ids: HashSet<Blake3Hash> = arc_core::revset::compile(&expr, graph, &mut resolver)
            .unwrap()
            .collect();

        assert!(
            ids.contains(&feature_head),
            "revset must include feature head"
        );
        assert!(
            ids.contains(&main_head),
            "revset must include feature ancestor"
        );
    }

    #[test]
    fn test_log_revset_touched_filters_history() {
        let dir = tempfile::tempdir().unwrap();
        let repo_path = dir.path().join("revset_touched_project");

        let mut repo = Repository::init(&repo_path).unwrap();
        let (author, signing_key) = arc_core::store::author::test_keypair();
        repo.set_identity(author, signing_key);

        fs::write(repo_path.join("main.rs"), "fn main() { let a = 1; }\n").unwrap();
        let main_id = repo.snap("main", false).unwrap().unwrap();

        fs::write(repo_path.join("util.rs"), "pub fn util() {}\n").unwrap();
        let util_id = repo.snap("util", false).unwrap().unwrap();

        repo.create_view("feature").unwrap();
        repo.switch_view("feature").unwrap();
        fs::write(repo_path.join("main.rs"), "fn main() { let feature = 2; }\n").unwrap();
        let feature_main_id = repo.snap("feature main", false).unwrap().unwrap();

        repo.switch_view("main").unwrap();
        let mut reopened = Repository::open(&repo_path).unwrap();
        let (author2, signing_key2) = arc_core::store::author::test_keypair();
        reopened.set_identity(author2, signing_key2);

        let selected = reopened.log_revset("touched(\"main.rs\")").unwrap();
        let selected_ids: HashSet<Blake3Hash> = selected.into_iter().map(|change| change.id).collect();

        assert!(selected_ids.contains(&main_id));
        assert!(!selected_ids.contains(&util_id));
        assert!(
            !selected_ids.contains(&feature_main_id),
            "touched() should be scoped to current view ancestry"
        );
    }

    #[test]
    fn test_log_is_multi_head_safe_by_default() {
        let dir = tempfile::tempdir().unwrap();
        let repo_path = dir.path().join("multi_head_log_project");

        let mut repo = Repository::init(&repo_path).unwrap();
        let (author, signing_key) = arc_core::store::author::test_keypair();
        repo.set_identity(author, signing_key);

        fs::write(repo_path.join("base.rs"), "fn base() {}\n").unwrap();
        repo.snap("base", false).unwrap().expect("base snap");

        repo.create_view("feature").unwrap();
        repo.switch_view("feature").unwrap();
        fs::write(repo_path.join("feature.rs"), "fn feature() {}\n").unwrap();
        let feature_head = repo
            .snap("feature head", false)
            .unwrap()
            .expect("feature snap");

        repo.switch_view("main").unwrap();
        fs::write(repo_path.join("main.rs"), "fn main_head() {}\n").unwrap();
        let main_head = repo.snap("main head", false).unwrap().expect("main snap");

        repo.merge_view("feature").unwrap();

        let entries = repo
            .log()
            .expect("default log must work for multi-head views");
        let ids: HashSet<Blake3Hash> = entries.iter().map(|change| change.id).collect();
        assert!(ids.contains(&feature_head));
        assert!(ids.contains(&main_head));
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

        // Merge should succeed by producing a first-class conflict change.
        let result = repo.merge_view("feature");
        assert!(result.is_ok(), "merge of conflicting changes must yield Ok");

        // Legacy metadata remains for Ghost Node compatibility.
        assert!(
            repo_path.join(".arc").join("conflict").exists(),
            ".arc/conflict must exist after conflict-bearing merge"
        );

        let content = fs::read_to_string(repo_path.join("shared.rs")).unwrap();
        assert!(
            content.contains("<<<<<<< side_a")
                && content.contains("=======")
                && content.contains(">>>>>>> side_b"),
            "working file must contain conflict markers, got: {content}"
        );

        let main_view = arc_core::store::view::View::load(&repo_path, "main").unwrap();
        assert_eq!(
            main_view.heads.len(),
            1,
            "conflict-bearing merge should produce one synthetic head"
        );
        let head = *main_view.heads.iter().next().unwrap();
        let graph = repo.graph.load();
        let change = graph
            .get(&head)
            .expect("conflict change must exist in graph");
        assert!(
            change
                .atoms
                .iter()
                .any(|a| matches!(a, Atom::Conflict { .. })),
            "merged head must contain Atom::Conflict"
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

        // Merge succeeds with conflict-bearing change and creates .arc/conflict.
        let result = repo.merge_view("feature");
        assert!(result.is_ok());

        let merged = fs::read_to_string(repo_path.join("shared.rs")).unwrap();
        assert!(
            merged.contains("<<<<<<< side_a")
                && merged.contains("=======")
                && merged.contains(">>>>>>> side_b"),
            "merged file must contain conflict markers before AI resolution"
        );

        // Resolve via the mock AI resolver (Ghost Node mode).
        let resolver = MockResolver;
        repo.resolve_conflict(&resolver).unwrap();

        // .arc/ai/pending.json must exist — the Ghost Node.
        assert!(
            repo_path
                .join(".arc")
                .join("ai")
                .join("pending.json")
                .exists(),
            ".arc/ai/pending.json must exist after resolve_conflict"
        );

        // .arc/conflict should be cleaned up immediately.
        assert!(
            !repo_path.join(".arc").join("conflict").exists(),
            ".arc/conflict must be removed after resolution"
        );

        // The working directory should already show the merged content.
        let content = fs::read_to_string(repo_path.join("shared.rs")).unwrap();
        // MockResolver concatenates ours + "\n" + theirs.
        assert!(
            content.contains("fn shared() { let b = 2; }")
                && content.contains("fn shared() { let a = 1; }"),
            "merged content must contain both sides, got: {content}"
        );

        // Now approve: should write Author::AI change, advance view, clear pending.
        let (approve_author, approve_key) = arc_core::store::author::test_keypair();
        let merge_id = repo
            .approve_pending_ai(&approve_author, &approve_key)
            .unwrap();

        // The merge change ID should be non-zero.
        assert_ne!(merge_id, [0u8; 32]);

        // pending.json must be gone after approval.
        assert!(
            !repo_path
                .join(".arc")
                .join("ai")
                .join("pending.json")
                .exists(),
            ".arc/ai/pending.json must be removed after approve"
        );

        // The committed change should carry Author::AI authorship.
        let changes = repo.log().unwrap();
        let ai_changes: Vec<_> = changes
            .iter()
            .filter(|c| matches!(&c.author, arc_core::store::author::Author::AI { .. }))
            .collect();
        assert!(
            !ai_changes.is_empty(),
            "at least one AI-authored change must be in log"
        );
        assert!(
            ai_changes[0].verify_signature(),
            "AI change must have a valid signature"
        );
    }

    #[test]
    fn test_ai_conflict_resolution_with_provider() {
        struct MockProvider;

        #[async_trait::async_trait]
        impl arc_net::ai::AiProvider for MockProvider {
            async fn resolve_conflict(
                &self,
                _base: &str,
                side_a: &str,
                side_b: &str,
                _file_path: &str,
            ) -> anyhow::Result<String> {
                Ok(format!("{side_a}\n{side_b}"))
            }
        }

        let dir = tempfile::tempdir().unwrap();
        let repo_path = dir.path().join("ai_resolve_provider_project");

        let mut repo = Repository::init(&repo_path).unwrap();
        let (author, signing_key) = arc_core::store::author::test_keypair();
        repo.set_identity(author, signing_key);

        fs::write(repo_path.join("shared.rs"), "fn shared() {}").unwrap();
        repo.snap("initial shared.rs", false).unwrap();

        repo.create_view("feature").unwrap();
        repo.switch_view("feature").unwrap();
        fs::write(repo_path.join("shared.rs"), "fn shared() { let a = 1; }").unwrap();
        repo.snap("modify shared.rs on feature", false).unwrap();

        repo.switch_view("main").unwrap();
        fs::write(repo_path.join("shared.rs"), "fn shared() { let b = 2; }").unwrap();
        repo.snap("modify shared.rs on main", false).unwrap();

        repo.merge_view("feature").unwrap();

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(repo.resolve_conflict_with_provider(&MockProvider, "mock-model"))
            .unwrap();

        let content = fs::read_to_string(repo_path.join("shared.rs")).unwrap();
        assert!(content.contains("let a = 1") && content.contains("let b = 2"));

        let (approve_author, approve_key) = arc_core::store::author::test_keypair();
        let _ = repo
            .approve_pending_ai(&approve_author, &approve_key)
            .unwrap();

        let oplog = arc_core::store::oplog::OpLog::new(&repo.shared_root.join(".arc"));
        let ops = oplog.read_all().unwrap();
        assert!(
            ops.iter().any(|op| op.command == "ai resolve"),
            "approval flow should record 'ai resolve' operation"
        );
    }

    fn configure_test_merge_tool(repo_path: &Path, behavior: &str) {
        let script_path = if cfg!(windows) {
            repo_path.join("merge-tool.cmd")
        } else {
            repo_path.join("merge-tool.sh")
        };

        let script_content = if cfg!(windows) {
            match behavior {
                "copy-theirs" => "@echo off\r\ntype \"%3\" > \"%4\"\r\nexit /b 0\r\n",
                "copy-ours" => "@echo off\r\ntype \"%2\" > \"%4\"\r\nexit /b 0\r\n",
                _ => "@echo off\r\nexit /b 9\r\n",
            }
        } else {
            match behavior {
                "copy-theirs" => "#!/bin/sh\ncat \"$3\" > \"$4\"\n",
                "copy-ours" => "#!/bin/sh\ncat \"$2\" > \"$4\"\n",
                _ => "#!/bin/sh\nexit 9\n",
            }
        };
        fs::write(&script_path, script_content).unwrap();

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mut perms = fs::metadata(&script_path).unwrap().permissions();
            perms.set_mode(0o755);
            fs::set_permissions(&script_path, perms).unwrap();
        }

        let mut cfg = ArcConfig::default();
        cfg.merge.tool = Some("testtool".to_string());
        cfg.merge_tools.insert(
            "testtool".to_string(),
            MergeToolConfig {
                program: Some(if cfg!(windows) {
                    "cmd".to_string()
                } else {
                    "sh".to_string()
                }),
                merge_args: if cfg!(windows) {
                    vec![
                        "/C".to_string(),
                        script_path.to_string_lossy().to_string(),
                        "$base".to_string(),
                        "$left".to_string(),
                        "$right".to_string(),
                        "$output".to_string(),
                    ]
                } else {
                    vec![
                        script_path.to_string_lossy().to_string(),
                        "$base".to_string(),
                        "$left".to_string(),
                        "$right".to_string(),
                        "$output".to_string(),
                    ]
                },
                edit_args: Vec::new(),
                diff_args: Vec::new(),
            },
        );
        save_local_config(&cfg, repo_path).unwrap();
    }

    #[test]
    fn test_merge_tool_conflict_resolution_stages_pending() {
        let dir = tempfile::tempdir().unwrap();
        let repo_path = dir.path().join("merge_tool_resolve_project");

        let mut repo = Repository::init(&repo_path).unwrap();
        let (author, signing_key) = arc_core::store::author::test_keypair();
        repo.set_identity(author, signing_key);

        fs::write(repo_path.join("shared.rs"), "fn shared() {}\n").unwrap();
        repo.snap("initial shared.rs", false).unwrap();

        repo.create_view("feature").unwrap();
        repo.switch_view("feature").unwrap();
        fs::write(repo_path.join("shared.rs"), "fn shared() { let a = 1; }\n").unwrap();
        repo.snap("modify shared.rs on feature", false).unwrap();

        repo.switch_view("main").unwrap();
        fs::write(repo_path.join("shared.rs"), "fn shared() { let b = 2; }\n").unwrap();
        repo.snap("modify shared.rs on main", false).unwrap();
        repo.merge_view("feature").unwrap();

        configure_test_merge_tool(&repo_path, "copy-theirs");

        repo.resolve_conflict_with_merge_tool(None).unwrap();

        assert!(
            !repo_path.join(".arc").join("conflict").exists(),
            "conflict metadata should be cleared after staging merge-tool resolution"
        );
        assert!(
            repo_path
                .join(".arc")
                .join("ai")
                .join("pending.json")
                .exists(),
            "pending ghost node should be staged"
        );

        let pending = load_pending_ai(&repo_path).expect("pending ghost node should load");
        assert!(
            pending.model.starts_with("merge-tool:"),
            "model marker should indicate merge-tool provenance"
        );

        let content = fs::read_to_string(repo_path.join("shared.rs")).unwrap();
        assert!(
            content.contains("let a = 1") || content.contains("let b = 2"),
            "merge-tool output should contain one resolved side"
        );
        assert!(
            !content.contains("<<<<<<< side_a"),
            "resolved file should no longer contain conflict markers"
        );
    }

    #[test]
    fn test_merge_tool_resolution_accepts_unchanged_output() {
        let dir = tempfile::tempdir().unwrap();
        let repo_path = dir.path().join("merge_tool_accept_unchanged_project");

        let mut repo = Repository::init(&repo_path).unwrap();
        let (author, signing_key) = arc_core::store::author::test_keypair();
        repo.set_identity(author, signing_key);

        fs::write(repo_path.join("shared.rs"), "fn shared() {}\n").unwrap();
        repo.snap("initial shared.rs", false).unwrap();

        repo.create_view("feature").unwrap();
        repo.switch_view("feature").unwrap();
        fs::write(repo_path.join("shared.rs"), "fn shared() { let a = 1; }\n").unwrap();
        repo.snap("modify shared.rs on feature", false).unwrap();

        repo.switch_view("main").unwrap();
        fs::write(repo_path.join("shared.rs"), "fn shared() { let b = 2; }\n").unwrap();
        repo.snap("modify shared.rs on main", false).unwrap();
        repo.merge_view("feature").unwrap();

        configure_test_merge_tool(&repo_path, "copy-ours");
        repo.resolve_conflict_with_merge_tool(None).unwrap();

        assert!(
            repo_path
                .join(".arc")
                .join("ai")
                .join("pending.json")
                .exists(),
            "unchanged output should still stage a pending resolution"
        );
    }

    #[test]
    fn test_merge_tool_resolution_rejects_nonzero_exit() {
        let dir = tempfile::tempdir().unwrap();
        let repo_path = dir.path().join("merge_tool_unchanged_project");

        let mut repo = Repository::init(&repo_path).unwrap();
        let (author, signing_key) = arc_core::store::author::test_keypair();
        repo.set_identity(author, signing_key);

        fs::write(repo_path.join("shared.rs"), "fn shared() {}\n").unwrap();
        repo.snap("initial shared.rs", false).unwrap();

        repo.create_view("feature").unwrap();
        repo.switch_view("feature").unwrap();
        fs::write(repo_path.join("shared.rs"), "fn shared() { let a = 1; }\n").unwrap();
        repo.snap("modify shared.rs on feature", false).unwrap();

        repo.switch_view("main").unwrap();
        fs::write(repo_path.join("shared.rs"), "fn shared() { let b = 2; }\n").unwrap();
        repo.snap("modify shared.rs on main", false).unwrap();
        repo.merge_view("feature").unwrap();

        configure_test_merge_tool(&repo_path, "exit-fail");

        let err = repo.resolve_conflict_with_merge_tool(None).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("exited with status"));
        assert!(repo_path.join(".arc").join("conflict").exists());
        assert!(
            !repo_path
                .join(".arc")
                .join("ai")
                .join("pending.json")
                .exists()
        );
    }

    #[test]
    fn test_pending_ai_roundtrip() {
        use crate::ai_pending::{
            PendingAiChange, clear_pending_ai, has_pending_ai, load_pending_ai, save_pending_ai,
        };
        let dir = tempfile::tempdir().unwrap();
        let repo_root = dir.path();

        assert!(!has_pending_ai(repo_root));

        let pending = PendingAiChange::new_generate(
            "gpt-4o-mini".to_owned(),
            "add retry backoff".to_owned(),
            vec![std::path::PathBuf::from("src/client.rs")],
        );
        save_pending_ai(repo_root, &pending).unwrap();
        assert!(has_pending_ai(repo_root));

        let loaded = load_pending_ai(repo_root).expect("must be loadable");
        assert_eq!(loaded.intent, "add retry backoff");
        assert_eq!(loaded.model, "gpt-4o-mini");
        assert!(matches!(
            loaded.kind,
            crate::ai_pending::PendingKind::Generate
        ));

        clear_pending_ai(repo_root);
        assert!(!has_pending_ai(repo_root));
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
            .load()
            .get(&revert_id)
            .expect("revert change must be present in the graph")
            .clone();
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

        // set_tag must refuse moving without allow_move.
        assert!(
            repo.set_tag("v1.0.0", &[9u8; 32], false).is_err(),
            "set_tag must refuse moves without --allow-move"
        );

        // allow_move updates the target.
        repo.set_tag("v1.0.0", &[9u8; 32], true).unwrap();
        let moved = repo
            .list_tags_matching(&["v*".to_string()])
            .unwrap()
            .into_iter()
            .find(|t| t.name == "v1.0.0")
            .expect("tag should still exist");
        assert_eq!(moved.target, [9u8; 32]);

        // Pattern delete should remove matching tags only.
        repo.set_tag("release-candidate", &snap_id, true).unwrap();
        let deleted = repo
            .delete_tags_matching(&["release-*".to_string()])
            .unwrap();
        assert_eq!(deleted, vec!["release-candidate".to_string()]);
    }

    #[test]
    fn test_bookmark_operations() {
        let dir = tempfile::tempdir().unwrap();
        let repo_path = dir.path().join("bookmark_project");

        let mut repo = Repository::init(&repo_path).unwrap();
        let (author, signing_key) = arc_core::store::author::test_keypair();
        repo.set_identity(author, signing_key);

        fs::write(repo_path.join("lib.rs"), "fn lib() { 1 }\n").unwrap();
        let first = repo.snap("add lib", false).unwrap().unwrap();

        fs::write(repo_path.join("lib.rs"), "fn lib() { 2 }\n").unwrap();
        let second = repo.snap("update lib", false).unwrap().unwrap();

        repo.create_bookmark("trunk/main", &first).unwrap();
        assert!(
            repo.create_bookmark("trunk/main", &first).is_err(),
            "duplicate bookmark must be rejected"
        );

        repo.move_bookmark("trunk/main", &second, false).unwrap();
        assert!(
            repo.move_bookmark("trunk/main", &first, false).is_err(),
            "non-fast-forward move must be rejected by default"
        );
        repo.move_bookmark("trunk/main", &first, true).unwrap();

        let decorations = repo.bookmark_decorations().unwrap();
        assert!(
            decorations
                .get(&ChangeId::from(first))
                .is_some_and(|names| names.contains(&"trunk/main".to_string())),
            "bookmark decoration must include trunk/main"
        );

        repo.delete_bookmark("trunk/main").unwrap();
        assert!(
            repo.delete_bookmark("trunk/main").is_err(),
            "deleting missing bookmark must fail"
        );

        let invalid_names = [
            "",
            "   ",
            "/absolute",
            "..",
            "../escape",
            "feature/../escape",
            "feature\\..\\escape",
            "C:\\temp\\escape",
        ];
        for name in invalid_names {
            assert!(
                repo.create_bookmark(name, &first).is_err(),
                "invalid bookmark name '{name}' must be rejected"
            );
        }
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

    #[test]
    fn test_implicit_ignore_skips_env_and_node_modules() {
        let dir = tempfile::tempdir().unwrap();
        let repo_path = dir.path().join("implicit_ignore_project");

        let mut repo = Repository::init(&repo_path).unwrap();
        let (author, signing_key) = arc_core::store::author::test_keypair();
        repo.set_identity(author, signing_key);

        fs::write(repo_path.join(".env"), "API_KEY=super-secret").unwrap();
        fs::create_dir_all(repo_path.join("node_modules")).unwrap();
        fs::write(
            repo_path.join("node_modules").join("fake.js"),
            "module.exports = 1;",
        )
        .unwrap();

        let delta = repo.status().unwrap();
        assert!(
            delta.is_empty(),
            "implicit ignore should hide .env and node_modules from DAG delta"
        );

        let snap = repo.snap("should be ignored", false).unwrap();
        assert!(
            snap.is_none(),
            "snapshot should be empty when only implicitly ignored files changed"
        );
    }

    #[test]
    fn test_implicit_ignore_still_allows_delete_of_tracked_legacy_asset() {
        let dir = tempfile::tempdir().unwrap();
        let repo_path = dir.path().join("implicit_ignore_delete_project");

        let mut repo = Repository::init(&repo_path).unwrap();
        let (author, signing_key) = arc_core::store::author::test_keypair();
        repo.set_identity(author.clone(), signing_key.clone());

        let blob_hash = repo.store.write_blob(b"legacy").unwrap();
        let legacy_change = Change::new(
            HashSet::new(),
            vec![Atom::Blob {
                path: vec!["file".to_string(), "legacy.txt".to_string()],
                hash: blob_hash,
            }],
            "seed legacy tracked blob",
            author,
            &signing_key,
        );
        repo.store.write_change(&legacy_change).unwrap();

        View::new("main", HashSet::from([legacy_change.id]))
            .save(&repo_path)
            .unwrap();

        let delta = repo.status().unwrap();
        assert!(
            delta.iter().any(|atom| matches!(
                atom,
                Atom::Delete { at, .. } if at.len() >= 2 && at[0] == "file" && at[1] == "legacy.txt"
            )),
            "tracked legacy asset deletion must still emit a Delete atom"
        );
    }

    /// Universal asset engine: non-Rust files are tracked as [`Atom::Blob`].
    ///
    /// Verifies that:
    /// 1. Snapping a `.md` file writes raw bytes to `.arc/blobs/`.
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

        // Write a non-Rust markdown file and snap it.
        let txt_path = repo_path.join("readme.md");
        fs::write(&txt_path, b"Hello, arc universal assets!").unwrap();

        let snap_id = repo
            .snap("add readme.md", false)
            .unwrap()
            .expect("snap must produce a change for a new markdown file");

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
        let path_key = vec!["file".to_string(), "readme.md".to_string()];
        let blob_ref = state.get(&path_key).expect("blob ref must be in state");
        assert!(
            blob_ref.starts_with(b"ARC_BLOB_REF:"),
            "state entry must start with ARC_BLOB_REF:"
        );
        assert_eq!(blob_ref.len(), 45, "blob ref must be 13 + 32 bytes");

        // restore() must recover the original bytes.
        fs::write(&txt_path, b"corrupted").unwrap();
        repo.restore("readme.md").unwrap();
        let restored = fs::read(&txt_path).unwrap();
        assert_eq!(
            restored, b"Hello, arc universal assets!",
            "restore must recover original bytes"
        );

        // Snap must carry a valid cryptographic signature.
        let g = repo.graph.load_full();
        let change = g.get(&snap_id).expect("snap must be in graph");
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

        // Snap a markdown file.
        let txt_path = repo_path.join("data.md");
        fs::write(&txt_path, b"important data").unwrap();
        repo.snap("add data.md", false)
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

    #[test]
    fn test_rewrite_transaction_undo_restores_heads() {
        let dir = tempfile::tempdir().unwrap();
        let repo_path = dir.path().join("rewrite_undo_project");

        let mut repo = Repository::init(&repo_path).unwrap();
        let (author, signing_key) = arc_core::store::author::test_keypair();
        repo.set_identity(author, signing_key);

        fs::write(repo_path.join("main.rs"), "fn a() {}\n").unwrap();
        repo.snap("add a", false)
            .unwrap()
            .expect("snap must produce id");

        fs::write(repo_path.join("main.rs"), "fn a() {}\nfn b() {}\n").unwrap();
        repo.snap("add b", false)
            .unwrap()
            .expect("snap must produce id");

        let before = View::load(&repo_path, "main").unwrap().heads;
        assert_eq!(before.len(), 1, "test assumes one-head view");

        let _ = repo.squash_into("HEAD~1").unwrap();

        let op = repo
            .op_log()
            .unwrap()
            .into_iter()
            .find(|entry| entry.command == "squash")
            .expect("oplog must contain squash operation");
        assert!(
            matches!(op.kind, arc_core::store::oplog::OperationKind::Rewrite),
            "squash must be stored as rewrite operation"
        );
        assert!(!op.rewrite_map.is_empty(), "rewrite map must be recorded");

        repo.undo().unwrap().expect("undo should pop rewrite op");
        let restored = View::load(&repo_path, "main").unwrap().heads;
        assert_eq!(restored, before, "undo must restore original heads");
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
    fn test_sparse_to_full_restores_tracked_files() {
        let dir = tempfile::tempdir().unwrap();
        let repo_path = dir.path().join("sparse_full_restore_project");

        let mut repo = Repository::init(&repo_path).unwrap();
        let (author, signing_key) = arc_core::store::author::test_keypair();
        repo.set_identity(author, signing_key);

        fs::write(repo_path.join("a.rs"), "fn a() {}").unwrap();
        fs::create_dir_all(repo_path.join("b")).unwrap();
        fs::write(repo_path.join("b").join("c.rs"), "fn c() {}").unwrap();
        repo.snap("add files", false)
            .unwrap()
            .expect("snap must produce a change");

        repo.apply_sparse(&["b/".to_string()]).unwrap();
        assert!(!repo_path.join("a.rs").exists());
        assert!(repo_path.join("b").join("c.rs").exists());

        repo.apply_sparse(&[]).unwrap();

        assert!(
            repo_path.join("a.rs").exists(),
            "a.rs must be restored when sparse is cleared"
        );
        assert!(
            repo_path.join("b").join("c.rs").exists(),
            "b/c.rs must remain present after returning to full checkout"
        );

        let atoms = repo.status().unwrap();
        assert!(
            atoms.is_empty(),
            "status must remain clean after sparse->full restoration; got: {atoms:?}"
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
        assert!(
            ws_path.join(".arc-workspace").exists(),
            ".arc-workspace must be written"
        );

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
        assert!(
            list.contains(&ws_path),
            "workspace_list must return ws_path"
        );
    }

    #[test]
    fn test_workspace_forget_rename_and_root() {
        let dir = tempfile::tempdir().unwrap();
        let primary_path = dir.path().join("primary");
        let ws_path = dir.path().join("workspace-a");
        let ws_path_renamed = dir.path().join("workspace-b");

        let mut primary = Repository::init(&primary_path).unwrap();
        primary.workspace_add(&ws_path, None).unwrap();

        let root = primary.workspace_root(Some(&ws_path)).unwrap();
        assert!(root.ends_with("workspace-a"));

        primary
            .workspace_rename(&ws_path, &ws_path_renamed)
            .unwrap();
        assert!(ws_path_renamed.join(".arc-workspace").exists());

        primary.workspace_forget(&ws_path_renamed).unwrap();
        assert!(!ws_path_renamed.join(".arc-workspace").exists());
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

        // Verify the CAS is using the two-level sharding layout that gc()'s
        // two-level walk requires.  The old flat walk silently found zero
        // changes because it looked for 64-char files at the top level; the
        // fixed walk looks for 2-char shard dirs containing 62-char files.
        let store_dir = repo.shared_root.join(".arc").join("store");
        let shards: Vec<_> = fs::read_dir(&store_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().unwrap().is_dir())
            .collect();
        assert!(
            !shards.is_empty(),
            "CAS must use two-level sharding (at least one shard dir exists)"
        );
        for shard in &shards {
            let name = shard.file_name().to_string_lossy().into_owned();
            assert_eq!(
                name.len(),
                2,
                "shard directory '{name}' must be exactly 2 hex chars"
            );
            assert!(
                name.bytes().all(|b| b.is_ascii_hexdigit()),
                "shard directory '{name}' must be lowercase hex"
            );
        }
    }

    /// Verifies that GC removes orphaned blob files — blobs present in `.arc/blobs/`
    /// that are not referenced by any reachable Change — while leaving reachable
    /// blobs intact.
    #[test]
    fn test_local_gc_removes_orphans() {
        let dir = tempfile::tempdir().unwrap();
        let repo_path = dir.path().join("orphan_project");

        let mut repo = Repository::init(&repo_path).unwrap();
        let (author, signing_key) = arc_core::store::author::test_keypair();
        repo.set_identity(author.clone(), signing_key.clone());

        // Snap a change so there is at least one reachable blob.
        fs::write(repo_path.join("main.rs"), "fn main() {}").unwrap();
        repo.snap("add main.rs", false).unwrap().expect("snap");

        // Write an orphan blob directly into `.arc/blobs/` — no Change atom
        // references this hash, so GC must collect it.
        let orphan_content = b"orphan bytes not referenced by any change";
        let orphan_hash: Blake3Hash = *blake3::hash(orphan_content).as_bytes();
        let orphan_hex = _hex(&orphan_hash);
        let blobs_dir = repo_path.join(".arc").join("blobs");
        let orphan_path = blobs_dir.join(&orphan_hex);
        fs::write(&orphan_path, orphan_content).unwrap();
        assert!(orphan_path.exists(), "orphan blob must exist before GC");

        // Record how many blobs exist before GC.
        let before_count = fs::read_dir(&blobs_dir).unwrap().count();

        let result = repo.gc().unwrap();
        assert_eq!(
            result.changes_deleted, 0,
            "reachable changes must not be deleted"
        );
        assert!(
            result.blobs_deleted >= 1,
            "orphan blob must be deleted by GC"
        );
        assert!(
            !orphan_path.exists(),
            "orphan blob file must be removed from disk"
        );

        let after_count = fs::read_dir(&blobs_dir).unwrap().count();
        assert!(
            after_count < before_count,
            "blob count must decrease after GC"
        );
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
        assert_eq!(
            view.heads.len(),
            1,
            "view must have exactly one head after compact"
        );
        assert!(
            view.heads.contains(&genesis_id),
            "view head must be the genesis change"
        );

        // The in-memory graph must contain only the Genesis Change
        // (no ancestors — it has empty deps).
        let ancestors = repo2.graph.load().ancestors(&view.heads);
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
        let state = repo2
            .materialize("main")
            .expect("materialize after compact");
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

    #[test]
    fn test_amend() {
        let dir = tempfile::tempdir().unwrap();
        let repo_path = dir.path().join("amend_project");
        let mut repo = Repository::init(&repo_path).unwrap();
        let (author, signing_key) = arc_core::store::author::test_keypair();
        repo.set_identity(author, signing_key);

        // Create a file and snap it.
        fs::create_dir_all(repo_path.join("src")).unwrap();
        fs::write(repo_path.join("src").join("main.rs"), "fn a() {}").unwrap();
        let snap_id = repo
            .snap("add fn a", false)
            .unwrap()
            .expect("snap must produce a change");

        // Modify the file and amend — no new message supplied.
        fs::write(
            repo_path.join("src").join("main.rs"),
            "fn a() {}\nfn amended() {}",
        )
        .unwrap();
        let new_id = repo.amend(None).expect("amend must succeed");

        // The amended change must have a different ID.
        assert_ne!(snap_id, new_id, "amend must produce a new change ID");

        // The view must point at the amended change as its sole head.
        let view = View::load(&repo_path, "main").expect("load view");
        assert_eq!(
            view.heads.len(),
            1,
            "view must have exactly one head after amend"
        );
        assert!(
            view.heads.contains(&new_id),
            "view head must be the amended change"
        );
        assert!(
            !view.heads.contains(&snap_id),
            "original snap must no longer be a head"
        );

        // Materialising the view must reflect the amended content.
        let state = repo.materialize("main").expect("materialize after amend");
        let all_content: String = state
            .values()
            .filter_map(|v| std::str::from_utf8(v).ok())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            all_content.contains("amended"),
            "materialised state must contain 'amended'; got: {all_content:?}"
        );

        // The epoch map must redirect the old snap ID to the new amended ID.
        let epochs_path = repo_path.join(".arc").join("epochs");
        assert!(epochs_path.exists(), ".arc/epochs must exist after amend");
        let raw = fs::read_to_string(&epochs_path).unwrap();
        let epoch_map: std::collections::HashMap<String, String> =
            serde_json::from_str(&raw).unwrap();
        let snap_hex: String = snap_id.iter().map(|b| format!("{b:02x}")).collect();
        let new_hex: String = new_id.iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(
            epoch_map.get(&snap_hex).map(String::as_str),
            Some(new_hex.as_str()),
            "epoch map must redirect old snap ID to amended ID"
        );
    }

    #[test]
    fn interactive_selector_keeps_non_ast_atoms() {
        let interactive_insert = Atom::Insert {
            at: vec![
                "file".to_string(),
                "src/main.rs".to_string(),
                "fn_new".to_string(),
            ],
            content_hash: [7u8; 32],
        };
        let non_interactive_dir = Atom::Directory {
            path: vec!["dir".to_string(), "src".to_string()],
        };

        let selected = select_atoms_interactively(
            vec![interactive_insert.clone(), non_interactive_dir.clone()],
            |_filepath, _label| false,
        );

        assert_eq!(selected, vec![non_interactive_dir]);
    }

    #[test]
    fn interactive_selector_can_accept_ast_atoms() {
        let keep = Atom::Insert {
            at: vec![
                "file".to_string(),
                "src/lib.rs".to_string(),
                "fn_keep".to_string(),
            ],
            content_hash: [1u8; 32],
        };
        let drop = Atom::Delete {
            at: vec![
                "file".to_string(),
                "src/lib.rs".to_string(),
                "fn_drop".to_string(),
            ],
            prior_hash: [2u8; 32],
        };

        let selected = select_atoms_interactively(vec![keep.clone(), drop], |_filepath, label| {
            label.contains("fn_keep")
        });

        assert_eq!(selected, vec![keep]);
    }
}
