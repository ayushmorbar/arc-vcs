use std::{
    collections::{BTreeSet, HashSet},
    io::IsTerminal,
    path::PathBuf,
    sync::OnceLock,
};

use anyhow::Context as _;
use arc_algebra_types::{Blake3Hash, SpacetimeCoordinate};
use arc_cli::{
    governance::audit_github_governance,
    graph_render::{GraphDecorations, GraphRenderer, LogTemplate},
    interop::git::import_repo,
    ops::{OperationStage, SloTimer},
    progress::{PipelineStage, Progress},
    repo::{
        ArcConfig, Repository, global_config_file_path, load_global_config_layer,
        load_merged_config, local_config_file_path, save_global_config, save_local_config,
    },
    sync::{fetch, pull},
    tooling::audit_workspace_tooling,
    workspace_policy::audit_workspace_policy,
};
use arc_diagnostics::{ResultExt, init_tracing};
use arc_net::ai::build_provider;
use arc_store_policy::{ArcIgnoreMatcher, PathPolicyDecision, explain_config_key};
use arc_store_types::{
    author::{Author, load_identity, save_identity},
    newtypes::{ChangeId, SnapshotId},
};
use arc_store_view::{
    View,
    oplog::OperationAgent,
    synthesis::{SynthesisSnapshot, list_snapshot_ids},
};
use arc_ux::{
    HumanPlainRenderer, HumanRichRenderer, JsonRenderer, OutputEvent, RenderMode, Renderer,
    TerminalCapabilities, arc_error_to_report, detect_capabilities, hyperlink_for_hash,
    hyperlink_for_path,
};
use clap::{
    CommandFactory, Parser, Subcommand,
    builder::styling::{AnsiColor, Effects, Styles},
};
use comfy_table::{Cell, Color, Table, presets};
use inquire::Text;
use owo_colors::OwoColorize;

static OUTPUT_PREFS: OnceLock<(bool, bool)> = OnceLock::new();

/// Serialisable session record written to `.arc/local/session.json` when
/// `ARC_EPHEMERAL_RUNNER` is set.  Keeps the same ephemeral key stable for
/// the lifetime of the workspace so CRDT replica IDs don't flip mid-session.
#[derive(serde::Serialize, serde::Deserialize)]
struct EphemeralSession {
    session_id: String,
    secret_key_bytes: [u8; 32],
}

/// Load or create an ephemeral `Author::Transient` identity scoped to the arc
/// repository at `shared_root`.
///
/// Priority:
/// 1. `.arc/local/session.json` if it already exists (stable within a session).
/// 2. Generate a fresh keypair whose `session_id` comes from the `ARC_EPHEMERAL_RUNNER` environment
///    variable (or the OS process ID as a fallback), then persist it for subsequent commands.
///
/// Global permanent identity (`~/.arc/identity.json`) is intentionally ignored
/// when `ARC_EPHEMERAL_RUNNER` is set — the caller opted into ephemeral mode.
fn load_ephemeral_session_identity(
    shared_root: &std::path::Path,
) -> anyhow::Result<(Author, ed25519_dalek::SigningKey)> {
    let session_path = shared_root.join(".arc").join("local").join("session.json");

    let (author, seed) = if session_path.exists() {
        let json = std::fs::read_to_string(&session_path)
            .context("failed to read .arc/local/session.json")?;
        let s: EphemeralSession =
            serde_json::from_str(&json).context("failed to parse .arc/local/session.json")?;
        let key =
            ed25519_dalek::SigningKey::from_bytes(&s.secret_key_bytes).verifying_key().to_bytes();
        let author = Author::Transient { session_id: s.session_id, key };
        (author, s.secret_key_bytes)
    } else {
        let session_id = std::env::var("ARC_EPHEMERAL_RUNNER")
            .unwrap_or_else(|_| format!("ephemeral-{}", std::process::id()));
        let (author, seed) = arc_store_types::author::generate_transient_keypair_seed(&session_id);
        if let Some(parent) = session_path.parent() {
            std::fs::create_dir_all(parent).context("failed to create .arc/local/ directory")?;
        }
        let record = EphemeralSession { session_id, secret_key_bytes: seed };
        std::fs::write(&session_path, serde_json::to_string_pretty(&record)?)
            .context("failed to write .arc/local/session.json")?;
        (author, seed)
    };

    let signing_key = ed25519_dalek::SigningKey::from_bytes(&seed);
    Ok((author, signing_key))
}

fn load_identity_with_ephemeral_fallback(
    shared_root: &std::path::Path,
) -> anyhow::Result<(Author, ed25519_dalek::SigningKey)> {
    if std::env::var("ARC_EPHEMERAL_RUNNER").is_ok() {
        load_ephemeral_session_identity(shared_root)
    } else {
        load_identity().map_err(|_| {
            anyhow::anyhow!(
                "No cryptographic identity found. Run 'arc auth generate' to create one, or set \
                 ARC_EPHEMERAL_RUNNER for CI/CD pipelines."
            )
        })
    }
}

fn cli_styles() -> Styles {
    Styles::styled()
        .header(AnsiColor::Cyan.on_default() | Effects::BOLD)
        .usage(AnsiColor::Green.on_default() | Effects::BOLD)
        .literal(AnsiColor::Yellow.on_default() | Effects::BOLD)
        .placeholder(AnsiColor::Magenta.on_default())
        .error(AnsiColor::Red.on_default() | Effects::BOLD)
        .valid(AnsiColor::Green.on_default() | Effects::BOLD)
        .invalid(AnsiColor::Red.on_default() | Effects::BOLD)
}

#[derive(Parser)]
#[command(
    name = "arc",
    version,
    about = "Atomic Replayable Changes",
    styles = cli_styles(),
    subcommand_help_heading = "COMMANDS",
    after_help = "DAILY USE\n  arc init\n  arc snap\n  arc watch\n  arc sync\n\nADVANCED\n  Use `arc util`, `arc op`, `arc bench`, and `arc workspace` for advanced workflows."
)]
struct Cli {
    /// Emit versioned machine-readable JSON output where supported.
    #[arg(long, global = true, help_heading = "OUTPUT")]
    json: bool,
    /// Reduce non-essential output where supported.
    #[arg(long, global = true, help_heading = "OUTPUT")]
    quiet: bool,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Initialize a new arc repository.
    Init {
        /// Directory to initialize (defaults to current directory).
        #[arg(help_heading = "DAILY USE")]
        path: Option<String>,
        /// Repository name used for interactive initialization.
        #[arg(long, help_heading = "DAILY USE")]
        repo_name: Option<String>,
        /// Default branch/view name (defaults to `main`).
        #[arg(long, help_heading = "DAILY USE")]
        default_branch: Option<String>,
        /// Optional origin remote URL to configure during init.
        #[arg(long, help_heading = "ADVANCED")]
        remote_url: Option<String>,
        /// Skip auto-detection of an existing Git repository.
        #[arg(long, help_heading = "ADVANCED")]
        no_git: bool,
    },
    /// Snapshot the working directory into a semantic change.
    Snap {
        /// Description of the change. If omitted, arc creates an auto snapshot message.
        #[arg(short, long)]
        message: Option<String>,
        /// Analyze the pending AST atoms and generate the commit message automatically
        /// using any OpenAI-schema LLM (set ARC_AI_KEY, and optionally ARC_AI_URL /
        /// ARC_AI_MODEL to target Ollama, Groq, or any local inference server).
        #[arg(long)]
        auto_msg: bool,
        /// Interactively select which AST atoms to stage.
        #[arg(short = 'i', long, default_value_t = false)]
        interactive: bool,
    },
    /// Start the arc-watch daemon for debounced auto-snapshots.
    Watch {
        /// Optional root path to watch recursively (defaults to current directory).
        #[arg(help_heading = "DAILY USE")]
        path: Option<PathBuf>,
    },
    /// Show the change log.
    Log {
        /// Semantic search query to filter changes by intent similarity.
        /// Requires the local embedding model (downloaded on first use).
        #[arg(long)]
        intent: Option<String>,
        /// Revset query expression used to select which changes to show.
        #[arg(short = 'r', long)]
        revset: Option<String>,
        /// Row template for non-semantic log output.
        /// Supported placeholders: {id}, {id_short}, {author}, {intent},
        /// {state_badges}, {ref_badges}, {badges}
        #[arg(long)]
        template: Option<String>,
    },
    /// Show uncommitted changes as semantic AST atoms.
    Status,
    /// Port an existing change into the current view by its hash.
    CherryPick {
        /// Full 64-character hex hash of the change to port.
        hash: String,
    },
    /// Query semantic blame: who authored each AST node in a file.
    Blame {
        /// Path to the file (relative to the repository root).
        filepath: String,
    },
    /// Stash dirty working-directory changes into a hidden view.
    Stash {
        #[command(subcommand)]
        action: StashAction,
    },
    /// Manage views (branches).
    View {
        #[command(subcommand)]
        action: ViewAction,
    },
    /// AI-powered operations.
    Ai {
        #[command(subcommand)]
        action: AiAction,
    },
    /// Import history from another VCS.
    Import {
        #[command(subcommand)]
        source: ImportSource,
    },
    /// Run an interactive onboarding tour for new arc users.
    Tour,
    /// Perform native TCP sync handshake with a remote arc peer.
    Sync {
        /// Native sync server address, e.g. 127.0.0.1:9000.
        address: String,
    },
    /// Fetch missing changes from a remote repository.
    Fetch {
        /// Path to the remote repository.
        remote_path: String,
        /// Name of the remote view to fetch.
        view: String,
    },
    /// Pull changes from a remote repository and merge into the current view.
    Pull {
        /// Path to the remote repository.
        remote_path: String,
        /// Name of the remote view to pull.
        view: String,
    },
    /// Verify cryptographic provenance of all changes in the graph.
    Verify {
        /// Also validate reproducible workspace tooling policies under `.config/`.
        #[arg(long, default_value_t = false)]
        tooling: bool,
        /// Also validate GitHub governance and CI policy files under `.github/`.
        #[arg(long, default_value_t = false)]
        governance: bool,
        /// Also validate root workspace policy files (.editorconfig/.gitattributes/etc).
        #[arg(long, default_value_t = false)]
        workspace_policy: bool,
    },
    /// Manage arc identity (cryptographic key-pair).
    Auth {
        #[command(subcommand)]
        action: AuthAction,
    },
    /// Start the native TCP sync server for the current repository.
    Serve {
        /// TCP port to listen on.
        #[arg(short, long, default_value_t = 8080)]
        port: u16,
    },
    /// Manage named remote aliases.
    Remote {
        #[command(subcommand)]
        action: RemoteAction,
    },
    /// Create a cryptographically signed, immutable tag pointing to a change.
    ///
    /// Argument order: `arc tag <name> <hash-or-ref>`  (name first, target second).
    Tag {
        /// Tag name (e.g. "v1.0.0").
        name: String,
        /// Commit reference: 64-char hex, short prefix, view name, or `HEAD`.
        hash: String,
    },
    /// List all tags in the repository.
    Tags,
    /// Create or move one or more tags to a target revision.
    TagSet {
        /// Allow moving existing tags.
        #[arg(long, default_value_t = false)]
        allow_move: bool,
        /// Target revision to point tags to.
        #[arg(long, short = 'r', default_value = "@")]
        rev: String,
        /// Tag names to create or update.
        #[arg(required = true)]
        names: Vec<String>,
    },
    /// Delete tags matching one or more glob-like patterns.
    TagDelete {
        /// Tag name patterns (`*` and `?` supported).
        #[arg(required = true)]
        names: Vec<String>,
    },
    /// List tags optionally filtered by glob-like patterns.
    TagList {
        /// Optional tag name patterns (`*` and `?` supported).
        names: Vec<String>,
    },
    /// Manage mutable bookmarks pointing at change heads.
    Bookmark {
        #[command(subcommand)]
        action: BookmarkAction,
    },
    /// Semantically revert a change by rolling back its AST atoms.
    Revert {
        /// Commit reference: 64-char hex, short prefix, view name, or HEAD/HEAD~N.
        hash: String,
    },
    /// Restore a file to its snapped state in the current view.
    Restore {
        /// Path to the file to restore (relative to the repository root).
        filepath: String,
    },
    /// Print a telemetry dashboard for the current repository.
    Info,
    /// Switch to a different view — alias for `view switch`.
    Checkout {
        /// Name of the view to switch to.
        name: String,
    },
    /// Create a view or list views — alias for `view create` / `view list`.
    ///
    /// With a name: creates the named view. Without a name: lists all views,
    /// marking the active one with `*`.
    Branch {
        /// When provided, creates a new view with this name.
        /// When omitted, lists all existing views.
        name: Option<String>,
    },
    /// Unsupported — arc uses `snap` instead of `commit`.
    Commit,
    /// Abandon one or more head revisions by moving the view frontier to their parents.
    ///
    /// Arc's AST-CRDT model treats this as a frontier rewrite (no textual rebase).
    Abandon {
        /// Revisions to abandon (defaults to `@`).
        #[arg(long, short = 'r')]
        revisions: Vec<String>,
    },
    /// Update the current head message without changing semantic content.
    Describe {
        /// New change description.
        #[arg(long = "message", short = 'm')]
        message: String,
        /// Target revision. Only `@` / `HEAD` is currently supported.
        #[arg(long, short = 'r', default_value = "@")]
        revision: String,
    },
    /// Undo the last view-mutating operation using the operation log (O(1) pointer-swap).
    Undo,
    /// Redo the most recently undone operation.
    Redo,
    /// Print the current workspace root directory.
    Root,
    /// Display version information.
    Version,
    /// Bisect history to isolate first bad (or good) change.
    Bisect {
        #[command(subcommand)]
        action: BisectAction,
    },
    /// Benchmark core DAG and revset operations.
    Bench {
        #[command(subcommand)]
        action: BenchAction,
    },
    /// Inspect and manage the spacetime operation log.
    Op {
        #[command(subcommand)]
        action: OpAction,
    },
    /// Manage semantic sparse checkouts (monorepo tamer).
    Sparse {
        #[command(subcommand)]
        action: SparseAction,
    },
    /// Manage explicit binary/blob tracking extensions.
    Blob {
        #[command(subcommand)]
        action: BlobAction,
    },
    /// Manage sub-repository mounts (mathematical submodule replacement).
    Mount {
        #[command(subcommand)]
        action: MountAction,
    },
    /// Manage linked workspaces (split-root, jj-style).
    Workspace {
        #[command(subcommand)]
        action: WorkspaceAction,
    },
    /// Infrequently used utility commands.
    Util {
        #[command(subcommand)]
        action: UtilAction,
    },
    /// Run garbage collection to reclaim unreachable CAS objects.
    Gc {
        /// Print what would be deleted without removing anything.
        #[arg(long, default_value_t = false)]
        dry_run: bool,
    },
    /// Compact causally-stable history into a single Genesis base state.
    ///
    /// All changes in the causal-stability frontier are collapsed into one
    /// synthetic "Compacted Base State" change, permanently eliminating
    /// CRDT tombstones and reducing repository size.  An Epoch Map is
    /// written to `.arc/epochs` so future `hydrate` calls transparently
    /// redirect compacted IDs; no live Change object is ever rewritten.
    Compact,
    /// Amend the most recent snap, optionally replacing its message.
    ///
    /// Rewrites the last change in-place: the amended commit gets a new hash
    /// and the old hash is added to the Epoch Map so peers transparently graft
    /// onto the amended history.
    Amend {
        /// New commit message.  If omitted, the original message is kept.
        #[arg(short, long)]
        message: Option<String>,
    },
    /// Absorb working-copy edits into existing history.
    ///
    /// Current scaffold supports `--ast` mode with conservative safety checks.
    Absorb {
        /// Use AST-aware absorption mode.
        #[arg(long, default_value_t = false)]
        ast: bool,
    },
    /// Squash a contiguous linear spine of changes into a single change.
    ///
    /// All changes from `--into <change>` up to the current HEAD are fused
    /// into one new change.  The target change's deps are preserved and the
    /// working directory is rematerialised.
    Squash {
        /// Change to squash into (rev, hash prefix, or HEAD~N).
        #[arg(long)]
        into: String,
    },
    /// Reorder a contiguous linear chain of revisions.
    ///
    /// Example: `arc reorder HEAD~2 HEAD HEAD~1`.
    Reorder {
        /// Desired oldest->newest revision order.
        #[arg(required = true)]
        revs: Vec<String>,
    },
    /// Restack a revision chain with resumable checkpoints.
    Restack {
        /// Resume a previously paused restack transaction.
        #[arg(
            long = "continue",
            default_value_t = false,
            conflicts_with_all = ["abort", "revs"]
        )]
        continue_mode: bool,
        /// Abort a pending restack transaction and restore original heads.
        #[arg(long, default_value_t = false, conflicts_with = "revs")]
        abort: bool,
        /// Desired oldest->newest revision order.
        #[arg(
            required_unless_present_any = ["continue_mode", "abort"],
            conflicts_with_all = ["continue_mode", "abort"]
        )]
        revs: Vec<String>,
    },
    /// Interactively edit the AST content of an existing change.
    ///
    /// Two-step workflow:
    ///  1. `arc diffedit --prepare <change>` — check out the change's state to the working dir.
    ///  2. Edit files with any editor.
    ///  3. `arc diffedit --apply` — compute the diff and record the replacement.
    Diffedit {
        /// Prepare a diffedit session for the given change.
        #[arg(long, conflicts_with = "apply")]
        prepare: Option<String>,
        /// Apply the active diffedit session (after editing).
        #[arg(long)]
        apply: bool,
        /// Replace the change's commit message.
        #[arg(short, long)]
        message: Option<String>,
    },
    /// Configure your name, email, and Ed25519 signing identity in one step.
    ///
    /// Equivalent to `arc auth login` but with a simpler interface.
    /// Generates a fresh Ed25519 keypair and persists it alongside the identity.
    Identity {
        /// Your full name.
        #[arg(long)]
        name: String,
        /// Your email address.
        #[arg(long)]
        email: String,
    },
    /// Show uncommitted working-directory changes as a coloured diff.
    ///
    /// By default, renders a Sesame-aligned text diff with per-token inline
    /// highlighting (the "Micro" view).  Pass `--semantic` to switch to a
    /// structural AST view that summarises the high-level *intent* of each
    /// change — moves, refactors, insertions — without raw text noise
    /// (the "Macro" view).  The two views are complementary: use `--semantic`
    /// to understand *what* changed architecturally, then plain `arc diff` to
    /// verify *how* it was implemented.
    Diff {
        /// Show the structural AST (intent) diff instead of the text diff.
        ///
        /// Renders each pending atom as a named structural operation:
        /// `[+] Insert function 'parse'`, `[~] Move 'validate' → 'validator.rs'`,
        /// `[≈] Refactor variable 'obj': renamed to 'item'`.  Multi-mappings
        /// (e.g. three deletion sites that all map to one extracted method) are
        /// shown explicitly, which a text diff cannot express.
        #[arg(long)]
        semantic: bool,
    },
    /// Push local changes to a remote repository.
    Push {
        /// Remote URL (or configured remote alias) to push to.
        remote_url: String,
        /// Optional view name to push (defaults to the current view).
        view: Option<String>,
    },
    /// Get or set arc configuration / global aliases.
    Config {
        /// Apply operation to the global config instead of the local one.
        #[arg(long)]
        global: bool,
        #[command(subcommand)]
        action: ConfigAction,
    },
    /// Inspect policy resolution and provenance.
    Policy {
        #[command(subcommand)]
        action: PolicyAction,
    },
    /// Capture and inspect architecture synthesis snapshots.
    Synthesis {
        #[command(subcommand)]
        action: SynthesisAction,
    },
    /// Package OS metadata and an anonymized DAG dump for bug reporting.
    BugReport {
        /// Output file path (default: `./arc-bugreport.json`).
        #[arg(short, long)]
        output: Option<String>,
        /// Include raw `intent` strings in the dump (opt-in — may contain
        /// proprietary information).
        #[arg(long)]
        include_raw_intent: bool,
    },
    /// Start the IDE JSON-RPC daemon.
    #[command(hide = true)]
    Daemon,
}

#[derive(Subcommand)]
enum OpAction {
    /// Print the operation log in reverse-chronological order.
    Log,
    /// Restore the current view to the state after the selected operation.
    Restore {
        /// Operation id (short id or snapshot prefix) shown by `arc op log`.
        op_id: String,
    },
    /// Revert the selected operation by restoring its pre-operation heads.
    Revert {
        /// Operation id (short id or snapshot prefix) shown by `arc op log`.
        op_id: String,
    },
}

#[derive(Subcommand)]
enum BisectAction {
    /// Start a new bisect session over a revset range.
    Start {
        /// Revset range expression (e.g. `ancestors(@)`).
        #[arg(long, short = 'r', required = true)]
        range: String,
        /// Find first good revision instead of first bad.
        #[arg(long, default_value_t = false)]
        find_good: bool,
    },
    /// Show or compute the next revision to test.
    Next,
    /// Mark current revision as good.
    Good,
    /// Mark current revision as bad.
    Bad,
    /// Print bisect session status.
    Status,
    /// Reset (clear) bisect session state.
    Reset,
}

#[derive(Subcommand)]
enum BenchAction {
    /// Benchmark common-ancestor computation.
    CommonAncestors {
        /// Left revision.
        left: String,
        /// Right revision.
        right: String,
        /// Number of benchmark iterations.
        #[arg(long, default_value_t = 100)]
        iterations: u32,
    },
    /// Benchmark ancestor predicate.
    IsAncestor {
        /// Potential ancestor revision.
        ancestor: String,
        /// Potential descendant revision.
        descendant: String,
        /// Number of benchmark iterations.
        #[arg(long, default_value_t = 100)]
        iterations: u32,
    },
    /// Benchmark commit hash-prefix resolution.
    ResolvePrefix {
        /// Prefix to resolve.
        prefix: String,
        /// Number of benchmark iterations.
        #[arg(long, default_value_t = 100)]
        iterations: u32,
    },
    /// Benchmark revset compilation and evaluation.
    Revset {
        /// Revset expression.
        revset: String,
        /// Number of benchmark iterations.
        #[arg(long, default_value_t = 100)]
        iterations: u32,
    },
}

#[derive(Subcommand)]
enum StashAction {
    /// Save all dirty changes and reset the working directory.
    Push,
    /// Apply the most recent stash and drop it.
    Pop,
    /// List all stored stashes.
    List,
}

#[derive(Subcommand)]
enum ViewAction {
    /// Create a new view forked from the current view.
    Create {
        /// Name of the new view.
        name: String,
    },
    /// Switch the working directory to a different view.
    Switch {
        /// Name of the view to switch to.
        name: String,
    },
    /// Merge another view into the current view.
    Merge {
        /// Name of the view to merge.
        name: String,
    },
}

#[derive(Subcommand)]
enum AiAction {
    /// Resolve a pending semantic conflict using the AI resolver.
    ///
    /// Uses `[ai]` provider settings from config and reads the API key from
    /// `ARC_AI_API_KEY` at runtime. The resolved content is written to the
    /// working directory as a Ghost Node - run 'arc ai approve' to finalise.
    Resolve,
    /// Approve and cryptographically sign a pending AI change (Ghost Node).
    ///
    /// Constructs Author::AI { model, human_sponsor } signed by the active
    /// human identity key, commits the change to CAS, and advances the view.
    Approve {
        /// Optional expected hash (or prefix) of the sponsored node.
        hash: Option<String>,
    },
    /// Generate code using an AI agent and apply it as a Ghost Node.
    ///
    /// Queries the local intent vector index for context, calls the LLM, and
    /// writes the result to --file.  Run 'arc ai approve' to finalise.
    Generate {
        /// High-level goal for the AI (e.g. "add retry backoff to client.rs").
        #[arg(long)]
        goal: String,
        /// Path to the file the AI should modify (relative to repo root).
        #[arg(long)]
        file: Option<std::path::PathBuf>,
    },
}

#[derive(Subcommand)]
enum ImportSource {
    /// Import from a local Git repository.
    Git {
        /// Path to the Git repository.
        git_path: String,
    },
}

#[derive(Subcommand)]
enum AuthAction {
    /// Create and persist a new Ed25519 identity.
    Login {
        /// Your full name.
        #[arg(long)]
        name: String,
        /// Your email address.
        #[arg(long)]
        email: String,
    },
    /// Print the currently active identity.
    Whoami,
}

#[derive(Subcommand)]
enum RemoteAction {
    /// Add or update a named remote alias.
    Add {
        /// Short name for the remote (e.g. "origin").
        name: String,
        /// URL or filesystem path of the remote repository.
        url: String,
    },
    /// List all configured remote aliases.
    List,
    /// Remove an existing remote alias.
    Remove {
        /// Short name of the remote to delete (e.g. "origin").
        name: String,
    },
}

#[derive(Subcommand)]
enum BookmarkAction {
    /// Create a new bookmark at a target revision.
    Create {
        /// Bookmark name (supports slash namespaces, e.g. trunk/main).
        name: String,
        /// Target revision (hash, view, @, HEAD, or prefix).
        #[arg(default_value = "@")]
        rev: String,
    },
    /// Set a bookmark to a target revision (create if missing).
    Set {
        /// Bookmark name (supports slash namespaces, e.g. trunk/main).
        name: String,
        /// Target revision (hash, view, @, HEAD, or prefix).
        #[arg(default_value = "@")]
        rev: String,
    },
    /// Move an existing bookmark, enforcing fast-forward by default.
    Move {
        /// Bookmark name to move.
        name: String,
        /// New target revision.
        rev: String,
        /// Allow moving backwards in history.
        #[arg(long, default_value_t = false)]
        allow_backwards: bool,
    },
    /// Delete a bookmark.
    Delete {
        /// Bookmark name to delete.
        name: String,
    },
    /// List all bookmarks grouped by target change.
    List,
}

#[derive(Subcommand)]
enum SparseAction {
    /// Set or mutate sparse cone path prefixes.
    Set {
        /// Replace sparse patterns with these path prefixes.
        #[arg(value_name = "PATH")]
        paths: Vec<String>,
        /// Add one or more path prefixes to the existing sparse cone.
        #[arg(long, value_name = "PATH")]
        add: Vec<String>,
        /// Remove one or more path prefixes from the existing sparse cone.
        #[arg(long, value_name = "PATH")]
        remove: Vec<String>,
        /// Clear all sparse prefixes before applying `--add` values.
        #[arg(long, default_value_t = false)]
        clear: bool,
    },
    /// Edit sparse cone patterns in your configured text editor.
    Edit,
    /// List the active sparse cone patterns.
    List,
    /// Remove the sparse filter and restore the full working directory.
    Reset,
}

#[derive(Subcommand)]
enum BlobAction {
    /// Track a binary/blob file extension (for example: png, zip, wasm).
    Track {
        /// Extension with or without leading dot (e.g. `png` or `.png`).
        extension: String,
    },
    /// List tracked binary/blob extensions.
    Ls,
}

#[derive(Subcommand)]
enum MountAction {
    /// Declare a sub-repository mount in the current view.
    Add {
        /// Spacetime coordinate (for example `arc://org/repo@<hash>`).
        coordinate: String,
        /// Local path at which to mount the sub-repository.
        path: String,
    },
    /// Clone / update all declared mounts.
    Sync,
}

#[derive(Subcommand)]
enum WorkspaceAction {
    /// Create a linked workspace at the given path.
    Add {
        /// Directory to create the workspace in.
        path: String,
        /// View to check out (defaults to the current view).
        #[arg(long)]
        view: Option<String>,
    },
    /// List all workspaces sharing this repository's CAS.
    List,
    /// Stop tracking a workspace by removing its link manifest.
    Forget {
        /// Workspace directory to forget.
        path: String,
    },
    /// Rename a workspace directory on disk.
    Rename {
        /// Existing workspace directory.
        old_path: String,
        /// New workspace directory.
        new_path: String,
    },
    /// Print canonical root path of the current or named workspace directory.
    Root {
        /// Optional workspace directory path (defaults to current workspace).
        path: Option<String>,
    },
    /// Refresh the workspace state if stale by taking a snapshot.
    UpdateStale,
}

#[derive(Subcommand)]
enum UtilAction {
    /// Print shell completion scripts for arc.
    Completion {
        /// Target shell.
        shell: ShellCompletion,
    },
    /// Snapshot the working copy if needed.
    Snapshot,
    /// Execute an external command with ARC_WORKSPACE_ROOT in the environment.
    Exec {
        /// External command to execute.
        command: String,
        /// Arguments to pass to the external command.
        args: Vec<String>,
    },
    /// Print a JSON schema for arc config keys.
    ConfigSchema,
    /// Install arc man pages into the provided root directory.
    InstallManPages {
        /// Path where `man1` should be created.
        path: std::path::PathBuf,
    },
    /// Print CLI help for all subcommands in Markdown.
    MarkdownHelp,
    /// Run utility garbage collection (alias for repository GC).
    Gc {
        /// Time threshold. Currently only `now` is supported.
        #[arg(long)]
        expire: Option<String>,
    },
    /// UX renderer demo for replace-on-done and clickable paths.
    UxDemo,
}

#[derive(clap::ValueEnum, Clone, Copy, Debug, Eq, Hash, PartialEq)]
enum ShellCompletion {
    Bash,
    Elvish,
    Fish,
    Nushell,
    PowerShell,
    Zsh,
}

#[derive(Subcommand)]
enum ConfigAction {
    /// Define or overwrite a command alias (stored in global config).
    Alias {
        /// Short alias name (e.g. "st").
        name: String,
        /// Expansion string (e.g. "status").
        expansion: String,
    },
    /// List all configured aliases (global + local).
    Aliases,
    /// Read a typed configuration value.
    ///
    /// Known keys: `user.name`, `user.email`, `ui.color`, `merge.tool`,
    /// `ai.provider`, `ai.model`, `ai.endpoint`, `remotes.<name>`, `aliases.<name>`.
    Get {
        /// Dot-separated config key (e.g. `ui.color`).
        key: String,
    },
    /// Write a typed configuration value.
    Set {
        /// Dot-separated config key (e.g. `ui.color`).
        key: String,
        /// New value as a string.
        value: String,
    },
    /// Remove a configuration key.
    Unset {
        /// Dot-separated config key to remove.
        key: String,
    },
    /// Print the path to the target configuration file.
    Path,
    /// Edit the target configuration file in your text editor.
    Edit,
    /// Print all configuration values (global + local merged).
    List,
}

#[derive(Subcommand)]
enum PolicyAction {
    /// Explain whether a path is ignored and why.
    Explain {
        /// Path relative to workspace root to inspect.
        path: String,
        /// Resolve as config key instead of ignore path.
        #[arg(long, default_value_t = false)]
        config: bool,
    },
}

#[derive(Subcommand)]
enum SynthesisAction {
    /// Capture a deterministic synthesis snapshot from one or more files.
    Capture {
        /// Source label (e.g. `jj-main`, `git-master`).
        #[arg(long, default_value = "jj-main")]
        source: String,
        /// Input files to include in the synthesis snapshot.
        #[arg(required = true)]
        files: Vec<String>,
    },
    /// Show one snapshot by id.
    Show {
        /// 64-hex snapshot id.
        id: String,
    },
    /// List all captured synthesis snapshots.
    List,
}

#[tracing::instrument(skip(files), fields(source = %source))]
fn capture_synthesis_snapshot(source: &str, files: &[String]) -> anyhow::Result<()> {
    let repo = Repository::open(".")?;
    let paths: Vec<std::path::PathBuf> = files.iter().map(std::path::PathBuf::from).collect();
    let snapshot = SynthesisSnapshot::capture(&repo.work_root, source.to_string(), &paths)?;
    snapshot.persist(&repo.shared_root)?;

    tracing::info!(
        id = %snapshot.id,
        artifact_count = snapshot.artifacts.len(),
        "synthesis snapshot captured"
    );

    println!("Captured synthesis snapshot: {}", snapshot.id);
    for artifact in &snapshot.artifacts {
        println!(
            "  - {} ({} bytes, {})",
            artifact.path,
            artifact.byte_len,
            artifact.content_hash.iter().map(|b| format!("{b:02x}")).collect::<String>()
        );
    }
    Ok(())
}

#[tracing::instrument(fields(snapshot_id = %id))]
fn show_synthesis_snapshot(id: &str) -> anyhow::Result<()> {
    let repo = Repository::open(".")?;
    let parsed = SnapshotId::from_hex(id)?;
    let snapshot = SynthesisSnapshot::load(&repo.shared_root, parsed)?;

    println!("Snapshot: {}", snapshot.id);
    println!("Source: {}", snapshot.source);
    println!("Created: {}", snapshot.created_at_unix);
    println!("Artifacts:");
    for artifact in &snapshot.artifacts {
        println!(
            "  - {} ({} bytes, {})",
            artifact.path,
            artifact.byte_len,
            artifact.content_hash.iter().map(|b| format!("{b:02x}")).collect::<String>()
        );
    }
    Ok(())
}

#[cfg(test)]
fn diagnostic_lines(error: &anyhow::Error) -> Vec<String> {
    use arc_diagnostics::ArcError;
    let mut lines = Vec::new();
    let diagnostic = ArcError::from_anyhow(error);
    lines.push(format!("error: {}", diagnostic.message()).red().bold().to_string());
    for cause in diagnostic.causes() {
        lines.push(format!("caused by: {cause}").red().to_string());
    }
    if let Some(hint) = diagnostic.hint() {
        lines.push("-".repeat(60).dimmed().to_string());
        lines.push(format!("Hint: {}", hint.explanation()).yellow().to_string());
        if let Some(command) = hint.suggested_command() {
            lines.push(format!("Try: {command}").cyan().bold().to_string());
        }
    }
    lines
}

fn render_diagnostic_error(error: anyhow::Error, force_json: bool, quiet: bool) {
    let report = arc_error_to_report(arc_error::Error::from_error(AnyhowCompat(error)));
    let caps = detect_capabilities(force_json, quiet);
    let mut renderer = make_renderer(&caps);
    renderer.render(OutputEvent::Diagnostic(report));
}

#[derive(Debug)]
struct AnyhowCompat(anyhow::Error);

impl std::fmt::Display for AnyhowCompat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:#}", self.0)
    }
}

impl std::error::Error for AnyhowCompat {}

fn make_renderer(caps: &TerminalCapabilities) -> Box<dyn Renderer> {
    match caps.mode {
        RenderMode::HumanRich => Box::new(HumanRichRenderer::new(caps.supports_osc8)),
        RenderMode::HumanPlain => Box::new(HumanPlainRenderer),
        RenderMode::MachineJson => Box::new(JsonRenderer { ndjson: true }),
    }
}

fn run_mock_snap_demo(caps: &TerminalCapabilities) {
    let mut renderer = make_renderer(caps);
    renderer.render(OutputEvent::Started("snap: scanning semantic atoms".to_string()));

    for idx in 1..=3_u64 {
        std::thread::sleep(std::time::Duration::from_millis(150));
        renderer.render(OutputEvent::Progress(idx, 3, format!("indexed {idx}/3 files")));
    }

    let hash = "2d33b4f5f55fa8a8d5f704d5909b8f79e76ab26a4f3b7b7d30aef7d56e05e6aa";
    let details = vec![
        format!("change: {}", hyperlink_for_hash(hash, caps.supports_osc8)),
        format!("updated: {}", hyperlink_for_path("src/lib.rs", 128, caps.supports_osc8)),
        format!(
            "updated: {}",
            hyperlink_for_path("crates/arc-cli/src/main.rs", 3344, caps.supports_osc8)
        ),
    ];
    renderer.render(OutputEvent::Success("snap completed".to_string(), details));
}

fn resolve_sync_token() -> anyhow::Result<Option<String>> {
    if let Ok(token) = std::env::var("ARC_SYNC_TOKEN")
        && !token.trim().is_empty()
    {
        return Ok(Some(token));
    }

    if !std::io::stdin().is_terminal() {
        return Ok(None);
    }

    eprint!("Auth token (leave blank for loopback/no-auth): ");
    let token = rpassword::read_password().context("failed to read hidden auth token")?;
    if token.trim().is_empty() { Ok(None) } else { Ok(Some(token)) }
}

fn normalize_blob_extension(raw: &str) -> anyhow::Result<String> {
    let normalized = raw.trim().trim_start_matches('.').to_ascii_lowercase();
    anyhow::ensure!(!normalized.is_empty(), "extension cannot be empty");
    anyhow::ensure!(
        normalized.chars().all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_'),
        "extension must contain only ASCII letters, digits, '-' or '_'"
    );
    Ok(normalized)
}

fn run_cli() -> anyhow::Result<()> {
    // Initialise the tempfile registry eagerly so no allocations happen inside
    // shutdown cleanup hooks later.
    arc_store_view::tempfile::init();

    let interrupts = arc_cli::devtools::interrupt::InterruptState::new();

    // Install cleanup hooks that run in normal thread context on shutdown.
    arc_cli::devtools::interrupt::install_cleanup_handlers(interrupts.clone())?;

    init_tracing("arc_cli");
    // --- Recursive alias interception with cycle detection ---------------
    let mut raw_args: Vec<String> =
        arc_cli::devtools::multicall::normalize_invocation_args(std::env::args().collect());
    if let Ok(config) = load_merged_config(std::path::Path::new(".")) {
        raw_args = expand_command_aliases(&config, raw_args)?;
    }
    let cli = Cli::parse_from(&raw_args);
    let term_caps = detect_capabilities(cli.json, cli.quiet);
    let _ = OUTPUT_PREFS.set((cli.json, cli.quiet));

    // Extension-style global pre-command behavior driven by config.
    if !cli.quiet
        && let Ok(config) = load_merged_config(std::path::Path::new("."))
        && let Some(greet) = config.ui.greet.as_deref()
        && !greet.trim().is_empty()
    {
        println!("{greet}");
    }

    arc_cli::devtools::run_wrapper::run_with_telemetry("arc", &interrupts, || {
        match cli.command {
            Command::Init { path, repo_name, default_branch, remote_url, no_git } => {
                let mut prompted_repo_name = repo_name;
                let mut prompted_default_branch = default_branch;
                let mut prompted_remote_url = remote_url;

                if prompted_repo_name.is_none()
                    && prompted_default_branch.is_none()
                    && prompted_remote_url.is_none()
                    && path.is_none()
                    && std::io::stdin().is_terminal()
                {
                    let default_repo_name = std::env::current_dir()
                        .ok()
                        .and_then(|dir| {
                            dir.file_name().map(|name| name.to_string_lossy().to_string())
                        })
                        .filter(|name| !name.trim().is_empty())
                        .unwrap_or_else(|| "arc-repo".to_string());

                    prompted_repo_name = Some(
                        Text::new("Repository name")
                            .with_default(&default_repo_name)
                            .prompt()
                            .context("interactive prompt failed for repository name")?,
                    );
                    prompted_default_branch = Some(
                        Text::new("Default branch")
                            .with_default("main")
                            .prompt()
                            .context("interactive prompt failed for default branch")?,
                    );

                    let remote = Text::new("Remote URL (optional)")
                        .with_placeholder("https://example.com/org/repo.git")
                        .prompt()
                        .context("interactive prompt failed for remote URL")?;
                    if !remote.trim().is_empty() {
                        prompted_remote_url = Some(remote.trim().to_string());
                    }
                }

                let target = path.unwrap_or_else(|| ".".to_string());
                let target_path = std::path::Path::new(&target);
                let display_repo_name = prompted_repo_name
                    .as_deref()
                    .map(str::trim)
                    .filter(|name| !name.is_empty())
                    .map(ToOwned::to_owned)
                    .unwrap_or_else(|| {
                        target_path
                            .file_name()
                            .map(|name| name.to_string_lossy().to_string())
                            .filter(|name| !name.trim().is_empty())
                            .unwrap_or_else(|| "arc-repo".to_string())
                    });

                // --- Git auto-detection (Phase D) ---
                let do_import = if !no_git {
                    match arc_git::resolve_git_dir(target_path) {
                        Ok(_git_dir) => {
                            // Count commits for the prompt.
                            let count = arc_git::analyze_git_repo(target_path)
                                .map(|a| a.commit_count)
                                .unwrap_or(0);
                            eprint!(
                                "Detected Git repository with {count} commit{}. Import history as \
                                 arc Changes? [Y/n] ",
                                if count == 1 { "" } else { "s" }
                            );
                            use std::io::Write;
                            let _ = std::io::stderr().flush();
                            let mut line = String::new();
                            std::io::stdin().read_line(&mut line).ok();
                            matches!(line.trim().to_lowercase().as_str(), "" | "y" | "yes")
                        }
                        Err(_) => false,
                    }
                } else {
                    false
                };

                let mut repo = Repository::init(&target)?;

                if let Some(branch) = prompted_default_branch
                    .as_deref()
                    .map(str::trim)
                    .filter(|branch| !branch.is_empty() && *branch != "main")
                {
                    repo.create_view(branch)?;
                    repo.switch_view(branch)?;
                }

                if let Some(url) =
                    prompted_remote_url.as_deref().map(str::trim).filter(|url| !url.is_empty())
                {
                    repo.add_remote("origin", url)?;
                }
                println!("Initialized empty arc repository '{display_repo_name}' in {target}/.arc");

                if do_import {
                    // --- Trust Anchor identity flow ---
                    let (author, signing_key) = match load_identity() {
                        Ok(pair) => pair,
                        Err(_) => {
                            // No identity yet: read git user.name/email as defaults.
                            let (git_name, git_email) = arc_git::read_git_user_config(target_path)
                                .unwrap_or_else(|| ("arc user".into(), "".into()));
                            eprintln!(
                                "No arc cryptographic identity found.\nGenerating Ed25519 keypair \
                                 for {git_name} <{git_email}>\nPress Enter to confirm, or Ctrl-C \
                                 to abort."
                            );
                            use std::io::Write;
                            let _ = std::io::stderr().flush();
                            let mut _confirm = String::new();
                            std::io::stdin().read_line(&mut _confirm).ok();
                            save_identity(&git_name, &git_email)?;
                            // Also persist display identity into config.toml [user].
                            let mut cfg = ArcConfig::default();
                            cfg.user.name = Some(git_name);
                            cfg.user.email = Some(git_email);
                            save_global_config(&cfg)?;
                            load_identity()?
                        }
                    };

                    // --- Import ---
                    let n = import_repo(&target, &mut repo, &author, &signing_key)?;
                    println!(
                        "Imported {n} change{} across all branches.\nNote: Rust source files \
                         imported semantically; other file types imported as blobs.",
                        if n == 1 { "" } else { "s" }
                    );
                }
            }
            Command::Snap { message, auto_msg, interactive } => {
                use std::io::Write;

                let mut repo = Repository::open(".")?;
                let (author, signing_key) =
                    load_identity_with_ephemeral_fallback(&repo.shared_root)?;
                repo.set_identity(author, signing_key);

                let final_message: String = if auto_msg {
                    // Derive the diff summary from the pending AST atoms so the LLM
                    // gets precise semantic context rather than raw file bytes.
                    let atoms = repo.status()?;
                    if atoms.is_empty() {
                        println!("Nothing to snap — working directory is clean.");
                        return Ok(());
                    }
                    let mut diff_summary = String::new();
                    for atom in &atoms {
                        diff_summary.push_str(&format!("{atom:?}\n"));
                    }
                    // Keep the prompt within a safe context-window budget.
                    diff_summary.truncate(2000);

                    eprint!("{} Analyzing changes", "🧠".cyan());
                    let _ = std::io::stderr().flush();

                    let rt =
                        tokio::runtime::Runtime::new().context("failed to start async runtime")?;
                    match rt.block_on(arc_ai::generate_message(&diff_summary)) {
                        Ok(msg) => {
                            eprintln!(); // newline after the spinner text
                            println!("{} {msg}", "Generated:".green().bold());
                            msg
                        }
                        Err(e) => {
                            // Transient failure: print the reason and fall back to an
                            // interactive prompt so the user never loses the snap.
                            eprintln!("\n{} AI generation failed: {e}", "⚠".yellow().bold());
                            eprint!("Enter commit message manually: ");
                            std::io::stderr().flush().ok();
                            let mut fallback = String::new();
                            std::io::stdin().read_line(&mut fallback)?;
                            let trimmed = fallback.trim().to_owned();
                            if trimmed.is_empty() {
                                anyhow::bail!("Aborted: empty commit message.");
                            }
                            trimmed
                        }
                    }
                } else if let Some(message) = message {
                    message
                } else {
                    "auto: snapshot safe working-directory changes".to_owned()
                };

                if let Some(id) = repo.snap(&final_message, interactive)? {
                    let hex: String = id.iter().map(|b| format!("{b:02x}")).collect();
                    println!("snap {hex}");
                } else {
                    println!("Nothing to snap — working directory matches history.");
                }
            }
            Command::Log { intent, revset, template } => {
                let mut repo = Repository::open(".")?;
                let (author, signing_key) =
                    load_identity_with_ephemeral_fallback(&repo.shared_root)?;
                repo.set_identity(author, signing_key);
                let _ = repo.snapshot()?;
                if let Some(query) = intent {
                    let results = repo.log_semantic(&query, 10)?;
                    if results.is_empty() {
                        println!("No semantically similar changes found.");
                    } else {
                        let mut table = Table::new();
                        table.load_preset(presets::NOTHING);
                        for (change, score) in results {
                            let hex: String =
                                change.id.iter().map(|b| format!("{b:02x}")).collect();
                            let author_str = match &change.author {
                                Author::Human { name, email, .. } => format!("{name} <{email}>"),
                                Author::AI { model, human_sponsor } => {
                                    let sponsor: String =
                                        human_sponsor.iter().map(|b| format!("{b:02x}")).collect();
                                    format!("{model} | sponsor:{}", &sponsor[..8])
                                }
                                Author::Server { canonical_id, .. } => {
                                    format!("{canonical_id} [server]")
                                }
                                Author::Transient { session_id, .. } => {
                                    format!("{session_id} [transient]")
                                }
                            };
                            table.add_row(vec![
                                Cell::new(&hex[..8]).fg(Color::Cyan),
                                Cell::new(format!("{score:.3}")).fg(Color::Yellow),
                                Cell::new(&author_str).fg(Color::Magenta),
                                Cell::new(&change.intent),
                            ]);
                        }
                        println!("{table}");
                    }
                } else {
                    let changes = if let Some(query) = revset.as_deref() {
                        repo.log_revset(query)?
                    } else {
                        repo.log_smartlog()?
                    };
                    let parsed_template = if let Some(raw_template) = template.as_deref() {
                        let config = load_merged_config(std::path::Path::new("."))?;
                        let resolved_template = resolve_log_template_alias(raw_template, &config);
                        Some(
                            LogTemplate::parse(&resolved_template).map_err(|msg| {
                                anyhow::anyhow!("invalid --template value: {msg}")
                            })?,
                        )
                    } else {
                        None
                    };
                    if changes.is_empty() {
                        println!("No changes yet. Use 'arc snap' to create your first change.");
                    } else {
                        let current = repo.resolve_revset_symbol_typed("@")?;
                        let current_view = repo.current_view_name()?;
                        let active_heads: BTreeSet<ChangeId> =
                            View::load(&repo.shared_root, &current_view)
                                .map(|view| view.heads.into_iter().map(ChangeId::from).collect())
                                .unwrap_or_default();

                        let mut stable_anchor_heads: HashSet<Blake3Hash> = HashSet::new();
                        for function_name in ["remote_branches", "tags", "bookmarks"] {
                            let heads = repo.resolve_revset_reference_heads(function_name)?;
                            stable_anchor_heads.extend(heads.into_iter().map(Blake3Hash::from));
                        }
                        if !stable_anchor_heads.is_empty() {
                            repo.hydrate_heads(&stable_anchor_heads)?;
                        }
                        let stable_ancestors: BTreeSet<ChangeId> = if stable_anchor_heads.is_empty()
                        {
                            BTreeSet::new()
                        } else {
                            repo.graph
                                .load()
                                .ancestors(&stable_anchor_heads)
                                .into_iter()
                                .map(ChangeId::from)
                                .collect()
                        };

                        let decorations = GraphDecorations {
                            tags: repo.tag_decorations()?,
                            remotes: repo.remote_branch_decorations()?,
                            current,
                            active_heads,
                            stable_ancestors,
                        };
                        let renderer = GraphRenderer::new();
                        for line in renderer.render_with_decorations_and_template(
                            &changes,
                            &decorations,
                            parsed_template.as_ref(),
                        ) {
                            println!("{line}");
                        }
                    }
                }
            }
            Command::Status => {
                let mut repo = Repository::open(".")?;
                let (author, signing_key) =
                    load_identity_with_ephemeral_fallback(&repo.shared_root)?;
                repo.set_identity(author, signing_key);
                let _ = repo.snapshot()?;
                let view_name = repo.current_view_name()?;
                println!("On view: {}", view_name.cyan().bold());
                let atoms = repo.status()?;
                if atoms.is_empty() {
                    println!("Nothing to snap — working directory is clean.");
                } else {
                    println!("Uncommitted changes:");
                    for atom in &atoms {
                        println!("  {}", atom_display_label(atom));
                    }
                }
            }
            Command::CherryPick { hash } => {
                let hash_bytes = hex_to_hash(&hash)?;
                let mut repo = Repository::open(".")?;
                let (author, signing_key) = load_identity()?;
                repo.set_identity(author, signing_key);
                repo.cherry_pick(&hash_bytes)?;
                let hex: String = hash_bytes.iter().map(|b| format!("{b:02x}")).collect();
                println!("Cherry-picked {} into current view.", &hex[..8]);
            }
            Command::Blame { filepath } => {
                let mut repo = Repository::open(".")?;
                let (author, signing_key) = load_identity()?;
                repo.set_identity(author, signing_key);
                let entries = repo.blame(&filepath)?;
                if entries.is_empty() {
                    println!("No blame data for '{filepath}'");
                } else {
                    println!("{:<40} {:<10} {:<35} Intent", "Node", "Hash", "Author");
                    println!("{}", "-".repeat(100));
                    for (path, change) in &entries {
                        // Use only the node-level suffix (strip the ["file","path"] prefix).
                        let node = path[2..].join("/");
                        let hash_hex: String =
                            change.id.iter().map(|b| format!("{b:02x}")).collect();
                        let short_hash = &hash_hex[..8];
                        let author_str = match &change.author {
                            Author::Human { name, email, .. } => {
                                format!("{name} <{email}>")
                            }
                            Author::AI { model, human_sponsor } => {
                                let sponsor: String =
                                    human_sponsor.iter().map(|b| format!("{b:02x}")).collect();
                                format!("{model} | sponsor:{}", &sponsor[..8])
                            }
                            Author::Server { canonical_id, .. } => {
                                format!("{canonical_id} [server]")
                            }
                            Author::Transient { session_id, .. } => {
                                format!("{session_id} [transient]")
                            }
                        };
                        let sig_status =
                            if change.verify_signature() { "[verified]" } else { "[UNVERIFIED]" };
                        println!(
                            "{:<40} {:<10} {:<35} {} {}",
                            node, short_hash, author_str, sig_status, change.intent
                        );
                    }
                }
            }
            Command::Stash { action } => match action {
                StashAction::Push => {
                    let mut repo = Repository::open(".")?;
                    let (author, signing_key) = load_identity()?;
                    repo.set_identity(author, signing_key);
                    let name = repo.stash()?;
                    println!("Saved working directory state to '{name}'");
                }
                StashAction::Pop => {
                    let mut repo = Repository::open(".")?;
                    let (author, signing_key) = load_identity()?;
                    repo.set_identity(author, signing_key);
                    let list = repo.stash_list()?;
                    let name =
                        list.last().cloned().ok_or_else(|| anyhow::anyhow!("no stash found"))?;
                    repo.stash_pop()?;
                    println!("Applied and dropped stash '{name}'");
                }
                StashAction::List => {
                    let repo = Repository::open(".")?;
                    let stashes = repo.stash_list()?;
                    if stashes.is_empty() {
                        println!("No stashes.");
                    } else {
                        for s in &stashes {
                            println!("{s}");
                        }
                    }
                }
            },
            Command::View { action } => match action {
                ViewAction::Create { name } => {
                    let repo = Repository::open(".")?;
                    repo.create_view(&name)?;
                    println!("Created view '{name}'");
                }
                ViewAction::Switch { name } => {
                    let mut repo = Repository::open(".")?;
                    repo.switch_view(&name)?;
                    println!("Switched to view '{name}'");
                }
                ViewAction::Merge { name } => {
                    let mut repo = Repository::open(".")?;
                    repo.merge_view(&name)?;
                    println!("Merged view '{name}' into current view");
                }
            },
            Command::Ai { action } => match action {
                AiAction::Resolve => {
                    let mut repo = Repository::open(".")?;
                    let (author, signing_key) = load_identity()?;
                    repo.set_identity(author, signing_key);

                    match repo.resolve_last_policy_error_with_ai() {
                        Ok(true) => {
                            println!(
                                "[arc] Policy mismatch converted into a Lensed Ghost Node. Review \
                                 changes then run 'arc ai approve <hash>'."
                            );
                            return Ok(());
                        }
                        Ok(false) => {}
                        Err(err) => return Err(err),
                    }

                    let cfg = load_merged_config(std::path::Path::new("."))?;
                    let merge_tool_resolution = if let Some(tool) = cfg.merge.tool.as_deref() {
                        eprintln!("[arc] Using merge tool '{}' for conflict resolution.", tool);
                        Some(repo.resolve_conflict_with_merge_tool(Some(tool)))
                    } else {
                        None
                    };

                    if let Some(result) = merge_tool_resolution {
                        match result {
                            Ok(()) => {
                                println!(
                                    "[arc] Resolution staged as Ghost Node. Review changes then \
                                     run 'arc ai approve'."
                                );
                            }
                            Err(err) => {
                                let err_text = format!("{err:#}");
                                let should_fallback = err_text
                                    .contains("failed to execute merge tool")
                                    || err_text.contains("exited with status")
                                    || err_text.contains("produced empty output");
                                if !should_fallback {
                                    return Err(err);
                                }

                                eprintln!(
                                    "[arc] Merge-tool resolution failed ({err_text}). Falling \
                                     back to AI provider."
                                );
                                let provider_name =
                                    cfg.ai.provider.as_deref().unwrap_or("openai-compatible");
                                let model = cfg
                                    .ai
                                    .model
                                    .clone()
                                    .unwrap_or_else(|| "gpt-4o-mini".to_string());
                                let endpoint = cfg.ai.endpoint.clone();
                                let api_key = std::env::var("ARC_AI_API_KEY").map_err(|_| {
                                    anyhow::anyhow!(
                                        "ARC_AI_API_KEY is required for 'arc ai resolve' and is \
                                         read only at runtime"
                                    )
                                })?;

                                let provider =
                                    build_provider(provider_name, &model, endpoint, api_key)?;
                                eprintln!(
                                    "[arc] Using AI provider '{}' with model '{}'.",
                                    provider_name, model
                                );

                                let rt = tokio::runtime::Builder::new_current_thread()
                                    .enable_all()
                                    .build()?;
                                rt.block_on(
                                    repo.resolve_conflict_with_provider(provider.as_ref(), &model),
                                )?;

                                println!(
                                    "[arc] Resolution staged as Ghost Node. Review changes then \
                                     run 'arc ai approve'."
                                );
                            }
                        }
                    } else {
                        let provider_name =
                            cfg.ai.provider.as_deref().unwrap_or("openai-compatible");
                        let model =
                            cfg.ai.model.clone().unwrap_or_else(|| "gpt-4o-mini".to_string());
                        let endpoint = cfg.ai.endpoint.clone();
                        let api_key = std::env::var("ARC_AI_API_KEY").map_err(|_| {
                            anyhow::anyhow!(
                                "ARC_AI_API_KEY is required for 'arc ai resolve' and is read only \
                                 at runtime"
                            )
                        })?;

                        let provider = build_provider(provider_name, &model, endpoint, api_key)?;
                        eprintln!(
                            "[arc] Using AI provider '{}' with model '{}'.",
                            provider_name, model
                        );

                        let rt =
                            tokio::runtime::Builder::new_current_thread().enable_all().build()?;
                        rt.block_on(
                            repo.resolve_conflict_with_provider(provider.as_ref(), &model),
                        )?;

                        println!(
                            "[arc] Resolution staged as Ghost Node. Review changes then run 'arc \
                             ai approve'."
                        );
                    }
                }
                AiAction::Approve { hash } => {
                    let mut repo = Repository::open(".")?;
                    let (author, signing_key) = load_identity()?;
                    let id =
                        repo.approve_pending_ai_with_hash(&author, &signing_key, hash.as_deref())?;
                    let hex: String = id.iter().map(|b| format!("{b:02x}")).collect();
                    println!("Approved and committed AI change → {hex}");
                }
                AiAction::Generate { goal, file } => {
                    let mut repo = Repository::open(".")?;
                    arc_cli::generate::run(&goal, file.as_deref(), &mut repo)?;
                }
            },
            Command::Import { source } => match source {
                ImportSource::Git { git_path } => {
                    let mut repo = match Repository::open(".") {
                        Ok(r) => r,
                        Err(_) => Repository::init(".")?,
                    };
                    let (author, signing_key) = load_identity()?;
                    import_repo(&git_path, &mut repo, &author, &signing_key)?;
                    println!("Imported Git history from {git_path}");
                }
            },
            Command::Tour => {
                arc_cli::commands::tour::run_tour()?;
            }
            Command::Fetch { remote_path, view } => {
                let mut repo = Repository::open(".")?;
                let heads = fetch(&mut repo, &remote_path, &view)?;
                println!("Fetched {} head(s) from {remote_path}", heads.len());
            }
            Command::Pull { remote_path, view } => {
                let mut repo = Repository::open(".")?;
                pull(&mut repo, &remote_path, &view)?;
                println!("Pulled and merged view '{view}' from {remote_path}");
            }
            Command::Verify { tooling, governance, workspace_policy } => {
                let mut repo = Repository::open(".")?;
                let name = repo.current_view_name()?;
                repo.hydrate(&name)?;
                repo.verify_graph()?;
                println!("Graph cryptographic provenance verified.");

                let frontier = View::load(&repo.shared_root, &name)?
                    .heads
                    .into_iter()
                    .map(arc_store_types::newtypes::ChangeId::from)
                    .collect::<Vec<_>>();
                let snapshots = list_snapshot_ids(&repo.shared_root)?;

                if tooling {
                    let report = audit_workspace_tooling(
                        &repo.shared_root,
                        frontier.clone(),
                        snapshots.clone(),
                    )?;
                    println!(
                        "Tooling policy verified: {} codespell rules, {} required mise tasks, \
                         nextest default timeout {}, ci terminate-after {}.",
                        report.codespell_rules,
                        report.present_required_tasks.len(),
                        report.default_slow_timeout_period,
                        report
                            .ci_terminate_after
                            .map_or_else(|| "none".to_string(), |v| v.to_string())
                    );
                    println!(
                        "Typed evidence: {} frontier ChangeId(s), {} SnapshotId(s).",
                        report.frontier.len(),
                        report.synthesis_snapshots.len()
                    );
                }

                if governance {
                    let report = audit_github_governance(
                        &repo.shared_root,
                        frontier.clone(),
                        snapshots.clone(),
                    )?;
                    println!(
                        "Governance policy verified: {} required workflows, {} pinned action \
                         reference(s), dependabot ecosystems [{}].",
                        report.required_workflows.len(),
                        report.pinned_action_references,
                        report.dependabot_ecosystems.join(", ")
                    );
                    println!(
                        "Typed evidence: {} frontier ChangeId(s), {} SnapshotId(s).",
                        report.frontier.len(),
                        report.synthesis_snapshots.len()
                    );
                }

                if workspace_policy {
                    let report = audit_workspace_policy(
                        &repo.shared_root,
                        frontier.clone(),
                        snapshots.clone(),
                    )?;
                    println!(
                        "Workspace policy verified: {} policy files, {} gitignore patterns.",
                        report.policy_files.len(),
                        report.validated_gitignore_patterns
                    );
                    println!(
                        "Typed evidence: {} frontier ChangeId(s), {} SnapshotId(s).",
                        report.frontier.len(),
                        report.synthesis_snapshots.len()
                    );
                }
            }
            Command::Auth { action } => match action {
                AuthAction::Login { name, email } => {
                    save_identity(&name, &email)?;
                    println!("Identity saved. Run 'arc auth whoami' to confirm.");
                }
                AuthAction::Whoami => {
                    let (author, _) = load_identity()?;
                    match author {
                        Author::Human { name, email, key } => {
                            let hex: String = key.iter().map(|b| format!("{b:02x}")).collect();
                            println!("Name:   {name}");
                            println!("Email:  {email}");
                            println!("Key:    {hex}");
                        }
                        Author::AI { model, human_sponsor } => {
                            let hex: String =
                                human_sponsor.iter().map(|b| format!("{b:02x}")).collect();
                            println!("Model:          {model}");
                            println!("Human sponsor:  {hex}");
                        }
                        Author::Server { canonical_id, key } => {
                            let hex: String = key.iter().map(|b| format!("{b:02x}")).collect();
                            println!("Server ID: {canonical_id}");
                            println!("Key:       {hex}");
                        }
                        Author::Transient { session_id, key } => {
                            let hex: String = key.iter().map(|b| format!("{b:02x}")).collect();
                            println!("Session ID: {session_id} [transient]");
                            println!("Key:        {hex}");
                        }
                    }
                }
            },
            Command::Watch { path } => {
                let watch_path = match path {
                    Some(path) => path,
                    None => std::env::current_dir()?,
                };
                let rt = tokio::runtime::Runtime::new()?;
                rt.block_on(arc_daemon::AutoSnapDaemon::start(watch_path, 400))?;
            }
            Command::Serve { port } => {
                let repo = Repository::open(".")?;
                let rt = tokio::runtime::Runtime::new()?;
                rt.block_on(arc_net::sync::server::serve(port, repo.shared_root.clone()))?;
            }
            Command::Sync { address } => {
                let pipeline = Progress::sync_pipeline();
                let timer = SloTimer::from_env("arc.sync");

                pipeline.start_stage(PipelineStage::Discover);
                let mut repo = match timer.stage(OperationStage::Discover, || Repository::open("."))
                {
                    Ok(repo) => {
                        pipeline.finish_stage(PipelineStage::Discover);
                        repo
                    }
                    Err(err) => {
                        pipeline
                            .fail_stage(PipelineStage::Discover, "Discovering graph delta failed");
                        return Err(err);
                    }
                };

                pipeline.start_stage(PipelineStage::Negotiate);
                let token = match timer.stage(OperationStage::Negotiate, || {
                    let token = resolve_sync_token()?;
                    Ok::<_, anyhow::Error>(token)
                }) {
                    Ok(token) => {
                        pipeline.finish_stage(PipelineStage::Negotiate);
                        token
                    }
                    Err(err) => {
                        pipeline
                            .fail_stage(PipelineStage::Negotiate, "Negotiating with remote failed");
                        return Err(err);
                    }
                };

                pipeline.start_stage(PipelineStage::Transfer);
                let imported_count = match timer.stage(OperationStage::Transfer, || {
                    repo.sync_native_with_semantic_gate(&address, token)
                }) {
                    Ok(count) => {
                        pipeline.finish_stage(PipelineStage::Transfer);
                        count
                    }
                    Err(err) => {
                        pipeline.fail_stage(PipelineStage::Transfer, "Transferring blobs failed");
                        return Err(err);
                    }
                };

                pipeline.start_stage(PipelineStage::Materialize);
                timer.stage(OperationStage::Materialize, || imported_count);
                pipeline.finish_stage(PipelineStage::Materialize);

                pipeline.start_stage(PipelineStage::Finalize);
                timer.stage(OperationStage::Finalize, || {
                    println!(
                        "[arc] Native sync completed. Imported {} verified change block(s).",
                        imported_count
                    );
                });
                pipeline.finish_stage(PipelineStage::Finalize);
                timer.finish();
            }
            Command::Remote { action } => match action {
                RemoteAction::Add { name, url } => {
                    let repo = Repository::open(".")?;
                    repo.add_remote(&name, &url)?;
                    println!("Remote '{name}' \u{2192} {url}");
                }
                RemoteAction::List => {
                    let repo = Repository::open(".")?;
                    let remotes = repo.list_remotes()?;
                    if remotes.is_empty() {
                        println!("No remotes configured.");
                    } else {
                        let mut pairs: Vec<_> = remotes.iter().collect();
                        pairs.sort_by_key(|(k, _)| k.as_str());
                        for (name, url) in pairs {
                            println!("{name}\t{url}");
                        }
                    }
                }
                RemoteAction::Remove { name } => {
                    let repo = Repository::open(".")?;
                    repo.remove_remote(&name)?;
                    println!("Remote '{}' removed.", name.cyan().bold());
                }
            },
            Command::Tag { name, hash } => {
                let mut repo = Repository::open(".")?;
                let (author, signing_key) = load_identity()?;
                repo.set_identity(author, signing_key);
                let hash_bytes = repo.resolve_rev(&hash)?;
                repo.create_tag(&name, &hash_bytes)?;
                let hex: String = hash_bytes.iter().map(|b| format!("{b:02x}")).collect();
                println!("Tagged {} as '{name}'", &hex[..8]);
            }
            Command::Tags => {
                let repo = Repository::open(".")?;
                let tags = repo.list_tags()?;
                if tags.is_empty() {
                    println!("No tags.");
                } else {
                    for t in &tags {
                        let h: String = t.target.iter().map(|b| format!("{b:02x}")).collect();
                        let sig = if t.verify() { "[verified]" } else { "[UNVERIFIED]" };
                        println!("{} {} {sig}", t.name, &h[..8]);
                    }
                }
            }
            Command::TagSet { allow_move, rev, names } => {
                let repo = Repository::open(".")?;
                let (author, signing_key) = load_identity()?;
                let mut writable = Repository::open(".")?;
                writable.set_identity(author, signing_key);
                let target = repo.resolve_rev(&rev)?;

                let existing = repo
                    .list_tags()?
                    .into_iter()
                    .map(|t| (t.name, t.target))
                    .collect::<std::collections::HashMap<_, _>>();
                let plan = plan_tag_set_updates(existing, target, names);

                for update in &plan.updates {
                    if update.needs_write {
                        writable.set_tag(&update.name, &target, allow_move)?;
                    }
                }
                if plan.created > 0 {
                    println!("Created {} tag(s).", plan.created);
                }
                if plan.moved > 0 {
                    println!("Moved {} tag(s).", plan.moved);
                }
                if plan.created == 0 && plan.moved == 0 {
                    println!("Nothing changed.");
                }
            }
            Command::TagDelete { names } => {
                let repo = Repository::open(".")?;
                let deleted = repo.delete_tags_matching(&names)?;
                if deleted.is_empty() {
                    println!("No tags to delete.");
                } else {
                    println!("Deleted {} tag(s): {}", deleted.len(), deleted.join(", "));
                }
            }
            Command::TagList { names } => {
                let repo = Repository::open(".")?;
                let tags = repo.list_tags_matching(&names)?;
                if tags.is_empty() {
                    println!("No tags.");
                } else {
                    for t in &tags {
                        let h: String = t.target.iter().map(|b| format!("{b:02x}")).collect();
                        let sig = if t.verify() { "[verified]" } else { "[UNVERIFIED]" };
                        println!("{} {} {sig}", t.name, &h[..8]);
                    }
                }
            }
            Command::Bookmark { action } => match action {
                BookmarkAction::Create { name, rev } => {
                    let mut repo = Repository::open(".")?;
                    let target = repo.resolve_rev(&rev)?;
                    repo.create_bookmark(&name, &target)?;
                    let hex: String = target.iter().map(|b| format!("{b:02x}")).collect();
                    println!("Created bookmark '{name}' at {}", &hex[..8]);
                }
                BookmarkAction::Set { name, rev } => {
                    let mut repo = Repository::open(".")?;
                    let target = repo.resolve_rev(&rev)?;
                    repo.set_bookmark(&name, &target)?;
                    let hex: String = target.iter().map(|b| format!("{b:02x}")).collect();
                    println!("Set bookmark '{name}' to {}", &hex[..8]);
                }
                BookmarkAction::Move { name, rev, allow_backwards } => {
                    let mut repo = Repository::open(".")?;
                    let target = repo.resolve_rev(&rev)?;
                    repo.move_bookmark(&name, &target, allow_backwards)?;
                    let hex: String = target.iter().map(|b| format!("{b:02x}")).collect();
                    println!("Moved bookmark '{name}' to {}", &hex[..8]);
                }
                BookmarkAction::Delete { name } => {
                    let repo = Repository::open(".")?;
                    repo.delete_bookmark(&name)?;
                    println!("Deleted bookmark '{name}'");
                }
                BookmarkAction::List => {
                    let repo = Repository::open(".")?;
                    let decorations = repo.bookmark_decorations()?;
                    if decorations.is_empty() {
                        println!("No bookmarks.");
                    } else {
                        for (id, names) in decorations {
                            let hex = id.to_hex();
                            println!("{}\t{}", &hex[..8], names.join(", "));
                        }
                    }
                }
            },
            Command::Revert { hash } => {
                let mut repo = Repository::open(".")?;
                let (author, signing_key) = load_identity()?;
                repo.set_identity(author, signing_key);
                let hash_bytes = repo.resolve_rev(&hash)?;
                let target_hex: String = hash_bytes.iter().map(|b| format!("{b:02x}")).collect();
                let revert_id = repo.revert(&hash_bytes)?;
                let revert_hex: String = revert_id.iter().map(|b| format!("{b:02x}")).collect();
                println!("Reverted {} \u{2192} new change {}", &target_hex[..8], &revert_hex[..8]);
            }
            Command::Restore { filepath } => {
                let mut repo = Repository::open(".")?;
                let (author, signing_key) = load_identity()?;
                repo.set_identity(author, signing_key);
                repo.restore(&filepath)?;
                println!("Restored '{filepath}' to its snapped state.");
            }
            Command::Info => {
                let repo = Repository::open(".")?;
                repo.info()?;
            }
            Command::Checkout { name } => {
                let mut repo = Repository::open(".")?;
                repo.switch_view(&name)?;
                println!("Switched to view '{name}'");
            }
            Command::Branch { name } => match name {
                Some(n) => {
                    let repo = Repository::open(".")?;
                    repo.create_view(&n)?;
                    println!("Created view '{n}'");
                }
                None => {
                    let repo = Repository::open(".")?;
                    let current = repo.current_view_name()?;
                    let views = repo.list_views()?;
                    for v in &views {
                        if v == &current {
                            println!("* {v}");
                        } else {
                            println!("  {v}");
                        }
                    }
                }
            },
            Command::Commit => {
                println!(
                    "Hint: arc uses 'snap' instead of 'commit'. Try: arc snap -m \"<message>\""
                );
            }
            Command::Abandon { revisions } => {
                let mut repo = Repository::open(".")?;
                let abandoned = repo.abandon_heads(&revisions)?;
                if abandoned.is_empty() {
                    println!("No heads were abandoned.");
                } else {
                    println!("Abandoned {} head(s).", abandoned.len());
                }
            }
            Command::Describe { message, revision } => {
                anyhow::ensure!(
                    revision == "@" || revision.eq_ignore_ascii_case("head"),
                    "describe currently supports only '@' / 'HEAD'"
                );
                let mut repo = Repository::open(".")?;
                let (author, signing_key) =
                    load_identity_with_ephemeral_fallback(&repo.shared_root)?;
                repo.set_identity(author, signing_key);
                let id = repo.amend(Some(&message))?;
                let hex: String = id.iter().map(|b| format!("{b:02x}")).collect();
                println!("Described {}", &hex[..8]);
            }
            Command::Undo => {
                let mut repo = Repository::open(".")?;
                match repo.undo()? {
                    Some(op) => {
                        let before = op.before_short();
                        let after = op.after_short();
                        println!(
                            "{} Undid {} on view {}. Restored: {} \u{2192} {}",
                            "⏪".cyan(),
                            format!("'{}'", op.command).green(),
                            format!("'{}'", op.view).yellow(),
                            after.dimmed(),
                            before.dimmed(),
                        );
                    }
                    None => println!("Nothing to undo — operation log is empty."),
                }
            }
            Command::Redo => {
                let mut repo = Repository::open(".")?;
                match repo.redo()? {
                    Some(op) => {
                        let before = op.before_short();
                        let after = op.after_short();
                        println!(
                            "{} Redid {} on view {}. Restored: {} → {}",
                            "⏩".cyan(),
                            format!("'{}'", op.command).green(),
                            format!("'{}'", op.view).yellow(),
                            before.dimmed(),
                            after.dimmed(),
                        );
                    }
                    None => println!("Nothing to redo."),
                }
            }
            Command::Root => {
                let repo = Repository::open(".")?;
                let root = repo.workspace_root(None)?;
                println!("{}", root.display());
            }
            Command::Version => {
                print!("{}", Cli::command().render_version());
            }
            Command::Bisect { action } => {
                let mut repo = Repository::open(".")?;
                match action {
                    BisectAction::Start { range, find_good } => {
                        let state = repo.bisect_start(&range, find_good)?;
                        println!(
                            "Started bisect: {} candidates ({} untested)",
                            state.candidates.len(),
                            state.untested_count()
                        );
                        match state.current {
                            Some(id) => println!("Next test revision: {}", short_change_id(id)),
                            None => {
                                println!("Bisect converged immediately; no untested revisions.")
                            }
                        }
                    }
                    BisectAction::Next => {
                        let state = repo.bisect_next()?;
                        match state.current {
                            Some(id) => println!("Next test revision: {}", short_change_id(id)),
                            None => println!("Bisect complete: no untested revisions remain."),
                        }
                    }
                    BisectAction::Good => {
                        let state = repo.bisect_mark_good()?;
                        let (good_count, bad_count) = display_bisect_counts(&state);
                        println!(
                            "Marked good. good={} bad={} untested={}",
                            good_count,
                            bad_count,
                            state.untested_count()
                        );
                        match state.current {
                            Some(id) => println!("Next test revision: {}", short_change_id(id)),
                            None => println!("Bisect complete: no untested revisions remain."),
                        }
                    }
                    BisectAction::Bad => {
                        let state = repo.bisect_mark_bad()?;
                        let (good_count, bad_count) = display_bisect_counts(&state);
                        println!(
                            "Marked bad. good={} bad={} untested={}",
                            good_count,
                            bad_count,
                            state.untested_count()
                        );
                        match state.current {
                            Some(id) => println!("Next test revision: {}", short_change_id(id)),
                            None => println!("Bisect complete: no untested revisions remain."),
                        }
                    }
                    BisectAction::Status => match repo.bisect_status()? {
                        Some(state) => {
                            let (good_count, bad_count) = display_bisect_counts(&state);
                            println!(
                                "Bisect status: range='{}' good={} bad={} untested={}",
                                state.range_expr,
                                good_count,
                                bad_count,
                                state.untested_count()
                            );
                            match state.current {
                                Some(id) => println!("Current revision: {}", short_change_id(id)),
                                None => println!("Current revision: <none>"),
                            }
                        }
                        None => println!("No active bisect session."),
                    },
                    BisectAction::Reset => {
                        repo.bisect_reset()?;
                        println!("Bisect session reset.");
                    }
                }
            }
            Command::Bench { action } => {
                let mut repo = Repository::open(".")?;
                match action {
                    BenchAction::CommonAncestors { left, right, iterations } => {
                        let (total_nanos, last_len) =
                            repo.bench_common_ancestors(&left, &right, iterations)?;
                        println!(
                            "bench common-ancestors: iterations={} avg={}ns result_count={}",
                            iterations.max(1),
                            total_nanos / u128::from(iterations.max(1)),
                            last_len
                        );
                    }
                    BenchAction::IsAncestor { ancestor, descendant, iterations } => {
                        let (total_nanos, result) =
                            repo.bench_is_ancestor(&ancestor, &descendant, iterations)?;
                        println!(
                            "bench is-ancestor: iterations={} avg={}ns result={}",
                            iterations.max(1),
                            total_nanos / u128::from(iterations.max(1)),
                            result
                        );
                    }
                    BenchAction::ResolvePrefix { prefix, iterations } => {
                        let (total_nanos, hits) = repo.bench_resolve_prefix(&prefix, iterations)?;
                        println!(
                            "bench resolve-prefix: iterations={} avg={}ns hits={}",
                            iterations.max(1),
                            total_nanos / u128::from(iterations.max(1)),
                            hits
                        );
                    }
                    BenchAction::Revset { revset, iterations } => {
                        let (total_nanos, count) = repo.bench_revset(&revset, iterations)?;
                        println!(
                            "bench revset: iterations={} avg={}ns count={}",
                            iterations.max(1),
                            total_nanos / u128::from(iterations.max(1)),
                            count
                        );
                    }
                }
            }
            Command::Op { action } => match action {
                OpAction::Log => {
                    let repo = Repository::open(".")?;
                    let ops = repo.op_log()?;
                    if ops.is_empty() {
                        println!("Operation log is empty.");
                    } else {
                        let mut table = Table::new();
                        table.load_preset(presets::UTF8_FULL);
                        table.set_header(vec![
                            Cell::new("ID").fg(Color::Cyan),
                            Cell::new("Time"),
                            Cell::new("View").fg(Color::Yellow),
                            Cell::new("Agent"),
                            Cell::new("Command").fg(Color::Green),
                            Cell::new("Before→After"),
                        ]);
                        for op in &ops {
                            let display_id = op
                                .snapshot
                                .map(|snapshot| snapshot.to_hex()[..12].to_string())
                                .unwrap_or_else(|| op.id.clone());
                            let agent_label = match op.agent {
                                OperationAgent::Human => op.agent.label().to_string(),
                                OperationAgent::Ai => op.agent.label().cyan().to_string(),
                            };
                            table.add_row(vec![
                                Cell::new(display_id).fg(Color::Cyan),
                                Cell::new(op.formatted_time()),
                                Cell::new(&op.view).fg(Color::Yellow),
                                Cell::new(agent_label),
                                Cell::new(&op.command).fg(Color::Green),
                                Cell::new(format!("{} → {}", op.before_short(), op.after_short())),
                            ]);
                        }
                        println!("{table}");
                    }
                }
                OpAction::Restore { op_id } => {
                    let mut repo = Repository::open(".")?;
                    let target = repo.op_restore(&op_id)?;
                    println!(
                        "Restored operation {} on view '{}': {} → {}",
                        op_id.cyan(),
                        target.view.yellow(),
                        target.before_short(),
                        target.after_short()
                    );
                }
                OpAction::Revert { op_id } => {
                    let mut repo = Repository::open(".")?;
                    let target = repo.op_revert(&op_id)?;
                    println!(
                        "Reverted operation {} on view '{}': {} → {}",
                        op_id.cyan(),
                        target.view.yellow(),
                        target.after_short(),
                        target.before_short()
                    );
                }
            },
            Command::Sparse { action } => match action {
                SparseAction::Set { paths, add, remove, clear } => {
                    let mut repo = Repository::open(".")?;
                    anyhow::ensure!(
                        !paths.is_empty() || clear || !add.is_empty() || !remove.is_empty(),
                        "sparse set requires either PATHS, --add/--remove, or --clear"
                    );

                    let replace_mode = !paths.is_empty();
                    let mut next = if replace_mode {
                        anyhow::ensure!(
                            add.is_empty() && remove.is_empty() && !clear,
                            "PATHS cannot be combined with --add/--remove/--clear"
                        );
                        paths
                    } else if clear {
                        Vec::new()
                    } else {
                        repo.read_sparse_patterns()
                    };

                    if !replace_mode {
                        let mut set: std::collections::BTreeSet<String> =
                            next.into_iter().collect();
                        for path in remove {
                            set.remove(&path);
                        }
                        for path in add {
                            set.insert(path);
                        }
                        next = set.into_iter().collect();
                    }

                    repo.apply_sparse(&next)?;
                    println!("Sparse cone updated ({} pattern(s)).", next.len());
                }
                SparseAction::Edit => {
                    let mut repo = Repository::open(".")?;
                    let current = repo.read_sparse_patterns();
                    let edited = edit_lines_in_editor("sparse patterns", ".arcsparse", &current)?;
                    repo.apply_sparse(&edited)?;
                    println!("Sparse cone updated ({} pattern(s)).", edited.len());
                }
                SparseAction::List => {
                    let repo = Repository::open(".")?;
                    let patterns = repo.read_sparse_patterns();
                    if patterns.is_empty() {
                        println!("Full checkout — no sparse patterns active.");
                    } else {
                        for p in &patterns {
                            println!("{p}");
                        }
                    }
                }
                SparseAction::Reset => {
                    let mut repo = Repository::open(".")?;
                    repo.apply_sparse(&[])?;
                    println!("Sparse filter cleared — working directory fully restored.");
                }
            },
            Command::Blob { action } => match action {
                BlobAction::Track { extension } => {
                    let repo = Repository::open(".")?;
                    let mut config = load_merged_config(&repo.shared_root)?;
                    let normalized = normalize_blob_extension(&extension)?;

                    let mut extensions = config.snapshot.blob_extensions.unwrap_or_default();
                    if extensions.iter().any(|ext| ext == &normalized) {
                        println!("Extension '.{normalized}' is already tracked.");
                    } else {
                        extensions.push(normalized.clone());
                        extensions.sort();
                        extensions.dedup();
                        config.snapshot.blob_extensions = Some(extensions);
                        save_local_config(&config, &repo.shared_root)?;
                        println!("Tracked blob extension '.{normalized}'.");
                    }
                }
                BlobAction::Ls => {
                    let repo = Repository::open(".")?;
                    let config = load_merged_config(&repo.shared_root)?;
                    let mut extensions = config.snapshot.blob_extensions.unwrap_or_default();
                    if extensions.is_empty() {
                        println!("No blob extensions tracked.");
                    } else {
                        extensions.sort();
                        extensions.dedup();
                        for ext in &extensions {
                            println!(".{ext}");
                        }
                    }
                }
            },
            Command::Mount { action } => match action {
                MountAction::Add { coordinate, path } => {
                    let mut repo = Repository::open(".")?;
                    let (author, signing_key) = load_identity()?;
                    repo.set_identity(author, signing_key);
                    let coord = SpacetimeCoordinate::from_uri(&coordinate).map_err(|e| {
                        anyhow::anyhow!(
                            "invalid coordinate '{coordinate}': {e} (expected \
                             arc://<namespace>/<repo>@<hash>)"
                        )
                    })?;
                    let id = repo.mount_add(&path, coord)?;
                    repo.refresh_projection()?;
                    let hex: String = id.iter().map(|b| format!("{b:02x}")).collect();
                    println!("Mounted '{path}' → {coordinate}  (change {})", &hex[..8]);
                }
                MountAction::Sync => {
                    let mut repo = Repository::open(".")?;
                    repo.mount_sync()?;
                }
            },
            Command::Workspace { action } => match action {
                WorkspaceAction::Add { path, view } => {
                    let mut repo = Repository::open(".")?;
                    repo.workspace_add(std::path::Path::new(&path), view.as_deref())?;
                    println!("Created workspace at '{path}'");
                }
                WorkspaceAction::List => {
                    let repo = Repository::open(".")?;
                    let workspaces = repo.workspace_list()?;
                    if workspaces.is_empty() {
                        println!("No linked workspaces.");
                    } else {
                        for ws in &workspaces {
                            println!("{}", ws.display());
                        }
                    }
                }
                WorkspaceAction::Forget { path } => {
                    let repo = Repository::open(".")?;
                    repo.workspace_forget(std::path::Path::new(&path))?;
                    println!("Forgot workspace at '{path}'");
                }
                WorkspaceAction::Rename { old_path, new_path } => {
                    let repo = Repository::open(".")?;
                    repo.workspace_rename(
                        std::path::Path::new(&old_path),
                        std::path::Path::new(&new_path),
                    )?;
                    println!("Renamed workspace '{}' -> '{}'", old_path, new_path);
                }
                WorkspaceAction::Root { path } => {
                    let repo = Repository::open(".")?;
                    let root = repo.workspace_root(path.as_deref().map(std::path::Path::new))?;
                    println!("{}", root.display());
                }
                WorkspaceAction::UpdateStale => {
                    let mut repo = Repository::open(".")?;
                    let (author, signing_key) =
                        load_identity_with_ephemeral_fallback(&repo.shared_root)?;
                    repo.set_identity(author, signing_key);
                    let changed = repo.snapshot()?;
                    if changed {
                        println!("Snapshot complete.");
                    } else {
                        println!("No snapshot needed.");
                    }
                }
            },
            Command::Util { action } => match action {
                UtilAction::Completion { shell } => {
                    use clap_complete::{Shell, generate};
                    use clap_complete_nushell::Nushell;

                    let mut cmd = Cli::command();
                    let mut buf = Vec::new();
                    let bin_name = "arc";
                    match shell {
                        ShellCompletion::Bash => {
                            generate(Shell::Bash, &mut cmd, bin_name, &mut buf)
                        }
                        ShellCompletion::Elvish => {
                            generate(Shell::Elvish, &mut cmd, bin_name, &mut buf)
                        }
                        ShellCompletion::Fish => {
                            generate(Shell::Fish, &mut cmd, bin_name, &mut buf)
                        }
                        ShellCompletion::Nushell => generate(Nushell, &mut cmd, bin_name, &mut buf),
                        ShellCompletion::PowerShell => {
                            generate(Shell::PowerShell, &mut cmd, bin_name, &mut buf)
                        }
                        ShellCompletion::Zsh => generate(Shell::Zsh, &mut cmd, bin_name, &mut buf),
                    }
                    std::io::Write::write_all(&mut std::io::stdout(), &buf)
                        .map_err(|e| anyhow::anyhow!("failed to write completion script: {e}"))?;
                }
                UtilAction::Snapshot => {
                    let mut repo = Repository::open(".")?;
                    let (author, signing_key) =
                        load_identity_with_ephemeral_fallback(&repo.shared_root)?;
                    repo.set_identity(author, signing_key);
                    let changed = repo.snapshot()?;
                    if changed {
                        println!("Snapshot complete.");
                    } else {
                        println!("No snapshot needed.");
                    }
                }
                UtilAction::Exec { command, args } => {
                    let repo = Repository::open(".")?;
                    let status = std::process::Command::new(&command)
                        .args(&args)
                        .env("ARC_WORKSPACE_ROOT", &repo.work_root)
                        .status()
                        .map_err(|e| {
                            anyhow::anyhow!("failed to execute external command '{}': {e}", command)
                        })?;
                    if let Some(code) = status.code() {
                        std::process::exit(code);
                    }
                    anyhow::ensure!(
                        status.success(),
                        "external command terminated by signal: {status}"
                    );
                }
                UtilAction::ConfigSchema => {
                    let schema = serde_json::json!({
                        "$schema": "https://json-schema.org/draft/2020-12/schema",
                        "title": "arc config schema",
                        "type": "object",
                        "properties": {
                            "user": {
                                "type": "object",
                                "properties": {
                                    "name": {"type": "string"},
                                    "email": {"type": "string"}
                                }
                            },
                            "ui": {
                                "type": "object",
                                "properties": {
                                    "color": {"type": "string", "enum": ["auto", "always", "never"]},
                                    "pager": {"type": "string"},
                                    "editor": {"type": "string"},
                                    "graph_style": {"type": "string"},
                                    "diff_formatter": {"type": "string"},
                                    "conflict_marker_style": {"type": "string"},
                                    "progress_indicator": {"type": "boolean"},
                                    "greet": {"type": "string"},
                                    "movement": {
                                        "type": "object",
                                        "properties": {
                                            "edit": {"type": "boolean"}
                                        }
                                    }
                                }
                            },
                            "merge": {
                                "type": "object",
                                "properties": {
                                    "tool": {"type": "string"}
                                }
                            },
                            "ai": {
                                "type": "object",
                                "properties": {
                                    "provider": {"type": "string", "enum": ["anthropic", "openai-compatible"]},
                                    "model": {"type": "string"},
                                    "endpoint": {"type": "string"}
                                }
                            },
                            "remotes": {"type": "object", "additionalProperties": {"type": "string"}},
                            "aliases": {"type": "object", "additionalProperties": {"type": "string"}},
                            "hooks": {
                                "type": "object",
                                "additionalProperties": {
                                    "type": "array",
                                    "items": {"type": "string"}
                                }
                            },
                            "colors": {"type": "object", "additionalProperties": {"type": "string"}},
                            "hints": {
                                "type": "object",
                                "properties": {
                                    "resolving_conflicts": {"type": "boolean"}
                                }
                            },
                            "snapshot": {
                                "type": "object",
                                "properties": {
                                    "max_new_file_size": {"type": "string"},
                                    "auto_track": {"type": "string"},
                                            "auto_update_stale": {"type": "boolean"},
                                            "blob_extensions": {
                                                "type": "array",
                                                "items": {"type": "string"}
                                            }
                                }
                            },
                            "revsets": {"type": "object", "additionalProperties": {"type": "string"}},
                            "templates": {"type": "object", "additionalProperties": {"type": "string"}},
                            "template-aliases": {"type": "object", "additionalProperties": {"type": "string"}},
                            "merge-tools": {
                                "type": "object",
                                "additionalProperties": {
                                    "type": "object",
                                    "properties": {
                                        "program": {"type": "string"},
                                        "merge_args": {"type": "array", "items": {"type": "string"}},
                                        "edit_args": {"type": "array", "items": {"type": "string"}},
                                        "diff_args": {"type": "array", "items": {"type": "string"}}
                                    }
                                }
                            }
                        }
                    });
                    println!("{}", serde_json::to_string_pretty(&schema)?);
                }
                UtilAction::InstallManPages { path } => {
                    let man1_dir = path.join("man1");
                    std::fs::create_dir_all(&man1_dir).map_err(|e| {
                        anyhow::anyhow!("failed to create man dir '{}': {e}", man1_dir.display())
                    })?;
                    let app = Cli::command();
                    clap_mangen::generate_to(app, man1_dir)
                        .map_err(|e| anyhow::anyhow!("failed to generate man pages: {e}"))?;
                }
                UtilAction::MarkdownHelp => {
                    let markdown = clap_markdown::help_markdown_command(&Cli::command());
                    std::io::Write::write_all(&mut std::io::stdout(), markdown.as_bytes())
                        .map_err(|e| anyhow::anyhow!("failed to write markdown help: {e}"))?;
                }
                UtilAction::Gc { expire } => {
                    if !matches!(expire.as_deref(), None | Some("now")) {
                        anyhow::bail!("--expire only accepts 'now'");
                    }
                    let mut repo = Repository::open(".")?;
                    let result = repo.gc()?;
                    println!(
                        "GC complete: {} change(s) deleted, {} blob(s) deleted.",
                        result.changes_deleted, result.blobs_deleted
                    );
                }
                UtilAction::UxDemo => {
                    run_mock_snap_demo(&term_caps);
                }
            },
            Command::Gc { dry_run } => {
                if dry_run {
                    println!(
                        "(dry-run) GC would analyse the CAS — run without --dry-run to delete."
                    );
                } else {
                    let mut repo = Repository::open(".")?;
                    let result = repo.gc()?;
                    println!(
                        "GC complete: {} change(s) deleted, {} blob(s) deleted.",
                        result.changes_deleted, result.blobs_deleted
                    );
                }
            }
            Command::Compact => {
                let mut repo = Repository::open(".")?;
                let genesis_id = repo.compact()?;
                let hex: String = genesis_id.iter().map(|b| format!("{b:02x}")).collect();
                println!(
                    "Successfully compacted causally stable history into new base state: {hex}"
                );
            }
            Command::Amend { message } => {
                let mut repo = Repository::open(".")?;
                let (author, signing_key) = load_identity()?;
                repo.set_identity(author, signing_key);
                let new_id = repo.amend(message.as_deref())?;
                let hex: String = new_id.iter().map(|b| format!("{b:02x}")).collect();
                println!("Amended → {}", &hex[..8]);
            }
            Command::Absorb { ast } => {
                let mut repo = Repository::open(".")?;
                let (author, signing_key) = load_identity()?;
                repo.set_identity(author, signing_key);

                if !ast {
                    return Err(anyhow::anyhow!("absorb currently requires --ast"))
                        .with_hint_command("Enable AST-aware absorb mode.", "arc absorb --ast");
                }

                let result = repo.absorb_ast()?;
                let target_hex: String =
                    result.selected_target.iter().map(|b| format!("{b:02x}")).collect();

                match result.new_head {
                    Some(new_head) => {
                        let new_hex: String = new_head.iter().map(|b| format!("{b:02x}")).collect();
                        println!(
                            "Absorbed {} AST atom(s) into {} → {}",
                            result.absorbed_atoms,
                            &target_hex[..8],
                            &new_hex[..8]
                        );
                    }
                    None => {
                        println!(
                            "No working-copy changes to absorb (target {}).",
                            &target_hex[..8]
                        );
                    }
                }
            }
            Command::Squash { into } => {
                let mut repo = Repository::open(".")?;
                let (author, signing_key) = load_identity()?;
                repo.set_identity(author, signing_key);
                let new_id = repo.squash_into(&into)?;
                let hex: String = new_id.iter().map(|b| format!("{b:02x}")).collect();
                println!("Squashed → {}", &hex[..8]);
            }
            Command::Reorder { revs } => {
                let mut repo = Repository::open(".")?;
                let (author, signing_key) = load_identity()?;
                repo.set_identity(author, signing_key);
                let new_id = repo.reorder(&revs)?;
                let hex: String = new_id.iter().map(|b| format!("{b:02x}")).collect();
                println!("Reordered → {}", &hex[..8]);
            }
            Command::Restack { continue_mode, abort, revs } => {
                let mut repo = Repository::open(".")?;
                let (author, signing_key) = load_identity()?;
                repo.set_identity(author, signing_key);

                if abort {
                    repo.restack_abort()?;
                    println!("Restack aborted and original heads restored.");
                } else if continue_mode {
                    let new_id = repo.restack_continue()?;
                    let hex: String = new_id.iter().map(|b| format!("{b:02x}")).collect();
                    println!("Restacked (resume) -> {}", &hex[..8]);
                } else {
                    let new_id = repo.restack(&revs)?;
                    let hex: String = new_id.iter().map(|b| format!("{b:02x}")).collect();
                    println!("Restacked -> {}", &hex[..8]);
                }
            }
            Command::Diffedit { prepare, apply, message } => {
                let mut repo = Repository::open(".")?;
                let (author, signing_key) = load_identity()?;
                repo.set_identity(author, signing_key);
                if let Some(target_rev) = prepare {
                    repo.diffedit_prepare(&target_rev)?;
                } else if apply {
                    let new_id = repo.diffedit_apply(message.as_deref())?;
                    let hex: String = new_id.iter().map(|b| format!("{b:02x}")).collect();
                    println!("diffedit applied → {}", &hex[..8]);
                } else {
                    anyhow::bail!("specify --prepare <change> or --apply");
                }
            }
            Command::Identity { name, email } => {
                save_identity(&name, &email)?;
                println!(
                    "Identity configured: {name} <{email}> (Ed25519 keypair active)\nRun 'arc \
                     auth whoami' to inspect your public key."
                );
            }
            Command::Diff { semantic } => {
                let mut repo = Repository::open(".")?;
                let (author, signing_key) =
                    load_identity_with_ephemeral_fallback(&repo.shared_root)?;
                repo.set_identity(author, signing_key);
                let _ = repo.snapshot()?;
                let view_name = repo.current_view_name()?;
                println!("On view: {}", view_name.cyan().bold());
                let (atoms, old_texts) = repo.diff_info()?;
                if atoms.is_empty() {
                    println!("Working directory clean — nothing to diff.");
                } else if semantic {
                    arc_cli::semantic_diff::group_and_render_semantic(&atoms)?;
                } else {
                    arc_cli::semantic_diff::group_and_render(&atoms, &old_texts, &repo.work_root)?;
                }
            }
            Command::Push { remote_url, view } => {
                let mut repo = Repository::open(".")?;
                let current_view = repo.current_view_name()?;
                let view_name = view.unwrap_or_else(|| current_view.clone());

                let config = load_merged_config(std::path::Path::new("."))?;
                let remote = config.remotes.get(&remote_url).cloned().unwrap_or(remote_url.clone());

                arc_cli::sync::push(&mut repo, &remote, &view_name)?;
                println!("Pushed '{}' \u{2192} {}.", view_name, remote);
            }
            Command::Config { global, action } => match action {
                ConfigAction::Alias { name, expansion } => {
                    let mut config = load_merged_config(std::path::Path::new("."))?;
                    config.aliases.insert(name.clone(), expansion.clone());
                    save_global_config(&config)?;
                    println!("Alias '{name}' = '{expansion}' saved.");
                }
                ConfigAction::Aliases => {
                    let config = load_merged_config(std::path::Path::new("."))?;
                    if config.aliases.is_empty() {
                        println!("No aliases configured.");
                    } else {
                        let mut pairs: Vec<_> = config.aliases.iter().collect();
                        pairs.sort_by_key(|(k, _)| k.as_str());
                        for (alias, expansion) in pairs {
                            println!("{alias} = {expansion}");
                        }
                    }
                }
                ConfigAction::Get { key } => {
                    let config = load_merged_config(std::path::Path::new("."))?;
                    match config_get(&config, &key) {
                        Some(v) => println!("{v}"),
                        None => anyhow::bail!("config key '{key}' is not set"),
                    }
                }
                ConfigAction::Set { key, value } => {
                    let shared_root = std::path::Path::new(".");
                    let mut config = if global {
                        load_global_config_layer()?
                    } else {
                        load_merged_config(shared_root)?
                    };
                    config_set(&mut config, &key, &value)?;
                    if global {
                        save_global_config(&config)?;
                    } else {
                        // Write back only the local layer.
                        let local_path = std::path::Path::new(".arc").join("config.toml");
                        let mut local = if local_path.exists() {
                            let text = std::fs::read_to_string(&local_path).unwrap_or_default();
                            toml::from_str::<ArcConfig>(&text).unwrap_or_default()
                        } else {
                            ArcConfig::default()
                        };
                        config_set(&mut local, &key, &value)?;
                        save_local_config(&local, shared_root)?;
                    }
                    println!("Set {key} = {value}");
                }
                ConfigAction::Unset { key } => {
                    let shared_root = std::path::Path::new(".");
                    let local_path = shared_root.join(".arc").join("config.toml");
                    let mut local = if local_path.exists() {
                        let text = std::fs::read_to_string(&local_path).unwrap_or_default();
                        toml::from_str::<ArcConfig>(&text).unwrap_or_default()
                    } else {
                        ArcConfig::default()
                    };
                    config_unset(&mut local, &key)?;
                    save_local_config(&local, shared_root)?;
                    println!("Unset {key}.");
                }
                ConfigAction::Path => {
                    let shared_root = std::path::Path::new(".");
                    let path = if global {
                        global_config_file_path()?
                    } else {
                        local_config_file_path(shared_root)
                    };
                    println!("{}", path.display());
                }
                ConfigAction::Edit => {
                    let shared_root = std::path::Path::new(".");
                    let path = if global {
                        global_config_file_path()?
                    } else {
                        local_config_file_path(shared_root)
                    };

                    if !path.exists() {
                        let empty = ArcConfig::default();
                        if global {
                            save_global_config(&empty)?;
                        } else {
                            save_local_config(&empty, shared_root)?;
                        }
                    }

                    println!("Editing file: {}", path.display());
                    let previous = std::fs::read_to_string(&path).unwrap_or_default();
                    loop {
                        run_editor_on_path(&path)?;
                        let text = std::fs::read_to_string(&path)
                            .map_err(|e| anyhow::anyhow!("failed to read edited config: {e}"))?;
                        match toml::from_str::<ArcConfig>(&text) {
                            Ok(_) => break,
                            Err(err) => {
                                eprintln!("Config parse error: {err}");
                                if !prompt_yes_no(
                                    "Keep editing config? If not, previous config will be restored",
                                )? {
                                    std::fs::write(&path, previous.as_bytes()).map_err(|e| {
                                        anyhow::anyhow!("failed to restore previous config: {e}")
                                    })?;
                                    break;
                                }
                            }
                        }
                    }
                }
                ConfigAction::List => {
                    let config = load_merged_config(std::path::Path::new("."))?;
                    println!("[user]");
                    if let Some(n) = &config.user.name {
                        println!("name = {n}");
                    }
                    if let Some(e) = &config.user.email {
                        println!("email = {e}");
                    }
                    println!("\n[ai]");
                    if let Some(provider) = &config.ai.provider {
                        println!("provider = {provider}");
                    }
                    if let Some(model) = &config.ai.model {
                        println!("model = {model}");
                    }
                    if let Some(endpoint) = &config.ai.endpoint {
                        println!("endpoint = {endpoint}");
                    }
                    println!("\n[ui]");
                    println!("color = {}", config.ui.color);
                    if let Some(v) = &config.ui.pager {
                        println!("pager = {v}");
                    }
                    if let Some(v) = &config.ui.editor {
                        println!("editor = {v}");
                    }
                    if let Some(v) = &config.ui.graph_style {
                        println!("graph_style = {v}");
                    }
                    if let Some(v) = &config.ui.diff_formatter {
                        println!("diff_formatter = {v}");
                    }
                    if let Some(v) = &config.ui.conflict_marker_style {
                        println!("conflict_marker_style = {v}");
                    }
                    if let Some(v) = config.ui.progress_indicator {
                        println!("progress_indicator = {v}");
                    }
                    if let Some(v) = &config.ui.greet {
                        println!("greet = {v}");
                    }
                    if let Some(v) = config.ui.movement.edit {
                        println!("movement.edit = {v}");
                    }
                    println!("\n[merge]");
                    if let Some(t) = &config.merge.tool {
                        println!("tool = {t}");
                    }
                    if let Some(v) = config.hints.resolving_conflicts {
                        println!("\n[hints]\nresolving_conflicts = {v}");
                    }
                    if config.snapshot.max_new_file_size.is_some()
                        || config.snapshot.auto_track.is_some()
                        || config.snapshot.auto_update_stale.is_some()
                    {
                        println!("\n[snapshot]");
                        if let Some(v) = &config.snapshot.max_new_file_size {
                            println!("max_new_file_size = {v}");
                        }
                        if let Some(v) = &config.snapshot.auto_track {
                            println!("auto_track = {v}");
                        }
                        if let Some(v) = config.snapshot.auto_update_stale {
                            println!("auto_update_stale = {v}");
                        }
                    }
                    if !config.remotes.is_empty() {
                        println!("\n[remotes]");
                        let mut rs: Vec<_> = config.remotes.iter().collect();
                        rs.sort_by_key(|(k, _)| k.as_str());
                        for (k, v) in rs {
                            println!("{k} = {v}");
                        }
                    }
                    if !config.aliases.is_empty() {
                        println!("\n[aliases]");
                        let mut al: Vec<_> = config.aliases.iter().collect();
                        al.sort_by_key(|(k, _)| k.as_str());
                        for (k, v) in al {
                            println!("{k} = {v}");
                        }
                    }
                    if !config.revsets.is_empty() {
                        println!("\n[revsets]");
                        let mut vals: Vec<_> = config.revsets.iter().collect();
                        vals.sort_by_key(|(k, _)| k.as_str());
                        for (k, v) in vals {
                            println!("{k} = {v}");
                        }
                    }
                    if !config.templates.is_empty() {
                        println!("\n[templates]");
                        let mut vals: Vec<_> = config.templates.iter().collect();
                        vals.sort_by_key(|(k, _)| k.as_str());
                        for (k, v) in vals {
                            println!("{k} = {v}");
                        }
                    }
                    if !config.template_aliases.is_empty() {
                        println!("\n[template-aliases]");
                        let mut vals: Vec<_> = config.template_aliases.iter().collect();
                        vals.sort_by_key(|(k, _)| k.as_str());
                        for (k, v) in vals {
                            println!("{k} = {v}");
                        }
                    }
                    if !config.colors.is_empty() {
                        println!("\n[colors]");
                        let mut vals: Vec<_> = config.colors.iter().collect();
                        vals.sort_by_key(|(k, _)| k.as_str());
                        for (k, v) in vals {
                            println!("{k} = {v}");
                        }
                    }
                    if !config.merge_tools.is_empty() {
                        let mut vals: Vec<_> = config.merge_tools.iter().collect();
                        vals.sort_by_key(|(k, _)| k.as_str());
                        for (name, tool) in vals {
                            println!("\n[merge-tools.{name}]");
                            if let Some(program) = &tool.program {
                                println!("program = {program}");
                            }
                            if !tool.merge_args.is_empty() {
                                println!("merge_args = {:?}", tool.merge_args);
                            }
                            if !tool.edit_args.is_empty() {
                                println!("edit_args = {:?}", tool.edit_args);
                            }
                            if !tool.diff_args.is_empty() {
                                println!("diff_args = {:?}", tool.diff_args);
                            }
                        }
                    }
                }
            },
            Command::Policy { action } => match action {
                PolicyAction::Explain { path, config } => {
                    let repo = Repository::open(".")?;
                    if config {
                        let trace = explain_config_key(&repo.work_root, &path)?;
                        let heading = format!("Policy explain (config): {}", trace.key);
                        println!("{}", heading.bold());
                        match trace.winner {
                            Some(arc_policy::PolicyValue::Present(value)) => {
                                println!("{} {}", "Result:".bold(), value.green().bold());
                            }
                            Some(arc_policy::PolicyValue::Cleared) => {
                                println!("{} {}", "Result:".bold(), "CLEARED".yellow().bold());
                            }
                            _ => {
                                println!("{} {}", "Result:".bold(), "UNSET".dimmed());
                            }
                        }
                        for entry in trace.entries {
                            let value = match entry.value {
                                arc_policy::PolicyValue::Present(v) => v,
                                arc_policy::PolicyValue::Cleared => "<cleared>".to_string(),
                                arc_policy::PolicyValue::Unset => "<unset>".to_string(),
                            };
                            let line = format!(
                                "{} = {}  [{} depth={} trust={:?}]",
                                entry.key,
                                value,
                                entry.source.origin,
                                entry.source.depth,
                                entry.source.trust
                            );
                            match entry.outcome {
                                arc_policy::TraceOutcome::Winning => {
                                    println!("{}", line.green().bold())
                                }
                                _ => println!("{}", line.dimmed()),
                            }
                        }
                    } else {
                        let matcher = ArcIgnoreMatcher::load(&repo.work_root)?;
                        let trace = matcher.explain_path(&path);
                        let heading = format!("Policy explain (ignore): {}", trace.query_path);
                        println!("{}", heading.bold());
                        let decision = match trace.decision {
                            PathPolicyDecision::Ignored => "IGNORED".green().bold().to_string(),
                            PathPolicyDecision::Included => "INCLUDED".yellow().bold().to_string(),
                            PathPolicyDecision::Unset => "UNSET".dimmed().to_string(),
                        };
                        println!("{} {}", "Result:".bold(), decision);
                        for entry in trace.entries {
                            let polarity = match entry.value {
                                arc_policy::PolicyValue::Present(true) => "ignore",
                                arc_policy::PolicyValue::Present(false) => "allow",
                                arc_policy::PolicyValue::Cleared => "cleared",
                                arc_policy::PolicyValue::Unset => "unset",
                            };
                            let line = format!(
                                "{} ({})  [{}:{} depth={} trust={:?}]",
                                entry.pattern,
                                polarity,
                                entry.source.origin,
                                entry.line,
                                entry.source.depth,
                                entry.source.trust
                            );
                            match entry.outcome {
                                arc_policy::TraceOutcome::Winning => {
                                    println!("{}", line.green().bold())
                                }
                                _ => println!("{}", line.dimmed()),
                            }
                        }
                    }
                }
            },
            Command::Synthesis { action } => match action {
                SynthesisAction::Capture { source, files } => {
                    capture_synthesis_snapshot(&source, &files)?;
                }
                SynthesisAction::Show { id } => {
                    show_synthesis_snapshot(&id)?;
                }
                SynthesisAction::List => {
                    let repo = Repository::open(".")?;
                    let ids = list_snapshot_ids(&repo.shared_root)?;
                    if ids.is_empty() {
                        println!("No synthesis snapshots found.");
                    } else {
                        for id in ids {
                            println!("{id}");
                        }
                    }
                }
            },
            Command::BugReport { output, include_raw_intent } => {
                let repo = Repository::open(".")?;
                let out_path = output.unwrap_or_else(|| "./arc-bugreport.json".to_string());
                arc_cli::bugreport::generate(&repo, &out_path, include_raw_intent)?;
                println!("Bug report written to: {out_path}");
            }
            Command::Daemon => {
                run_daemon_subprocess()?;
            }
        }
        Ok(())
    })
}

fn main() {
    let raw_args: Vec<String> = std::env::args().collect();
    let raw_force_json = raw_args.iter().any(|arg| arg == "--json");
    let raw_quiet = raw_args.iter().any(|arg| arg == "--quiet");

    match run_cli() {
        Ok(()) => {}
        Err(error) => {
            let (force_json, quiet) =
                OUTPUT_PREFS.get().copied().unwrap_or((raw_force_json, raw_quiet));
            render_diagnostic_error(error, force_json, quiet);
            std::process::exit(1);
        }
    }
}

fn run_daemon_subprocess() -> anyhow::Result<()> {
    use std::process::Stdio;

    let current_exe = std::env::current_exe()?;
    let daemon_name = if cfg!(windows) { "arc-daemon.exe" } else { "arc-daemon" };
    let sibling = current_exe.with_file_name(daemon_name);

    anyhow::ensure!(
        sibling.exists(),
        "arc-daemon executable was not found next to '{}'",
        current_exe.display()
    );

    let mut cmd = std::process::Command::new(sibling);

    let status = cmd
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .map_err(|e| anyhow::anyhow!("failed to launch arc-daemon subprocess: {e}"))?;

    anyhow::ensure!(status.success(), "arc-daemon exited with status {status}");
    Ok(())
}

#[tracing::instrument(skip_all, fields(label = %label))]
fn edit_lines_in_editor(
    label: &str,
    extension: &str,
    current_lines: &[String],
) -> anyhow::Result<Vec<String>> {
    let mut content = String::new();
    for line in current_lines {
        content.push_str(line);
        content.push('\n');
    }

    let mut temp = tempfile::Builder::new()
        .prefix("arc-")
        .suffix(extension)
        .tempfile()
        .map_err(|e| anyhow::anyhow!("failed to create temporary editor file: {e}"))?;
    use std::io::Write as _;
    temp.write_all(content.as_bytes())
        .map_err(|e| anyhow::anyhow!("failed to write temporary editor file: {e}"))?;

    let temp_path = temp.path().to_path_buf();

    run_editor_on_path(&temp_path)?;
    let edited = std::fs::read_to_string(&temp_path)
        .map_err(|e| anyhow::anyhow!("failed to read temporary editor file: {e}"))?;
    drop(temp);

    let mut out: Vec<String> = edited
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(ToOwned::to_owned)
        .collect();
    out.sort();
    out.dedup();
    Ok(out)
}

fn run_editor_on_path(path: &std::path::Path) -> anyhow::Result<()> {
    let editor = std::env::var("EDITOR")
        .unwrap_or_else(|_| if cfg!(windows) { "notepad".to_string() } else { "vi".to_string() });

    let mut parts = shlex::split(&editor)
        .ok_or_else(|| anyhow::anyhow!("failed to parse EDITOR command: {editor}"))?;
    let bin = parts.drain(..1).next().ok_or_else(|| anyhow::anyhow!("EDITOR command is empty"))?;
    let status = std::process::Command::new(bin)
        .args(parts)
        .arg(path)
        .status()
        .map_err(|e| anyhow::anyhow!("failed to launch editor: {e}"))?;
    anyhow::ensure!(status.success(), "editor exited with status {status}");
    Ok(())
}

fn prompt_yes_no(prompt: &str) -> anyhow::Result<bool> {
    use std::io::Write as _;
    print!("{prompt} [Y/n]: ");
    std::io::stdout().flush().map_err(|e| anyhow::anyhow!("failed to flush stdout: {e}"))?;

    let mut input = String::new();
    std::io::stdin()
        .read_line(&mut input)
        .map_err(|e| anyhow::anyhow!("failed to read prompt input: {e}"))?;
    let trimmed = input.trim().to_ascii_lowercase();
    Ok(trimmed.is_empty() || trimmed == "y" || trimmed == "yes")
}

fn resolve_log_template_alias(raw_template: &str, config: &ArcConfig) -> String {
    if let Some(template) = config.template_aliases.get(raw_template) {
        return template.clone();
    }
    if let Some(template) = config.templates.get(raw_template) {
        return template.clone();
    }
    raw_template.to_string()
}

fn find_command_token_index(
    raw_args: &[String],
    config: &ArcConfig,
    command_names: &std::collections::HashSet<String>,
) -> Option<usize> {
    if raw_args.len() < 2 {
        return None;
    }

    for (idx, token) in raw_args.iter().enumerate().skip(1) {
        if token == "--" {
            // `--` terminates top-level option parsing; remaining tokens are
            // literals and must not be rewritten by command alias expansion.
            return None;
        }

        if token.starts_with('-') {
            continue;
        }

        if config.aliases.contains_key(token) || command_names.contains(token) {
            return Some(idx);
        }
    }

    None
}

fn expand_command_aliases(
    config: &ArcConfig,
    mut raw_args: Vec<String>,
) -> anyhow::Result<Vec<String>> {
    let mut seen_aliases: Vec<String> = Vec::new();
    let command_names: std::collections::HashSet<String> =
        Cli::command().get_subcommands().map(|command| command.get_name().to_string()).collect();

    while let Some(command_idx) = find_command_token_index(&raw_args, config, &command_names) {
        let command = raw_args[command_idx].clone();
        let Some(expansion) = config.aliases.get(&command) else {
            break;
        };

        if let Some(cycle_start) = seen_aliases.iter().position(|name| name == &command) {
            let mut cycle = seen_aliases[cycle_start..].to_vec();
            cycle.push(command.clone());
            anyhow::bail!("alias cycle detected: {}", cycle.join(" -> "));
        }

        let expansion_trimmed = expansion.trim();
        if expansion_trimmed.is_empty() {
            anyhow::bail!("alias '{}' has an empty expansion", command);
        }

        let Some(expanded_tokens) = shlex::split(expansion_trimmed) else {
            anyhow::bail!("alias '{}' has invalid shell words: {}", command, expansion);
        };

        if expanded_tokens.is_empty() {
            anyhow::bail!("alias '{}' has an empty expansion", command);
        }

        seen_aliases.push(command);

        let mut next_args = Vec::with_capacity(raw_args.len() + expanded_tokens.len());
        next_args.extend(raw_args[..command_idx].iter().cloned());
        next_args.extend(expanded_tokens);
        next_args.extend(raw_args[command_idx + 1..].iter().cloned());
        raw_args = next_args;
    }

    Ok(raw_args)
}

struct TagSetUpdate {
    name: String,
    needs_write: bool,
}

struct TagSetPlan {
    updates: Vec<TagSetUpdate>,
    created: usize,
    moved: usize,
}

fn plan_tag_set_updates(
    existing: std::collections::HashMap<String, Blake3Hash>,
    target: Blake3Hash,
    names: Vec<String>,
) -> TagSetPlan {
    let mut unique_names = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for name in names {
        if seen.insert(name.clone()) {
            unique_names.push(name);
        }
    }

    let mut created = 0usize;
    let mut moved = 0usize;
    let mut updates = Vec::with_capacity(unique_names.len());

    for name in unique_names {
        let needs_write = match existing.get(&name) {
            Some(old) => {
                if old != &target {
                    moved += 1;
                    true
                } else {
                    false
                }
            }
            None => {
                created += 1;
                true
            }
        };
        updates.push(TagSetUpdate { name, needs_write });
    }

    TagSetPlan { updates, created, moved }
}

/// Get a typed config value by dot-separated key.
fn config_get(cfg: &ArcConfig, key: &str) -> Option<String> {
    match key {
        "user.name" => cfg.user.name.clone(),
        "user.email" => cfg.user.email.clone(),
        "ui.color" => Some(cfg.ui.color.clone()),
        "ui.pager" => cfg.ui.pager.clone(),
        "ui.editor" => cfg.ui.editor.clone(),
        "ui.graph_style" => cfg.ui.graph_style.clone(),
        "ui.diff_formatter" => cfg.ui.diff_formatter.clone(),
        "ui.conflict_marker_style" => cfg.ui.conflict_marker_style.clone(),
        "ui.progress_indicator" => cfg.ui.progress_indicator.map(|v| v.to_string()),
        "ui.greet" => cfg.ui.greet.clone(),
        "ui.movement.edit" => cfg.ui.movement.edit.map(|v| v.to_string()),
        "merge.tool" => cfg.merge.tool.clone(),
        "ai.provider" => cfg.ai.provider.clone(),
        "ai.model" => cfg.ai.model.clone(),
        "ai.endpoint" => cfg.ai.endpoint.clone(),
        "hints.resolving_conflicts" => cfg.hints.resolving_conflicts.map(|v| v.to_string()),
        "snapshot.max_new_file_size" => cfg.snapshot.max_new_file_size.clone(),
        "snapshot.auto_track" => cfg.snapshot.auto_track.clone(),
        "snapshot.auto_update_stale" => cfg.snapshot.auto_update_stale.map(|v| v.to_string()),
        _ => {
            // remotes.<name> and aliases.<name>
            if let Some(name) = key.strip_prefix("remotes.") {
                cfg.remotes.get(name).cloned()
            } else if let Some(name) = key.strip_prefix("aliases.") {
                cfg.aliases.get(name).cloned()
            } else if let Some(name) = key.strip_prefix("revsets.") {
                cfg.revsets.get(name).cloned()
            } else if let Some(name) = key.strip_prefix("templates.") {
                cfg.templates.get(name).cloned()
            } else if let Some(name) = key
                .strip_prefix("template-aliases.")
                .or_else(|| key.strip_prefix("template_aliases."))
            {
                cfg.template_aliases.get(name).cloned()
            } else if let Some(name) = key.strip_prefix("colors.") {
                cfg.colors.get(name).cloned()
            } else {
                None
            }
        }
    }
}

/// Set a typed config value by dot-separated key.
fn config_set(cfg: &mut ArcConfig, key: &str, value: &str) -> anyhow::Result<()> {
    match key {
        "user.name" => cfg.user.name = Some(value.to_string()),
        "user.email" => cfg.user.email = Some(value.to_string()),
        "ui.color" => {
            anyhow::ensure!(
                matches!(value, "auto" | "always" | "never"),
                "ui.color must be 'auto', 'always', or 'never'"
            );
            cfg.ui.color = value.to_string();
        }
        "ui.pager" => cfg.ui.pager = Some(value.to_string()),
        "ui.editor" => cfg.ui.editor = Some(value.to_string()),
        "ui.graph_style" => cfg.ui.graph_style = Some(value.to_string()),
        "ui.diff_formatter" => cfg.ui.diff_formatter = Some(value.to_string()),
        "ui.conflict_marker_style" => cfg.ui.conflict_marker_style = Some(value.to_string()),
        "ui.greet" => cfg.ui.greet = Some(value.to_string()),
        "ui.progress_indicator" => {
            cfg.ui.progress_indicator = Some(
                value
                    .parse::<bool>()
                    .map_err(|_| anyhow::anyhow!("ui.progress_indicator must be true or false"))?,
            )
        }
        "ui.movement.edit" => {
            cfg.ui.movement.edit = Some(
                value
                    .parse::<bool>()
                    .map_err(|_| anyhow::anyhow!("ui.movement.edit must be true or false"))?,
            )
        }
        "merge.tool" => cfg.merge.tool = Some(value.to_string()),
        "ai.provider" => {
            anyhow::ensure!(
                matches!(value, "anthropic" | "openai-compatible"),
                "ai.provider must be 'anthropic' or 'openai-compatible'"
            );
            cfg.ai.provider = Some(value.to_string());
        }
        "ai.model" => cfg.ai.model = Some(value.to_string()),
        "ai.endpoint" => cfg.ai.endpoint = Some(value.to_string()),
        "hints.resolving_conflicts" => {
            cfg.hints.resolving_conflicts =
                Some(value.parse::<bool>().map_err(|_| {
                    anyhow::anyhow!("hints.resolving_conflicts must be true or false")
                })?)
        }
        "snapshot.max_new_file_size" => cfg.snapshot.max_new_file_size = Some(value.to_string()),
        "snapshot.auto_track" => cfg.snapshot.auto_track = Some(value.to_string()),
        "snapshot.auto_update_stale" => {
            cfg.snapshot.auto_update_stale =
                Some(value.parse::<bool>().map_err(|_| {
                    anyhow::anyhow!("snapshot.auto_update_stale must be true or false")
                })?)
        }
        _ => {
            if let Some(name) = key.strip_prefix("remotes.") {
                cfg.remotes.insert(name.to_string(), value.to_string());
            } else if let Some(name) = key.strip_prefix("aliases.") {
                cfg.aliases.insert(name.to_string(), value.to_string());
            } else if let Some(name) = key.strip_prefix("revsets.") {
                cfg.revsets.insert(name.to_string(), value.to_string());
            } else if let Some(name) = key.strip_prefix("templates.") {
                cfg.templates.insert(name.to_string(), value.to_string());
            } else if let Some(name) = key
                .strip_prefix("template-aliases.")
                .or_else(|| key.strip_prefix("template_aliases."))
            {
                cfg.template_aliases.insert(name.to_string(), value.to_string());
            } else if let Some(name) = key.strip_prefix("colors.") {
                cfg.colors.insert(name.to_string(), value.to_string());
            } else {
                anyhow::bail!(
                    "unknown config key '{key}'; known keys: user.name, user.email, ui.color, \
                     ui.pager, ui.editor, ui.graph_style, ui.diff_formatter, \
                     ui.conflict_marker_style, ui.progress_indicator, ui.greet, ui.movement.edit, \
                     merge.tool, ai.provider, ai.model, ai.endpoint, hints.resolving_conflicts, \
                     snapshot.max_new_file_size, snapshot.auto_track, snapshot.auto_update_stale, \
                     remotes.<name>, aliases.<name>, revsets.<name>, templates.<name>, \
                     template-aliases.<name>, colors.<name>"
                );
            }
        }
    }
    Ok(())
}

/// Unset (clear) a typed config value by dot-separated key.
fn config_unset(cfg: &mut ArcConfig, key: &str) -> anyhow::Result<()> {
    match key {
        "user.name" => cfg.user.name = None,
        "user.email" => cfg.user.email = None,
        "ui.color" => cfg.ui.color = "auto".to_string(),
        "ui.pager" => cfg.ui.pager = None,
        "ui.editor" => cfg.ui.editor = None,
        "ui.graph_style" => cfg.ui.graph_style = None,
        "ui.diff_formatter" => cfg.ui.diff_formatter = None,
        "ui.conflict_marker_style" => cfg.ui.conflict_marker_style = None,
        "ui.progress_indicator" => cfg.ui.progress_indicator = None,
        "ui.greet" => cfg.ui.greet = None,
        "ui.movement.edit" => cfg.ui.movement.edit = None,
        "merge.tool" => cfg.merge.tool = None,
        "ai.provider" => cfg.ai.provider = None,
        "ai.model" => cfg.ai.model = None,
        "ai.endpoint" => cfg.ai.endpoint = None,
        "hints.resolving_conflicts" => cfg.hints.resolving_conflicts = None,
        "snapshot.max_new_file_size" => cfg.snapshot.max_new_file_size = None,
        "snapshot.auto_track" => cfg.snapshot.auto_track = None,
        "snapshot.auto_update_stale" => cfg.snapshot.auto_update_stale = None,
        _ => {
            if let Some(name) = key.strip_prefix("remotes.") {
                cfg.remotes.remove(name);
            } else if let Some(name) = key.strip_prefix("aliases.") {
                cfg.aliases.remove(name);
            } else if let Some(name) = key.strip_prefix("revsets.") {
                cfg.revsets.remove(name);
            } else if let Some(name) = key.strip_prefix("templates.") {
                cfg.templates.remove(name);
            } else if let Some(name) = key
                .strip_prefix("template-aliases.")
                .or_else(|| key.strip_prefix("template_aliases."))
            {
                cfg.template_aliases.remove(name);
            } else if let Some(name) = key.strip_prefix("colors.") {
                cfg.colors.remove(name);
            } else {
                anyhow::bail!(
                    "unknown config key '{key}'; known keys: user.name, user.email, ui.color, \
                     ui.pager, ui.editor, ui.graph_style, ui.diff_formatter, \
                     ui.conflict_marker_style, ui.progress_indicator, ui.greet, ui.movement.edit, \
                     merge.tool, ai.provider, ai.model, ai.endpoint, hints.resolving_conflicts, \
                     snapshot.max_new_file_size, snapshot.auto_track, snapshot.auto_update_stale, \
                     remotes.<name>, aliases.<name>, revsets.<name>, templates.<name>, \
                     template-aliases.<name>, colors.<name>"
                );
            }
        }
    }
    Ok(())
}

/// Parse a 64-character hex string into a [`Blake3Hash`].
fn hex_to_hash(hex: &str) -> anyhow::Result<Blake3Hash> {
    if hex.len() != 64 {
        anyhow::bail!("hash must be exactly 64 hex characters, got {}", hex.len());
    }
    let mut out = [0u8; 32];
    for (i, chunk) in hex.as_bytes().chunks(2).enumerate() {
        let hi = dehex(chunk[0])?;
        let lo = dehex(chunk[1])?;
        out[i] = (hi << 4) | lo;
    }
    Ok(out)
}

fn dehex(b: u8) -> anyhow::Result<u8> {
    match b {
        b'0'..=b'9' => Ok(b - b'0'),
        b'a'..=b'f' => Ok(b - b'a' + 10),
        b'A'..=b'F' => Ok(b - b'A' + 10),
        _ => anyhow::bail!("invalid hex character: {}", b as char),
    }
}

fn short_change_id(id: ChangeId) -> String {
    let hex = id.to_hex();
    hex[..8].to_string()
}

fn display_bisect_counts(state: &arc_store_graph::bisect::BisectState) -> (usize, usize) {
    if state.find_good {
        (state.bad_count(), state.good_count())
    } else {
        (state.good_count(), state.bad_count())
    }
}

use arc_algebra_types::Atom;

fn atom_display_label(atom: &Atom) -> String {
    match atom {
        Atom::Insert { at, .. } => {
            format!("++ Added:   {}", at.last().unwrap_or(&"?".to_string())).green().to_string()
        }
        Atom::Delete { at, .. } => {
            format!("-- Deleted: {}", at.last().unwrap_or(&"?".to_string())).red().to_string()
        }
        Atom::Move { from, to } => format!(
            "~~ Moved:   {} → {}",
            from.last().unwrap_or(&"?".to_string()),
            to.last().unwrap_or(&"?".to_string())
        )
        .yellow()
        .to_string(),
        Atom::SemanticsPreserving { at, description } => {
            format!("~~ Reformat: {} ({})", at.last().unwrap_or(&"?".to_string()), description)
                .yellow()
                .to_string()
        }
        Atom::Directory { path } => {
            format!("++ Dir:     {}", path.last().unwrap_or(&"?".to_string())).green().to_string()
        }
        Atom::Blob { path, .. } => format!("~~ Blob:    {path}").yellow().to_string(),
        Atom::Mount { path, .. } => {
            format!("~~ Mount:   {}", path.last().unwrap_or(&"?".to_string())).cyan().to_string()
        }
        Atom::Conflict { at, .. } => {
            format!("!! Conflict: {}", at.last().unwrap_or(&"?".to_string())).red().to_string()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use arc_cli::ops::DEFAULT_SYNC_SLO_MS;
    use arc_cli::policy_gate::{
        AiResolver, ArcPolicy, Ast, Evaluator, MockAiResolver, PolicyError, RUST_BOUNDARY_QUERY,
        RUST_INVOCATION_QUERY, TreeSitterEvaluator, ast_context, default_evaluator, verify_lens,
    };

    fn test_raw_args(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|part| (*part).to_string()).collect()
    }

    fn config_with_aliases(pairs: &[(&str, &str)]) -> ArcConfig {
        let mut config = ArcConfig::default();
        for (name, expansion) in pairs {
            config.aliases.insert((*name).to_string(), (*expansion).to_string());
        }
        config
    }

    #[test]
    fn plan_tag_set_updates_deduplicates_and_counts() {
        let existing = std::collections::HashMap::from([
            ("v1".to_string(), [1u8; 32]),
            ("v2".to_string(), [2u8; 32]),
        ]);
        let target = [2u8; 32];
        let names = vec!["v3".to_string(), "v1".to_string(), "v1".to_string(), "v2".to_string()];

        let plan = plan_tag_set_updates(existing, target, names);

        assert_eq!(plan.created, 1);
        assert_eq!(plan.moved, 1);
        assert_eq!(plan.updates.len(), 3);
        assert_eq!(plan.updates[0].name, "v3");
        assert!(plan.updates[0].needs_write);
        assert_eq!(plan.updates[1].name, "v1");
        assert!(plan.updates[1].needs_write);
        assert_eq!(plan.updates[2].name, "v2");
        assert!(!plan.updates[2].needs_write);
    }

    #[test]
    fn resolve_log_template_alias_prefers_template_aliases() {
        let mut config = ArcConfig::default();
        config.template_aliases.insert("compact".to_string(), "{id_short} {intent}".to_string());
        config.templates.insert("compact".to_string(), "{id} {intent}".to_string());

        let resolved = resolve_log_template_alias("compact", &config);
        assert_eq!(resolved, "{id_short} {intent}");
    }

    #[test]
    fn resolve_log_template_alias_falls_back_to_templates() {
        let mut config = ArcConfig::default();
        config.templates.insert("verbose".to_string(), "{id} {author} {intent}".to_string());

        let resolved = resolve_log_template_alias("verbose", &config);
        assert_eq!(resolved, "{id} {author} {intent}");
    }

    #[test]
    fn expand_command_aliases_recursively_expands_until_non_alias() {
        let config = config_with_aliases(&[("s", "st"), ("st", "status")]);
        let raw_args = test_raw_args(&["arc", "s"]);

        let expanded = expand_command_aliases(&config, raw_args).expect("alias expansion failed");

        assert_eq!(expanded, test_raw_args(&["arc", "status"]));
    }

    #[test]
    fn expand_command_aliases_detects_direct_cycle() {
        let config = config_with_aliases(&[("s", "s")]);
        let raw_args = test_raw_args(&["arc", "s"]);

        let err = expand_command_aliases(&config, raw_args).expect_err("expected cycle error");

        assert!(err.to_string().contains("alias cycle"));
        assert!(err.to_string().contains("s"));
    }

    #[test]
    fn expand_command_aliases_detects_indirect_cycle() {
        let config = config_with_aliases(&[("a", "b"), ("b", "c"), ("c", "a")]);
        let raw_args = test_raw_args(&["arc", "a"]);

        let err = expand_command_aliases(&config, raw_args).expect_err("expected cycle error");

        assert!(err.to_string().contains("alias cycle"));
        assert!(err.to_string().contains("a -> b -> c -> a"));
    }

    #[test]
    fn expand_command_aliases_supports_global_flags_before_command() {
        let config = config_with_aliases(&[("st", "status")]);
        let raw_args = test_raw_args(&["arc", "--help", "st"]);

        let expanded = expand_command_aliases(&config, raw_args).expect("alias expansion failed");

        assert_eq!(expanded, test_raw_args(&["arc", "--help", "status"]));
    }

    #[test]
    fn expand_command_aliases_preserves_trailing_args() {
        let config = config_with_aliases(&[("l", "log --intent")]);
        let raw_args = test_raw_args(&["arc", "l", "refactor parser"]);

        let expanded = expand_command_aliases(&config, raw_args).expect("alias expansion failed");

        assert_eq!(expanded, test_raw_args(&["arc", "log", "--intent", "refactor parser"]));
    }

    #[test]
    fn expand_command_aliases_errors_on_empty_alias_definition() {
        let config = config_with_aliases(&[("st", "   ")]);
        let raw_args = test_raw_args(&["arc", "st"]);

        let err =
            expand_command_aliases(&config, raw_args).expect_err("expected empty alias error");

        assert!(err.to_string().contains("alias 'st' has an empty expansion"));
    }

    #[test]
    fn expand_command_aliases_errors_on_invalid_alias_definition() {
        let config = config_with_aliases(&[("st", "\"")]);
        let raw_args = test_raw_args(&["arc", "st"]);

        let err = expand_command_aliases(&config, raw_args)
            .expect_err("expected invalid alias definition error");

        assert!(err.to_string().contains("alias 'st' has invalid shell words"));
    }

    #[test]
    fn expand_command_aliases_does_not_expand_after_option_terminator() {
        let config = config_with_aliases(&[("st", "status")]);
        let raw_args = test_raw_args(&["arc", "--", "st"]);

        let expanded = expand_command_aliases(&config, raw_args).expect("alias expansion failed");

        assert_eq!(expanded, test_raw_args(&["arc", "--", "st"]));
    }

    #[test]
    fn parses_policy_explain_command() {
        let parsed = Cli::try_parse_from(["arc", "policy", "explain", "src/main.rs"])
            .expect("policy explain should parse");

        match parsed.command {
            Command::Policy { action } => match action {
                PolicyAction::Explain { path, config } => {
                    assert_eq!(path, "src/main.rs");
                    assert!(!config);
                }
            },
            _ => panic!("unexpected command parsed"),
        }
    }

    #[test]
    fn parses_restack_continue_command() {
        let parsed =
            Cli::try_parse_from(["arc", "restack", "--continue"]).expect("restack continue");

        match parsed.command {
            Command::Restack { continue_mode, abort, revs } => {
                assert!(continue_mode);
                assert!(!abort);
                assert!(revs.is_empty());
            }
            _ => panic!("unexpected command parsed"),
        }
    }

    #[test]
    fn parses_restack_revisions_command() {
        let parsed =
            Cli::try_parse_from(["arc", "restack", "HEAD~1", "HEAD"]).expect("restack with revs");

        match parsed.command {
            Command::Restack { continue_mode, abort, revs } => {
                assert!(!continue_mode);
                assert!(!abort);
                assert_eq!(revs, vec!["HEAD~1".to_string(), "HEAD".to_string()]);
            }
            _ => panic!("unexpected command parsed"),
        }
    }

    #[test]
    fn diagnostic_lines_omit_hint_for_plain_error() {
        let err = anyhow::anyhow!("plain failure");
        let lines = diagnostic_lines(&err);
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("plain failure"));
    }

    #[test]
    fn diagnostic_lines_render_hint_and_command_when_present() {
        let err: anyhow::Result<()> = Err(anyhow::anyhow!("restack paused"));
        let err = err
            .with_hint_command("Resolve conflicts, then continue.", "arc restack --continue")
            .expect_err("expected error");
        let lines = diagnostic_lines(&err);
        assert_eq!(lines.len(), 4);
        assert!(lines[1].contains("-"));
        assert!(lines[2].contains("Resolve conflicts, then continue."));
        assert!(lines[3].contains("arc restack --continue"));
    }

    #[test]
    fn diagnostic_lines_render_error_causes() {
        let err = anyhow::anyhow!("disk write failed").context("cannot persist checkpoint");
        let lines = diagnostic_lines(&err);
        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains("cannot persist checkpoint"));
        assert!(lines[1].contains("caused by: disk write failed"));
    }

    #[test]
    fn parses_snap_without_message() {
        let parsed = Cli::try_parse_from(["arc", "snap"]).expect("snap should parse");

        match parsed.command {
            Command::Snap { message, auto_msg, interactive } => {
                assert!(message.is_none());
                assert!(!auto_msg);
                assert!(!interactive);
            }
            _ => panic!("unexpected command parsed"),
        }
    }

    #[test]
    fn help_shows_daily_use_group() {
        let mut cmd = Cli::command();
        let help = cmd.render_help().to_string();

        assert!(help.contains("DAILY USE"));
        assert!(help.contains("init"));
        assert!(help.contains("snap"));
        assert!(help.contains("watch"));
        assert!(help.contains("sync"));
    }

    // --- dehex tests -------------------------------------------------------

    #[test]
    fn dehex_digit_zero_through_nine() {
        for d in 0..=9u8 {
            let ch = b'0' + d;
            assert_eq!(dehex(ch).unwrap(), d, "dehex({} as char) should be {d}", ch as char);
        }
    }

    #[test]
    fn dehex_lowercase_a_through_f() {
        for (i, expected) in (10..=15u8).enumerate() {
            let ch = b'a' + i as u8;
            assert_eq!(dehex(ch).unwrap(), expected);
        }
    }

    #[test]
    fn dehex_uppercase_a_through_f() {
        for (i, expected) in (10..=15u8).enumerate() {
            let ch = b'A' + i as u8;
            assert_eq!(dehex(ch).unwrap(), expected);
        }
    }

    #[test]
    fn dehex_invalid_character_returns_error() {
        let err = dehex(b'x').expect_err("expected error for 'x'");
        assert!(err.to_string().contains("invalid hex character"));
    }

    #[test]
    fn dehex_space_is_invalid() {
        assert!(dehex(b' ').is_err());
    }

    // --- hex_to_hash tests -------------------------------------------------

    #[test]
    fn hex_to_hash_valid_64_char_string() {
        let hex = "00".repeat(32);
        let hash = hex_to_hash(&hex).expect("valid hex should succeed");
        assert_eq!(hash, [0u8; 32]);
    }

    #[test]
    fn hex_to_hash_all_ff_bytes() {
        let hex = "ff".repeat(32);
        let hash = hex_to_hash(&hex).expect("valid hex should succeed");
        assert_eq!(hash, [0xff; 32]);
    }

    #[test]
    fn hex_to_hash_known_pattern() {
        // "0a1b" -> first two bytes are 0x0a and 0x1b
        let mut hex = String::with_capacity(64);
        for i in 0..32u8 {
            hex.push_str(&format!("{:02x}", i));
        }
        let hash = hex_to_hash(&hex).expect("valid hex should succeed");
        for i in 0..32u8 {
            assert_eq!(hash[i as usize], i);
        }
    }

    #[test]
    fn hex_to_hash_too_short_returns_error() {
        let err = hex_to_hash("abcd").expect_err("expected error for short input");
        assert!(err.to_string().contains("must be exactly 64"));
    }

    #[test]
    fn hex_to_hash_too_long_returns_error() {
        let err = hex_to_hash(&"ab".repeat(33)).expect_err("expected error for long input");
        assert!(err.to_string().contains("must be exactly 64"));
    }

    #[test]
    fn hex_to_hash_invalid_chars_returns_error() {
        let hex = "gg".repeat(32);
        assert!(hex_to_hash(&hex).is_err());
    }

    // --- short_change_id tests ---------------------------------------------

    #[test]
    fn short_change_id_returns_first_8_hex_chars() {
        let hash = [
            0x0a, 0x1b, 0x2c, 0x3d, 0x4e, 0x5f, 0x6a, 0x7b, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00,
        ];
        let id = ChangeId(hash);
        let short = short_change_id(id);
        assert_eq!(short.len(), 8);
        assert_eq!(short, "0a1b2c3d");
    }

    // --- display_bisect_counts tests ----------------------------------------

    fn make_bisect_state(
        good: usize,
        bad: usize,
        untested: usize,
        find_good: bool,
    ) -> arc_store_graph::bisect::BisectState {
        use arc_store_graph::bisect::{BisectMark, BisectState};
        let mut marks = std::collections::BTreeMap::new();
        let mut candidates = BTreeSet::new();
        let mut id_bytes = [0u8; 32];
        for i in 0..(good + bad + untested) {
            id_bytes[31] = i as u8;
            let id = ChangeId(id_bytes);
            candidates.insert(id);
            let mark = if i < good {
                BisectMark::Good
            } else if i < good + bad {
                BisectMark::Bad
            } else {
                BisectMark::Untested
            };
            marks.insert(id, mark);
        }
        BisectState {
            range_expr: "test".to_string(),
            find_good,
            candidates,
            marks,
            current: None,
            started_at_unix: 0,
        }
    }

    #[test]
    fn display_bisect_counts_find_good_swaps_order() {
        let state = make_bisect_state(3, 2, 5, true);
        let (first, second) = display_bisect_counts(&state);
        // When find_good=true: returns (bad_count, good_count)
        assert_eq!(first, 2); // bad
        assert_eq!(second, 3); // good
    }

    #[test]
    fn display_bisect_counts_find_bad_natural_order() {
        let state = make_bisect_state(3, 2, 5, false);
        let (first, second) = display_bisect_counts(&state);
        // When find_good=false: returns (good_count, bad_count)
        assert_eq!(first, 3); // good
        assert_eq!(second, 2); // bad
    }

    // --- atom_display_label tests -------------------------------------------

    #[test]
    fn atom_display_label_insert_contains_added() {
        let atom = Atom::Insert {
            at: vec!["file.rs".to_string(), "fn_main".to_string()],
            content_hash: [0u8; 32],
        };
        let label = atom_display_label(&atom);
        assert!(label.contains("Added"));
        assert!(label.contains("fn_main"));
    }

    #[test]
    fn atom_display_label_delete_contains_deleted() {
        let atom = Atom::Delete {
            at: vec!["file.rs".to_string(), "fn_old".to_string()],
            prior_hash: [0u8; 32],
        };
        let label = atom_display_label(&atom);
        assert!(label.contains("Deleted"));
        assert!(label.contains("fn_old"));
    }

    #[test]
    fn atom_display_label_move_contains_moved() {
        let atom = Atom::Move { from: vec!["old.rs".to_string()], to: vec!["new.rs".to_string()] };
        let label = atom_display_label(&atom);
        assert!(label.contains("Moved"));
        assert!(label.contains("old.rs"));
        assert!(label.contains("new.rs"));
    }

    #[test]
    fn atom_display_label_directory_contains_dir() {
        let atom = Atom::Directory { path: vec!["src".to_string(), "utils".to_string()] };
        let label = atom_display_label(&atom);
        assert!(label.contains("Dir"));
        assert!(label.contains("utils"));
    }

    #[test]
    fn atom_display_label_blob_contains_blob() {
        let atom = Atom::Blob {
            path: "assets/logo.png".to_string(),
            hash: blake3::Hash::from_bytes([0u8; 32]),
            size: 1024,
        };
        let label = atom_display_label(&atom);
        assert!(label.contains("Blob"));
        assert!(label.contains("assets/logo.png"));
    }

    #[test]
    fn atom_display_label_conflict_contains_conflict() {
        let atom = Atom::Conflict {
            at: vec!["file.rs".to_string()],
            bases: vec![[0u8; 32]],
            sides: vec![[1u8; 32], [2u8; 32]],
        };
        let label = atom_display_label(&atom);
        assert!(label.contains("Conflict"));
        assert!(label.contains("file.rs"));
    }

    #[test]
    fn atom_display_label_empty_path_shows_question_mark() {
        let atom = Atom::Insert { at: vec![], content_hash: [0u8; 32] };
        let label = atom_display_label(&atom);
        assert!(label.contains("?"));
    }

    // --- config_get tests --------------------------------------------------

    #[test]
    fn config_get_returns_user_name() {
        let mut cfg = ArcConfig::default();
        cfg.user.name = Some("Alice".to_string());
        assert_eq!(config_get(&cfg, "user.name"), Some("Alice".to_string()));
    }

    #[test]
    fn config_get_returns_none_for_unset_key() {
        let cfg = ArcConfig::default();
        assert_eq!(config_get(&cfg, "user.name"), None);
    }

    #[test]
    fn config_get_returns_remote() {
        let mut cfg = ArcConfig::default();
        cfg.remotes.insert("origin".to_string(), "https://example.com".to_string());
        assert_eq!(config_get(&cfg, "remotes.origin"), Some("https://example.com".to_string()));
    }

    #[test]
    fn config_get_returns_alias() {
        let mut cfg = ArcConfig::default();
        cfg.aliases.insert("st".to_string(), "status".to_string());
        assert_eq!(config_get(&cfg, "aliases.st"), Some("status".to_string()));
    }

    #[test]
    fn config_get_returns_none_for_unknown_key() {
        let cfg = ArcConfig::default();
        assert_eq!(config_get(&cfg, "nonexistent.key"), None);
    }

    // --- config_set tests --------------------------------------------------

    #[test]
    fn config_set_sets_user_name() {
        let mut cfg = ArcConfig::default();
        config_set(&mut cfg, "user.name", "Bob").unwrap();
        assert_eq!(cfg.user.name, Some("Bob".to_string()));
    }

    #[test]
    fn config_set_sets_remote() {
        let mut cfg = ArcConfig::default();
        config_set(&mut cfg, "remotes.origin", "https://github.com/test").unwrap();
        assert_eq!(cfg.remotes.get("origin").unwrap(), "https://github.com/test");
    }

    #[test]
    fn config_set_sets_alias() {
        let mut cfg = ArcConfig::default();
        config_set(&mut cfg, "aliases.st", "status").unwrap();
        assert_eq!(cfg.aliases.get("st").unwrap(), "status");
    }

    #[test]
    fn config_set_sets_bool_field() {
        let mut cfg = ArcConfig::default();
        config_set(&mut cfg, "snapshot.auto_track", "true").unwrap();
        assert_eq!(cfg.snapshot.auto_track, Some("true".to_string()));
    }

    #[test]
    fn config_set_invalid_bool_returns_error() {
        let mut cfg = ArcConfig::default();
        let err = config_set(&mut cfg, "snapshot.auto_update_stale", "notabool")
            .expect_err("expected error for invalid bool");
        assert!(err.to_string().contains("must be true or false"));
    }

    #[test]
    fn config_set_unknown_key_returns_error() {
        let mut cfg = ArcConfig::default();
        let err =
            config_set(&mut cfg, "unknown.key", "val").expect_err("expected error for unknown key");
        assert!(err.to_string().contains("unknown config key"));
    }

    // --- config_unset tests ------------------------------------------------

    #[test]
    fn config_unset_clears_user_name() {
        let mut cfg = ArcConfig::default();
        cfg.user.name = Some("Alice".to_string());
        config_unset(&mut cfg, "user.name").unwrap();
        assert_eq!(cfg.user.name, None);
    }

    #[test]
    fn config_unset_removes_remote() {
        let mut cfg = ArcConfig::default();
        cfg.remotes.insert("origin".to_string(), "url".to_string());
        config_unset(&mut cfg, "remotes.origin").unwrap();
        assert!(!cfg.remotes.contains_key("origin"));
    }

    #[test]
    fn config_unset_removes_alias() {
        let mut cfg = ArcConfig::default();
        cfg.aliases.insert("st".to_string(), "status".to_string());
        config_unset(&mut cfg, "aliases.st").unwrap();
        assert!(!cfg.aliases.contains_key("st"));
    }

    #[test]
    fn config_unset_unknown_key_returns_error() {
        let mut cfg = ArcConfig::default();
        let err =
            config_unset(&mut cfg, "unknown.key").expect_err("expected error for unknown key");
        assert!(err.to_string().contains("unknown config key"));
    }

    // --- find_command_token_index tests -------------------------------------

    fn command_names_set() -> HashSet<String> {
        Cli::command().get_subcommands().map(|c| c.get_name().to_string()).collect()
    }

    #[test]
    fn find_command_token_index_finds_command() {
        let cfg = ArcConfig::default();
        let names = command_names_set();
        let args = test_raw_args(&["arc", "status"]);
        let idx = find_command_token_index(&args, &cfg, &names);
        assert_eq!(idx, Some(1));
    }

    #[test]
    fn find_command_token_index_skips_flags() {
        let cfg = ArcConfig::default();
        let names = command_names_set();
        let args = test_raw_args(&["arc", "--json", "status"]);
        let idx = find_command_token_index(&args, &cfg, &names);
        assert_eq!(idx, Some(2));
    }

    #[test]
    fn find_command_token_index_returns_none_after_double_dash() {
        let cfg = ArcConfig::default();
        let names = command_names_set();
        let args = test_raw_args(&["arc", "--", "status"]);
        let idx = find_command_token_index(&args, &cfg, &names);
        assert_eq!(idx, None);
    }

    #[test]
    fn find_command_token_index_expands_alias() {
        let mut cfg = ArcConfig::default();
        cfg.aliases.insert("st".to_string(), "status".to_string());
        let names = command_names_set();
        let args = test_raw_args(&["arc", "st"]);
        let idx = find_command_token_index(&args, &cfg, &names);
        assert_eq!(idx, Some(1));
    }

    #[test]
    fn find_command_token_index_too_few_args_returns_none() {
        let cfg = ArcConfig::default();
        let names = command_names_set();
        let args = test_raw_args(&["arc"]);
        let idx = find_command_token_index(&args, &cfg, &names);
        assert_eq!(idx, None);
    }

    #[test]
    fn find_command_token_index_no_command_returns_none() {
        let cfg = ArcConfig::default();
        let names = command_names_set();
        let args = test_raw_args(&["arc", "notarealcommand"]);
        let idx = find_command_token_index(&args, &cfg, &names);
        assert_eq!(idx, None);
    }

    // --- normalize_blob_extension tests -------------------------------------

    #[test]
    fn normalize_blob_extension_strips_leading_dot() {
        assert_eq!(normalize_blob_extension(".rs").unwrap(), "rs");
    }

    #[test]
    fn normalize_blob_extension_lowercases() {
        assert_eq!(normalize_blob_extension("PNG").unwrap(), "png");
    }

    #[test]
    fn normalize_blob_extension_trims_whitespace() {
        assert_eq!(normalize_blob_extension("  js  ").unwrap(), "js");
    }

    #[test]
    fn normalize_blob_extension_allows_hyphen_and_underscore() {
        assert_eq!(normalize_blob_extension("my-ext").unwrap(), "my-ext");
        assert_eq!(normalize_blob_extension("my_ext").unwrap(), "my_ext");
    }

    #[test]
    fn normalize_blob_extension_empty_returns_error() {
        assert!(normalize_blob_extension("").is_err());
    }

    #[test]
    fn normalize_blob_extension_dot_only_returns_error() {
        assert!(normalize_blob_extension(".").is_err());
    }

    #[test]
    fn normalize_blob_extension_invalid_chars_returns_error() {
        assert!(normalize_blob_extension("file name").is_err());
        assert!(normalize_blob_extension("a@b").is_err());
    }

    // --- PolicyError::to_mcp_payload tests ---------------------------------

    #[test]
    fn policy_error_signature_mismatch_payload() {
        let err = PolicyError::SignatureMismatch {
            broken_functions: vec!["read()".into(), "write()".into()],
            old_signature: "fn read(&self)".into(),
            new_signature: "fn read(&mut self)".into(),
        };
        let payload = err.to_mcp_payload();
        assert_eq!(payload["type"], "SignatureMismatch");
        let broken = payload["broken_functions"].as_array().unwrap();
        assert_eq!(broken.len(), 2);
        assert_eq!(broken[0], "read()");
        assert_eq!(broken[1], "write()");
        assert_eq!(payload["old_signature"], "fn read(&self)");
        assert_eq!(payload["new_signature"], "fn read(&mut self)");
    }

    #[test]
    fn policy_error_missing_dependency_payload() {
        let err = PolicyError::MissingDependency { dependency: "tree-sitter-rust".into() };
        let payload = err.to_mcp_payload();
        assert_eq!(payload["type"], "MissingDependency");
        assert_eq!(payload["dependency"], "tree-sitter-rust");
    }

    #[test]
    fn policy_error_read_config_payload() {
        let err = PolicyError::ReadConfig {
            path: "policy.json".into(),
            source: std::io::Error::new(std::io::ErrorKind::PermissionDenied, "permission denied"),
        };
        let payload = err.to_mcp_payload();
        assert_eq!(payload["type"], "ReadConfig");
        assert_eq!(payload["path"], "policy.json");
        assert!(payload["error"].as_str().unwrap().contains("permission denied"));
    }

    #[test]
    fn policy_error_parse_config_payload() {
        let err = PolicyError::ParseConfig {
            path: "policy.json".into(),
            source: serde_json::from_str::<serde_json::Value>("not valid json {{{").unwrap_err(),
        };
        let payload = err.to_mcp_payload();
        assert_eq!(payload["type"], "ParseConfig");
        assert_eq!(payload["path"], "policy.json");
        assert!(payload["error"].as_str().unwrap().contains("expected"));
    }

    // --- InterruptState tests -----------------------------------------------

    #[test]
    fn interrupt_state_new_is_not_interrupted() {
        let state = arc_cli::devtools::interrupt::InterruptState::new();
        assert!(!state.is_interrupted());
    }

    #[test]
    fn interrupt_state_mark_interrupted() {
        let state = arc_cli::devtools::interrupt::InterruptState::new();
        state.mark_interrupted();
        assert!(state.is_interrupted());
    }

    #[test]
    fn interrupt_state_default_not_interrupted() {
        let state = arc_cli::devtools::interrupt::InterruptState::default();
        assert!(!state.is_interrupted());
    }

    #[test]
    fn interrupt_state_clone_shares_atomic() {
        let a = arc_cli::devtools::interrupt::InterruptState::new();
        let b = a.clone();
        a.mark_interrupted();
        assert!(b.is_interrupted());
    }

    #[test]
    fn interrupt_state_two_instances_independent() {
        let a = arc_cli::devtools::interrupt::InterruptState::new();
        let b = arc_cli::devtools::interrupt::InterruptState::new();
        a.mark_interrupted();
        assert!(!b.is_interrupted());
    }

    // --- policy.rs tests ---------------------------------------------------

    #[test]
    fn arc_policy_default_values() {
        let policy = ArcPolicy::default();
        assert!(!policy.require_ghost_node_sponsor);
        assert!(!policy.block_unresolved_sem_breaks);
    }

    #[test]
    fn arc_policy_default_evaluator_returns_non_null() {
        let policy = ArcPolicy::default();
        let evaluator = TreeSitterEvaluator::new(policy);
        let ast = Ast::default();
        let result = evaluator.evaluate_delta_impact(&ast, &[]);
        assert!(result.is_ok());
    }

    #[test]
    fn arc_policy_load_from_missing_file_returns_default() {
        let path = std::path::Path::new("/nonexistent/policy/path/that/does/not/exist.json");
        let policy = ArcPolicy::load_from_path(path).unwrap();
        assert!(!policy.require_ghost_node_sponsor);
        assert!(!policy.block_unresolved_sem_breaks);
    }

    #[test]
    fn arc_policy_load_from_bad_json_returns_parse_error() {
        let dir = std::env::temp_dir().join("arc_test_policy_parse");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("bad.json");
        std::fs::write(&path, "{not valid json").unwrap();
        let err = ArcPolicy::load_from_path(&path).expect_err("expected parse error");
        assert!(err.to_string().contains("failed to parse policy file"));
        let _ = std::fs::remove_file(&path);
    }

    // --- resolver.rs tests -------------------------------------------------

    #[test]
    fn default_evaluator_returns_evaluator_with_default_policy() {
        let policy = ArcPolicy::default();
        let evaluator = TreeSitterEvaluator::new(policy);
        let ast = Ast::default();
        let result = evaluator.evaluate_delta_impact(&ast, &[]);
        assert!(result.is_ok());
    }

    #[test]
    fn ast_context_returns_ast_with_sources() {
        let local = vec!["fn main() {}".to_string()];
        let foreign = vec!["fn helper() {}".to_string()];
        let ctx = ast_context(local.clone(), foreign.clone());
        assert_eq!(ctx.local_rust_sources, local);
        assert_eq!(ctx.foreign_rust_sources, foreign);
    }

    #[test]
    fn ast_context_empty_sources() {
        let ctx = ast_context(vec![], vec![]);
        assert!(ctx.local_rust_sources.is_empty());
        assert!(ctx.foreign_rust_sources.is_empty());
        assert!(ctx.expected_api_signatures.is_empty());
    }

    #[test]
    fn mock_ai_resolver_synthesize_lens_with_signature_mismatch() {
        let resolver = MockAiResolver;
        let err = PolicyError::SignatureMismatch {
            broken_functions: vec!["read".into()],
            old_signature: "fn read(&self)".into(),
            new_signature: "fn read(&mut self)".into(),
        };
        let result = resolver.synthesize_lens(&err);
        let atoms = result.unwrap();
        assert_eq!(atoms.len(), 1);
    }

    #[test]
    fn mock_ai_resolver_synthesize_lens_with_missing_dep() {
        let resolver = MockAiResolver;
        let err = PolicyError::MissingDependency { dependency: "foo".into() };
        let result = resolver.synthesize_lens(&err);
        // MissingDependency has no broken_functions key, so synthesis fails
        assert!(result.is_err());
    }

    #[test]
    fn verify_lens_with_no_breaks() {
        let policy = ArcPolicy::default();
        let evaluator = TreeSitterEvaluator::new(policy);
        let local_ast = Ast::default();
        let result = verify_lens(&evaluator, &local_ast, &[], &[]);
        assert!(result.is_ok());
    }

    #[test]
    fn verify_lens_with_empty_lens_atoms() {
        let policy = ArcPolicy::default();
        let evaluator = TreeSitterEvaluator::new(policy);
        let local_ast = Ast::default();
        let incoming =
            vec![Atom::Insert { at: vec!["file.rs".to_string()], content_hash: [0u8; 32] }];
        let result = verify_lens(&evaluator, &local_ast, &incoming, &[]);
        assert!(result.is_ok());
    }

    #[test]
    fn default_evaluator_fn_returns_working_evaluator() {
        let evaluator = default_evaluator();
        let ast = Ast::default();
        let result = evaluator.evaluate_delta_impact(&ast, &[]);
        assert!(result.is_ok());
    }

    // --- OperationStage (extend existing) ----------------------------------

    #[test]
    fn operation_stage_all_variants_display() {
        let stages = [
            OperationStage::Discover,
            OperationStage::Negotiate,
            OperationStage::Transfer,
            OperationStage::Materialize,
            OperationStage::Finalize,
        ];
        for stage in stages {
            let s = format!("{}", stage);
            assert!(!s.is_empty());
        }
    }

    #[test]
    fn operation_stage_as_str_matches_display() {
        let stages = [
            OperationStage::Discover,
            OperationStage::Negotiate,
            OperationStage::Transfer,
            OperationStage::Materialize,
            OperationStage::Finalize,
        ];
        for stage in stages {
            assert_eq!(stage.as_str(), stage.to_string());
        }
    }

    #[test]
    fn slo_timer_finish_returns_positive_duration() {
        let timer = SloTimer::new("test-op", std::time::Duration::from_secs(10));
        std::thread::sleep(std::time::Duration::from_millis(10));
        let elapsed = timer.finish();
        assert!(elapsed.as_millis() >= 10, "elapsed must be at least 10ms");
    }

    #[test]
    fn slo_timer_stage_runs_closure() {
        let timer = SloTimer::new("stage-test", std::time::Duration::from_secs(10));
        let result = timer.stage(OperationStage::Transfer, || 42);
        assert_eq!(result, 42);
    }

    #[test]
    fn default_slo_ms_constant() {
        assert_eq!(DEFAULT_SYNC_SLO_MS, 500);
    }

    // --- evaluator.rs query constants tests --------------------------------

    #[test]
    fn rust_boundary_query_is_non_empty() {
        assert!(!RUST_BOUNDARY_QUERY.is_empty());
        assert!(RUST_BOUNDARY_QUERY.contains("function_item"));
    }

    #[test]
    fn rust_invocation_query_is_non_empty() {
        assert!(!RUST_INVOCATION_QUERY.is_empty());
        assert!(RUST_INVOCATION_QUERY.contains("call_expression"));
    }

    // --- evaluator.rs evaluate_delta_impact edge cases ----------------------

    #[test]
    fn evaluator_rejects_mount_with_ghost_sponsor_policy() {
        let policy =
            ArcPolicy { require_ghost_node_sponsor: true, block_unresolved_sem_breaks: false };
        let evaluator = TreeSitterEvaluator::new(policy);
        let ast = Ast::default();
        let atoms = vec![Atom::Mount {
            path: vec!["some".to_string(), "path".to_string()],
            coordinate: arc_algebra_types::SpacetimeCoordinate {
                namespace: "ns".into(),
                repo: "repo".into(),
                hash: blake3::Hash::from_bytes([1u8; 32]),
            },
        }];
        let result = evaluator.evaluate_delta_impact(&ast, &atoms);
        assert!(result.is_err());
        match result.unwrap_err() {
            PolicyError::SignatureMismatch { broken_functions, .. } => {
                assert!(broken_functions.contains(&"<ghost-node-sponsor>".to_string()));
            }
            other => panic!("expected SignatureMismatch, got {:?}", other),
        }
    }

    #[test]
    fn evaluator_allows_mount_without_ghost_sponsor_policy() {
        let policy =
            ArcPolicy { require_ghost_node_sponsor: false, block_unresolved_sem_breaks: false };
        let evaluator = TreeSitterEvaluator::new(policy);
        let ast = Ast::default();
        let atoms = vec![Atom::Mount {
            path: vec!["some".to_string(), "path".to_string()],
            coordinate: arc_algebra_types::SpacetimeCoordinate {
                namespace: "ns".into(),
                repo: "repo".into(),
                hash: blake3::Hash::from_bytes([1u8; 32]),
            },
        }];
        let result = evaluator.evaluate_delta_impact(&ast, &atoms);
        assert!(result.is_ok());
    }

    #[test]
    fn evaluator_blocks_sem_breaks_with_empty_sources() {
        let policy =
            ArcPolicy { require_ghost_node_sponsor: false, block_unresolved_sem_breaks: true };
        let evaluator = TreeSitterEvaluator::new(policy);
        let ast = Ast::default();
        let result = evaluator.evaluate_delta_impact(&ast, &[]);
        assert!(result.is_ok());
    }

    // ── AnyhowCompat ──────────────────────────────────────────────

    #[test]
    fn anyhow_compat_display_uses_alternate() {
        let inner = anyhow::anyhow!("test error message");
        let compat = AnyhowCompat(inner);
        assert_eq!(format!("{compat}"), "test error message");
    }

    #[test]
    fn anyhow_compat_display_with_context() {
        let inner = anyhow::anyhow!("outer").context("inner context");
        let compat = AnyhowCompat(inner);
        let s = format!("{compat}");
        assert!(s.contains("inner context"));
        assert!(s.contains("outer"));
    }

    #[test]
    fn anyhow_compat_implements_std_error() {
        let compat = AnyhowCompat(anyhow::anyhow!("e"));
        let e: &dyn std::error::Error = &compat;
        assert!(e.to_string().contains("e"));
    }

    #[test]
    fn anyhow_compat_debug_derive() {
        let compat = AnyhowCompat(anyhow::anyhow!("debug test"));
        let dbg = format!("{compat:?}");
        assert!(dbg.contains("AnyhowCompat"));
    }

    // ── make_renderer ─────────────────────────────────────────────

    #[test]
    fn make_renderer_human_rich() {
        let caps = TerminalCapabilities {
            mode: RenderMode::HumanRich,
            supports_osc8: true,
            supports_unicode: true,
        };
        let mut renderer = make_renderer(&caps);
        renderer.render(OutputEvent::Started("test".to_string()));
    }

    #[test]
    fn make_renderer_human_plain() {
        let caps = TerminalCapabilities {
            mode: RenderMode::HumanPlain,
            supports_osc8: false,
            supports_unicode: false,
        };
        let mut renderer = make_renderer(&caps);
        renderer.render(OutputEvent::Started("test".to_string()));
    }

    #[test]
    fn make_renderer_machine_json() {
        let caps = TerminalCapabilities {
            mode: RenderMode::MachineJson,
            supports_osc8: false,
            supports_unicode: true,
        };
        let mut renderer = make_renderer(&caps);
        renderer.render(OutputEvent::Started("test".to_string()));
    }

    // ── render_diagnostic_error ───────────────────────────────────

    #[test]
    fn render_diagnostic_error_plain_does_not_panic() {
        let err = anyhow::anyhow!("unit-test diagnostic");
        render_diagnostic_error(err, false, false);
    }

    #[test]
    fn render_diagnostic_error_json_does_not_panic() {
        let err = anyhow::anyhow!("json diagnostic test");
        render_diagnostic_error(err, true, false);
    }

    #[test]
    fn render_diagnostic_error_quiet_does_not_panic() {
        let err = anyhow::anyhow!("quiet diagnostic test");
        render_diagnostic_error(err, false, true);
    }

    #[test]
    fn render_diagnostic_error_with_context_does_not_panic() {
        let err = anyhow::anyhow!("root cause").context("wrapper context");
        render_diagnostic_error(err, false, false);
    }

    // ── resolve_sync_token ────────────────────────────────────────

    #[test]
    fn resolve_sync_token_env_var_scenarios() {
        // SAFETY: env var manipulation is process-global; this single test
        // serialises all scenarios to avoid parallel-test races.
        let run = |val: &str| -> Option<String> {
            // SAFETY: single-threaded test; env manipulation is safe here
            unsafe { std::env::set_var("ARC_SYNC_TOKEN", val) };
            let r = resolve_sync_token().unwrap();
            // SAFETY: single-threaded test; restoring env to clean state
            unsafe { std::env::remove_var("ARC_SYNC_TOKEN") };
            r
        };

        // normal value
        assert_eq!(run("test-token-abc123"), Some("test-token-abc123".into()));
        // whitespace-only → treated as empty
        assert_eq!(run("   \t\n  "), None);
        // empty string
        assert_eq!(run(""), None);
        // leading/trailing whitespace preserved (not trimmed by the function)
        assert_eq!(run("  my-token  "), Some("  my-token  ".into()));
    }

    // ── detect_capabilities ───────────────────────────────────────

    #[test]
    fn detect_capabilities_force_json() {
        let caps = detect_capabilities(true, false);
        assert_eq!(caps.mode, RenderMode::MachineJson);
    }

    #[test]
    fn detect_capabilities_quiet_plain() {
        let caps = detect_capabilities(false, true);
        assert_eq!(caps.mode, RenderMode::HumanPlain);
    }

    #[test]
    fn detect_capabilities_force_json_overrides_quiet() {
        let caps = detect_capabilities(true, true);
        assert_eq!(caps.mode, RenderMode::MachineJson);
    }

    // ── hyperlink_for_hash ────────────────────────────────────────

    #[test]
    fn hyperlink_for_hash_no_osc8_returns_bare_hash() {
        let hash = "abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890";
        let result = hyperlink_for_hash(hash, false);
        assert_eq!(result, hash);
    }

    #[test]
    fn hyperlink_for_hash_with_osc8_returns_osc8_link() {
        let hash = "abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890";
        let result = hyperlink_for_hash(hash, true);
        assert!(result.starts_with("\x1B]8;;"));
        assert!(result.contains(hash));
        assert!(result.ends_with("\x1B\\"));
    }

    // ── hyperlink_for_path ────────────────────────────────────────

    #[test]
    fn hyperlink_for_path_no_osc8_returns_bare_path() {
        let result = hyperlink_for_path("src/main.rs", 42, false);
        assert_eq!(result, "src/main.rs:42");
    }

    #[test]
    fn hyperlink_for_path_with_osc8_returns_osc8_link() {
        let result = hyperlink_for_path("src/main.rs", 42, true);
        assert!(result.starts_with("\x1B]8;;"));
        assert!(result.contains("src/main.rs:42"));
        assert!(result.ends_with("\x1B\\"));
    }

    // ── format_error_code ─────────────────────────────────────────

    #[test]
    fn format_error_code_zero_clamps_to_e0001() {
        assert_eq!(arc_ux::diagnostics::format_error_code(0), "E0001");
    }

    #[test]
    fn format_error_code_one() {
        assert_eq!(arc_ux::diagnostics::format_error_code(1), "E0001");
    }

    #[test]
    fn format_error_code_999() {
        assert_eq!(arc_ux::diagnostics::format_error_code(999), "E0999");
    }

    #[test]
    fn format_error_code_1000_clamps() {
        assert_eq!(arc_ux::diagnostics::format_error_code(1000), "E1000");
    }

    #[test]
    fn format_error_code_over_1000_clamps() {
        assert_eq!(arc_ux::diagnostics::format_error_code(5000), "E1000");
    }

    // ── diagnostic_lines ──────────────────────────────────────────

    #[test]
    fn diagnostic_lines_returns_non_empty_for_error() {
        let err = anyhow::anyhow!("simple error");
        let lines = diagnostic_lines(&err);
        assert!(!lines.is_empty());
        assert!(lines[0].contains("simple error"));
    }

    #[test]
    fn diagnostic_lines_includes_causes() {
        let err = anyhow::anyhow!("root").context("middle").context("top");
        let lines = diagnostic_lines(&err);
        assert!(lines.len() >= 2);
    }

    // ── OutputEvent rendering round-trips ─────────────────────────

    #[test]
    fn output_event_progress_rendering() {
        let caps = detect_capabilities(false, true);
        let mut renderer = make_renderer(&caps);
        renderer.render(OutputEvent::Progress(1, 5, "doing stuff".to_string()));
    }

    #[test]
    fn output_event_warning_rendering() {
        let caps = detect_capabilities(false, true);
        let mut renderer = make_renderer(&caps);
        renderer.render(OutputEvent::Warning("heads up".to_string()));
    }

    #[test]
    fn output_event_success_with_details() {
        let caps = detect_capabilities(false, true);
        let mut renderer = make_renderer(&caps);
        renderer.render(OutputEvent::Success(
            "done".to_string(),
            vec!["detail1".to_string(), "detail2".to_string()],
        ));
    }

    #[test]
    fn output_event_diagnostic_rendering() {
        use arc_ux::diagnostics::arc_error_to_report;
        let arc_err =
            arc_error::Error::from_error(AnyhowCompat(anyhow::anyhow!("ux diagnostic test")));
        let report = arc_error_to_report(arc_err);
        let caps = detect_capabilities(false, true);
        let mut renderer = make_renderer(&caps);
        renderer.render(OutputEvent::Diagnostic(report));
    }

    // ── cli_styles ────────────────────────────────────────────────

    #[test]
    fn cli_styles_returns_configured_styles() {
        let _styles = cli_styles();
    }

    // ── run_mock_snap_demo ────────────────────────────────────────

    #[test]
    fn run_mock_snap_demo_plain_does_not_panic() {
        let caps = detect_capabilities(false, true);
        run_mock_snap_demo(&caps);
    }

    #[test]
    fn run_mock_snap_demo_json_does_not_panic() {
        let caps = detect_capabilities(true, true);
        run_mock_snap_demo(&caps);
    }

    // ── Atom::Mount and Atom::Conflict display coverage ───────────

    #[test]
    fn atom_display_label_mount_shows_mount() {
        use arc_algebra_types::SpacetimeCoordinate;
        let atom = Atom::Mount {
            path: vec!["lib".to_string(), "core".to_string()],
            coordinate: SpacetimeCoordinate {
                namespace: "ns".to_string(),
                repo: "repo".to_string(),
                hash: blake3::Hash::from_bytes([0xAA; 32]),
            },
        };
        let label = atom_display_label(&atom);
        assert!(label.contains("Mount"));
        assert!(label.contains("core"));
    }

    #[test]
    fn atom_display_label_conflict_shows_conflict() {
        let atom = Atom::Conflict {
            at: vec!["lib.rs".to_string()],
            bases: vec![[0xAA; 32]],
            sides: vec![[0xBB; 32], [0xCC; 32]],
        };
        let label = atom_display_label(&atom);
        assert!(label.contains("Conflict"));
        assert!(label.contains("lib.rs"));
    }

    #[test]
    fn atom_display_label_move_shows_arrow() {
        let atom = Atom::Move { from: vec!["old.rs".to_string()], to: vec!["new.rs".to_string()] };
        let label = atom_display_label(&atom);
        assert!(label.contains("Moved"));
        assert!(label.contains("old.rs"));
        assert!(label.contains("new.rs"));
    }

    #[test]
    fn atom_display_label_semantics_preserving_shows_reformat() {
        let atom = Atom::SemanticsPreserving {
            at: vec!["style.rs".to_string()],
            description: "reformat".to_string(),
        };
        let label = atom_display_label(&atom);
        assert!(label.contains("Reformat"));
        assert!(label.contains("style.rs"));
        assert!(label.contains("reformat"));
    }
}
