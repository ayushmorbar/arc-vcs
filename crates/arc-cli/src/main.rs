use anyhow::Context as _;
use clap::{Parser, Subcommand};
use tracing_subscriber::EnvFilter;

use arc_cli::interop::git::import_repo;
use arc_cli::repo::{
    ArcConfig, Repository, load_merged_config, save_global_config, save_local_config,
};
use arc_cli::sync::{fetch, pull};
use arc_net::ai::build_provider;
use arc_core::algebra::Blake3Hash;
use arc_core::algebra::apply::MaterializedState;
use arc_core::store::author::{Author, load_identity, save_identity};
use arc_core::store::oplog::OperationAgent;
use arc_core::store::view::View;
use arc_git_bridge::http::{discover_refs, push_packfile};
use arc_git_bridge::object::GitIdentity;
use arc_git_bridge::pack::encode_packfile;
use arc_git_bridge::translator::{CommitCompileInput, GitMap, GitOdb, compile_commit, compile_tree};
use arc_lang::ast::{LanguagePlugin, rust_plugin::RustPlugin};
use comfy_table::{Cell, Color, Table, presets};
use owo_colors::OwoColorize;

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
/// 2. Generate a fresh keypair whose `session_id` comes from the
///    `ARC_EPHEMERAL_RUNNER` environment variable (or the OS process ID as a
///    fallback), then persist it for subsequent commands.
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
        let key = ed25519_dalek::SigningKey::from_bytes(&s.secret_key_bytes)
            .verifying_key()
            .to_bytes();
        let author = Author::Transient {
            session_id: s.session_id,
            key,
        };
        (author, s.secret_key_bytes)
    } else {
        let session_id = std::env::var("ARC_EPHEMERAL_RUNNER")
            .unwrap_or_else(|_| format!("ephemeral-{}", std::process::id()));
        let (author, seed) = arc_core::store::author::generate_transient_keypair_seed(&session_id);
        if let Some(parent) = session_path.parent() {
            std::fs::create_dir_all(parent).context("failed to create .arc/local/ directory")?;
        }
        let record = EphemeralSession {
            session_id,
            secret_key_bytes: seed,
        };
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
                "No cryptographic identity found. \
                 Run 'arc auth generate' to create one, or set \
                 ARC_EPHEMERAL_RUNNER for CI/CD pipelines."
            )
        })
    }
}

#[derive(Parser)]
#[command(name = "arc", version, about = "Atomic Replayable Changes")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Initialize a new arc repository.
    Init {
        /// Directory to initialize (defaults to current directory).
        path: Option<String>,
        /// Skip auto-detection of an existing Git repository.
        #[arg(long)]
        no_git: bool,
    },
    /// Snapshot the working directory into a semantic change.
    Snap {
        /// Description of the change.  Required unless `--auto-msg` is given.
        #[arg(short, long, required_unless_present = "auto_msg")]
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
    /// Show the change log.
    Log {
        /// Semantic search query to filter changes by intent similarity.
        /// Requires the local embedding model (downloaded on first use).
        #[arg(long)]
        intent: Option<String>,
        /// Revset query expression used to select which changes to show.
        #[arg(short = 'r', long, default_value = "ancestors(@)")]
        revset: String,
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
    Verify,
    /// Manage arc identity (cryptographic key-pair).
    Auth {
        #[command(subcommand)]
        action: AuthAction,
    },
    /// Start an HTTP server serving the current repository over TCP.
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
    /// Undo the last view-mutating operation using the operation log (O(1) pointer-swap).
    Undo,
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
    /// Interactively edit the AST content of an existing change.
    ///
    /// Two-step workflow:
    ///  1. `arc diffedit --prepare <change>` — check out the change's state
    ///     to the working dir.
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
}

#[derive(Subcommand)]
enum OpAction {
    /// Print the operation log in reverse-chronological order.
    Log,
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
    Approve,
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
enum SparseAction {
    /// Set the sparse cone to the given path prefixes (e.g. `frontend/`).
    ///
    /// Files outside the cone are removed from disk; the DAG is unaffected.
    Set {
        /// One or more path prefixes to include in the sparse cone.
        #[arg(required = true)]
        paths: Vec<String>,
    },
    /// List the active sparse cone patterns.
    List,
    /// Remove the sparse filter and restore the full working directory.
    Reset,
}

#[derive(Subcommand)]
enum MountAction {
    /// Declare a sub-repository mount in the current view.
    Add {
        /// Local path at which to mount the sub-repository.
        #[arg(long)]
        path: String,
        /// URL or filesystem path of the remote `arc` repository.
        #[arg(long)]
        url: String,
        /// View name to check out inside the mounted sub-repository.
        #[arg(long)]
        target: String,
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
    /// Print all configuration values (global + local merged).
    List,
}

fn init_tracing() {
    if let Ok(path) = std::env::var("ARC_TRACE_EVENT") {
        if let Ok(file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
        {
            let _ = tracing_subscriber::fmt()
                .json()
                .with_writer(std::sync::Mutex::new(file))
                .with_env_filter(EnvFilter::new("arc_cli=debug,info"))
                .try_init();
        }
    } else if std::env::var("ARC_TRACE").is_ok_and(|v| v == "1") {
        let _ = tracing_subscriber::fmt()
            .compact()
            .with_env_filter(EnvFilter::new("arc_cli=debug,info"))
            .try_init();
    }
    // Default: no subscriber installed — tracing macros are zero-overhead.
}

fn git_identity_from_author(author: &Author) -> GitIdentity {
    let (name, email) = match author {
        Author::Human { name, email, .. } => (name.clone(), email.clone()),
        Author::AI { model, .. } => (format!("AI {model}"), "ai@arc.local".to_string()),
        Author::Server { canonical_id, .. } => {
            (canonical_id.clone(), "server@arc.local".to_string())
        }
        Author::Transient { session_id, .. } => {
            (session_id.clone(), "transient@arc.local".to_string())
        }
    };

    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);

    GitIdentity {
        name,
        email,
        timestamp,
        timezone: "+0000".to_string(),
    }
}

fn projected_files_from_state(
    state: &MaterializedState,
) -> anyhow::Result<std::collections::HashMap<String, String>> {
    let mut filepaths = std::collections::HashSet::new();
    for key in state.keys() {
        if key.len() >= 2 && key[0] == "file" {
            filepaths.insert(key[1].clone());
        }
    }

    let plugin = RustPlugin::new();
    let mut files = std::collections::HashMap::new();
    for filepath in filepaths {
        let content = if filepath.ends_with(".rs") {
            plugin
                .unparse(state, &filepath)
                .map_err(|e| anyhow::anyhow!("failed to render '{filepath}' from state: {e}"))?
        } else {
            let key = vec!["file".to_string(), filepath.clone()];
            let bytes = state
                .get(&key)
                .ok_or_else(|| anyhow::anyhow!("missing file payload for '{filepath}'"))?;
            std::str::from_utf8(bytes)
                .map(|s| s.to_string())
                .map_err(|_| anyhow::anyhow!("cannot export non-UTF-8 file '{filepath}' to Git"))?
        };
        files.insert(filepath, content);
    }

    Ok(files)
}

fn main() -> anyhow::Result<()> {
    // Initialise the tempfile registry eagerly so no allocations happen inside
    // a signal handler later.
    arc_core::store::tempfile::init();

    // Register UNIX termination signals to trigger cleanup before exit.
    // On Windows, tempfile cleanup happens via Drop — no signal handler needed.
    #[cfg(unix)]
    {
        use signal_hook::consts::TERM_SIGNALS;
        for &sig in TERM_SIGNALS {
            // SAFETY: The handler only calls `remove_file` (unlink syscall)
            // and iterates a DashMap.  No memory allocation or non-signal-safe
            // operations are used inside `cleanup_signal_safe`.
            unsafe {
                signal_hook::low_level::register(sig, || {
                    arc_core::store::tempfile::cleanup_signal_safe();
                })?;
            }
        }
    }

    init_tracing();
    // --- Single-pass alias interception (no recursion) -------------------
    let mut raw_args: Vec<String> = std::env::args().collect();
    if raw_args.len() >= 2
        && let Ok(config) = load_merged_config(std::path::Path::new("."))
        && let Some(expansion) = config.aliases.get(&raw_args[1]).cloned()
        && let Some(expanded) = shlex::split(&expansion)
    {
        let rest = raw_args.split_off(2);
        raw_args.truncate(1); // keep argv[0] (program name)
        raw_args.extend(expanded);
        raw_args.extend(rest);
    }
    let cli = Cli::parse_from(&raw_args);

    match cli.command {
        Command::Init { path, no_git } => {
            let target = path.unwrap_or_else(|| ".".to_string());
            let target_path = std::path::Path::new(&target);

            // --- Git auto-detection (Phase D) ---
            let do_import = if !no_git {
                match arc_core::git_bridge::resolve_git_dir(target_path) {
                    Ok(_git_dir) => {
                        // Count commits for the prompt.
                        let count = arc_core::git_bridge::analyze_git_repo(target_path)
                            .map(|a| a.commit_count)
                            .unwrap_or(0);
                        eprint!(
                            "Detected Git repository with {count} commit{}. \
                             Import history as arc Changes? [Y/n] ",
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
            println!("Initialized empty arc repository in {target}/.arc");

            if do_import {
                // --- Trust Anchor identity flow ---
                let (author, signing_key) = match load_identity() {
                    Ok(pair) => pair,
                    Err(_) => {
                        // No identity yet: read git user.name/email as defaults.
                        let (git_name, git_email) =
                            arc_core::git_bridge::read_git_user_config(target_path)
                                .unwrap_or_else(|| ("arc user".into(), "".into()));
                        eprintln!(
                            "No arc cryptographic identity found.\n\
                             Generating Ed25519 keypair for {git_name} <{git_email}>\n\
                             Press Enter to confirm, or Ctrl-C to abort."
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
                    "Imported {n} change{} across all branches.\n\
                     Note: Rust source files imported semantically; \
                     other file types imported as blobs.",
                    if n == 1 { "" } else { "s" }
                );
            }
        }
        Command::Snap {
            message,
            auto_msg,
            interactive,
        } => {
            use std::io::Write;

            let mut repo = Repository::open(".")?;
            let (author, signing_key) = load_identity_with_ephemeral_fallback(&repo.shared_root)?;
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

                let rt = tokio::runtime::Runtime::new().context("failed to start async runtime")?;
                match rt.block_on(arc_core::ai::generate_message(&diff_summary)) {
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
            } else {
                // --message is enforced by clap (required_unless_present = "auto_msg")
                // so unwrap_or is a pure safety net here.
                message.unwrap_or_else(|| "WIP".to_owned())
            };

            if interactive {
                eprintln!(
                    "arc: --interactive is deprecated in auto-snapshot mode and is currently ignored"
                );
            }

            if !repo.snapshot()? {
                println!("Nothing to snap — working directory matches history.");
            } else {
                let id = repo.finalize_snapshot(&final_message)?;
                let _ = repo.fork_empty_snapshot()?;
                let hex: String = id.iter().map(|b| format!("{b:02x}")).collect();
                println!("snap {hex}");
            }
        }
        Command::Log { intent, revset } => {
            let mut repo = Repository::open(".")?;
            let (author, signing_key) = load_identity_with_ephemeral_fallback(&repo.shared_root)?;
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
                        let hex: String = change.id.iter().map(|b| format!("{b:02x}")).collect();
                        let author_str = match &change.author {
                            Author::Human { name, email, .. } => format!("{name} <{email}>"),
                            Author::AI {
                                model,
                                human_sponsor,
                            } => {
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
                let expr = arc_core::revset::parse(&revset)
                    .map_err(|e| anyhow::anyhow!("invalid revset '{}': {e}", revset))?;
                repo.prepare_revset(&expr)?;
                let graph = repo.graph_snapshot();
                let mut symbol_resolver = |symbol: &str| repo.resolve_revset_symbol(symbol);
                let rev_iter = arc_core::revset::compile(&expr, graph, &mut symbol_resolver)?;

                let mut table = Table::new();
                table.load_preset(presets::NOTHING);
                let mut printed = 0usize;

                for id in rev_iter {
                    let change = repo.read_change(&id)?;
                    let hex: String = change.id.iter().map(|b| format!("{b:02x}")).collect();
                    let author_str = match &change.author {
                        Author::Human { name, email, .. } => {
                            format!("{name} <{email}>")
                        }
                        Author::AI {
                            model,
                            human_sponsor,
                        } => {
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
                        Cell::new(&author_str).fg(Color::Magenta),
                        Cell::new(&change.intent),
                    ]);
                    printed += 1;
                }

                if printed == 0 {
                    println!("No changes yet. Use 'arc snap' to create your first change.");
                } else {
                    println!("{table}");
                }
            }
        }
        Command::Status => {
            let mut repo = Repository::open(".")?;
            let (author, signing_key) = load_identity_with_ephemeral_fallback(&repo.shared_root)?;
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
                    let hash_hex: String = change.id.iter().map(|b| format!("{b:02x}")).collect();
                    let short_hash = &hash_hex[..8];
                    let author_str = match &change.author {
                        Author::Human { name, email, .. } => {
                            format!("{name} <{email}>")
                        }
                        Author::AI {
                            model,
                            human_sponsor,
                        } => {
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
                    let sig_status = if change.verify_signature() {
                        "[verified]"
                    } else {
                        "[UNVERIFIED]"
                    };
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
                let name = list
                    .last()
                    .cloned()
                    .ok_or_else(|| anyhow::anyhow!("no stash found"))?;
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

                let cfg = load_merged_config(std::path::Path::new("."))?;
                let provider_name = cfg
                    .ai
                    .provider
                    .as_deref()
                    .unwrap_or("openai-compatible");
                let model = cfg
                    .ai
                    .model
                    .clone()
                    .unwrap_or_else(|| "gpt-4o-mini".to_string());
                let endpoint = cfg.ai.endpoint.clone();
                let api_key = std::env::var("ARC_AI_API_KEY").map_err(|_| {
                    anyhow::anyhow!(
                        "ARC_AI_API_KEY is required for 'arc ai resolve' and is read only at runtime"
                    )
                })?;

                let provider = build_provider(provider_name, &model, endpoint, api_key)?;
                eprintln!(
                    "[arc] Using AI provider '{}' with model '{}'.",
                    provider_name, model
                );

                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()?;
                rt.block_on(repo.resolve_conflict_with_provider(provider.as_ref(), &model))?;

                println!(
                    "[arc] Resolution staged as Ghost Node. \
                     Review changes then run 'arc ai approve'."
                );
            }
            AiAction::Approve => {
                let mut repo = Repository::open(".")?;
                let (author, signing_key) = load_identity()?;
                let id = repo.approve_pending_ai(&author, &signing_key)?;
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
        Command::Verify => {
            let mut repo = Repository::open(".")?;
            let name = repo.current_view_name()?;
            repo.hydrate(&name)?;
            repo.verify_graph()?;
            println!("Graph cryptographic provenance verified.");
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
                    Author::AI {
                        model,
                        human_sponsor,
                    } => {
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
        Command::Serve { port } => {
            let rt = tokio::runtime::Runtime::new()?;
            rt.block_on(arc_net::server::serve(port))?;
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
                    let sig = if t.verify() {
                        "[verified]"
                    } else {
                        "[UNVERIFIED]"
                    };
                    println!("{} {} {sig}", t.name, &h[..8]);
                }
            }
        }
        Command::Revert { hash } => {
            let mut repo = Repository::open(".")?;
            let (author, signing_key) = load_identity()?;
            repo.set_identity(author, signing_key);
            let hash_bytes = repo.resolve_rev(&hash)?;
            let target_hex: String = hash_bytes.iter().map(|b| format!("{b:02x}")).collect();
            let revert_id = repo.revert(&hash_bytes)?;
            let revert_hex: String = revert_id.iter().map(|b| format!("{b:02x}")).collect();
            println!(
                "Reverted {} \u{2192} new change {}",
                &target_hex[..8],
                &revert_hex[..8]
            );
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
            println!("Hint: arc uses 'snap' instead of 'commit'. Try: arc snap -m \"<message>\"");
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
                        let agent_label = match op.agent {
                            OperationAgent::Human => op.agent.label().to_string(),
                            OperationAgent::Ai => op.agent.label().cyan().to_string(),
                        };
                        table.add_row(vec![
                            Cell::new(&op.id).fg(Color::Cyan),
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
        },
        Command::Sparse { action } => match action {
            SparseAction::Set { paths } => {
                let mut repo = Repository::open(".")?;
                repo.apply_sparse(&paths)?;
                println!("Sparse cone set to: {}", paths.join(", "));
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
        Command::Mount { action } => match action {
            MountAction::Add { path, url, target } => {
                let mut repo = Repository::open(".")?;
                let (author, signing_key) = load_identity()?;
                repo.set_identity(author, signing_key);
                let id = repo.mount_add(&path, &url, &target)?;
                let hex: String = id.iter().map(|b| format!("{b:02x}")).collect();
                println!("Mounted '{path}' → {url}@{target}  (change {})", &hex[..8]);
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
        },
        Command::Gc { dry_run } => {
            if dry_run {
                println!("(dry-run) GC would analyse the CAS — run without --dry-run to delete.");
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
            println!("Successfully compacted causally stable history into new base state: {hex}");
        }
        Command::Amend { message } => {
            let mut repo = Repository::open(".")?;
            let (author, signing_key) = load_identity()?;
            repo.set_identity(author, signing_key);
            let new_id = repo.amend(message.as_deref())?;
            let hex: String = new_id.iter().map(|b| format!("{b:02x}")).collect();
            println!("Amended → {}", &hex[..8]);
        }
        Command::Squash { into } => {
            let mut repo = Repository::open(".")?;
            let (author, signing_key) = load_identity()?;
            repo.set_identity(author, signing_key);
            let new_id = repo.squash_into(&into)?;
            let hex: String = new_id.iter().map(|b| format!("{b:02x}")).collect();
            println!("Squashed → {}", &hex[..8]);
        }
        Command::Diffedit {
            prepare,
            apply,
            message,
        } => {
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
                "Identity configured: {name} <{email}> (Ed25519 keypair active)\n\
                 Run 'arc auth whoami' to inspect your public key."
            );
        }
        Command::Diff { semantic } => {
            let mut repo = Repository::open(".")?;
            let (author, signing_key) = load_identity_with_ephemeral_fallback(&repo.shared_root)?;
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
            let remote = config
                .remotes
                .get(&remote_url)
                .cloned()
                .unwrap_or(remote_url.clone());

            if !(remote.starts_with("http://") || remote.starts_with("https://")) {
                arc_cli::sync::push(&mut repo, &remote, &view_name)?;
                println!("Pushed '{}' \u{2192} {}.", view_name, remote);
            } else {
                if view_name == current_view {
                    let (author, signing_key) =
                        load_identity_with_ephemeral_fallback(&repo.shared_root)?;
                    repo.set_identity(author, signing_key);
                    let _ = repo.snapshot()?;
                }

                repo.hydrate(&view_name)?;

                let state = repo.materialize(&view_name)?;
                let projected_files = projected_files_from_state(&state)?;

                let view = View::load(&repo.shared_root, &view_name)
                    .map_err(|e| anyhow::anyhow!("failed to load view '{view_name}': {e}"))?;
                if view.heads.len() != 1 {
                    anyhow::bail!(
                        "git export requires exactly one head on view '{}'; found {}",
                        view_name,
                        view.heads.len()
                    );
                }

                let head = *view
                    .heads
                    .iter()
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("view '{}' has no head to push", view_name))?;

                let graph = repo.graph.load_full();
                let change = graph
                    .get(&head)
                    .ok_or_else(|| anyhow::anyhow!("head change missing from graph"))?;

                let mut odb = GitOdb::default();
                let mut map = GitMap::default();
                let tree_id = compile_tree(&projected_files, &mut odb)?;

                let ref_name = format!("refs/heads/{view_name}");
                let rt =
                    tokio::runtime::Runtime::new().context("failed to create tokio runtime")?;
                let refs = rt.block_on(discover_refs(&remote))?;
                let old_sha_hex = refs
                    .get(&ref_name)
                    .cloned()
                    .unwrap_or_else(|| "0000000000000000000000000000000000000000".to_string());

                let parent_ids = if let Some(parent) =
                    arc_git_bridge::object::GitSha1::from_hex(&old_sha_hex)
                {
                    if old_sha_hex.chars().all(|c| c == '0') {
                        Vec::new()
                    } else {
                        vec![parent]
                    }
                } else {
                    anyhow::bail!(
                        "remote ref '{}' returned invalid object id '{}'",
                        ref_name,
                        old_sha_hex
                    );
                };

                let ident = git_identity_from_author(&change.author);
                let new_commit = compile_commit(
                    CommitCompileInput {
                        change,
                        root_tree: tree_id,
                        parent_commits: &parent_ids,
                        author: &ident,
                        committer: &ident,
                        projected_state_has_conflict: false,
                    },
                    &mut odb,
                    &mut map,
                )?;

                let pack_objects = odb.pack_objects();
                let pack = encode_packfile(&pack_objects);
                rt.block_on(push_packfile(
                    &remote,
                    &old_sha_hex,
                    &new_commit.to_hex(),
                    &ref_name,
                    &pack,
                ))?;

                println!("Pushed '{}' \u{2192} {}.", view_name, remote);
            }
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
                    // Load global only for mutation.
                    load_merged_config(shared_root)?
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
                println!("\n[merge]");
                if let Some(t) = &config.merge.tool {
                    println!("tool = {t}");
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
            }
        },
        Command::BugReport {
            output,
            include_raw_intent,
        } => {
            let repo = Repository::open(".")?;
            let out_path = output.unwrap_or_else(|| "./arc-bugreport.json".to_string());
            arc_cli::bugreport::generate(&repo, &out_path, include_raw_intent)?;
            println!("Bug report written to: {out_path}");
        }
    }

    Ok(())
}

/// Get a typed config value by dot-separated key.
fn config_get(cfg: &ArcConfig, key: &str) -> Option<String> {
    match key {
        "user.name" => cfg.user.name.clone(),
        "user.email" => cfg.user.email.clone(),
        "ui.color" => Some(cfg.ui.color.clone()),
        "merge.tool" => cfg.merge.tool.clone(),
        "ai.provider" => cfg.ai.provider.clone(),
        "ai.model" => cfg.ai.model.clone(),
        "ai.endpoint" => cfg.ai.endpoint.clone(),
        _ => {
            // remotes.<name> and aliases.<name>
            if let Some(name) = key.strip_prefix("remotes.") {
                cfg.remotes.get(name).cloned()
            } else if let Some(name) = key.strip_prefix("aliases.") {
                cfg.aliases.get(name).cloned()
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
        _ => {
            if let Some(name) = key.strip_prefix("remotes.") {
                cfg.remotes.insert(name.to_string(), value.to_string());
            } else if let Some(name) = key.strip_prefix("aliases.") {
                cfg.aliases.insert(name.to_string(), value.to_string());
            } else {
                anyhow::bail!(
                    "unknown config key '{key}'; known keys: \
                     user.name, user.email, ui.color, merge.tool, \
                     ai.provider, ai.model, ai.endpoint, \
                     remotes.<name>, aliases.<name>"
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
        "merge.tool" => cfg.merge.tool = None,
        "ai.provider" => cfg.ai.provider = None,
        "ai.model" => cfg.ai.model = None,
        "ai.endpoint" => cfg.ai.endpoint = None,
        _ => {
            if let Some(name) = key.strip_prefix("remotes.") {
                cfg.remotes.remove(name);
            } else if let Some(name) = key.strip_prefix("aliases.") {
                cfg.aliases.remove(name);
            } else {
                anyhow::bail!(
                    "unknown config key '{key}'; known keys: \
                     user.name, user.email, ui.color, merge.tool, \
                     ai.provider, ai.model, ai.endpoint, \
                     remotes.<name>, aliases.<name>"
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

use arc_core::algebra::Atom;

fn atom_display_label(atom: &Atom) -> String {
    match atom {
        Atom::Insert { at, .. } => format!("++ Added:   {}", at.last().unwrap_or(&"?".to_string()))
            .green()
            .to_string(),
        Atom::Delete { at, .. } => format!("-- Deleted: {}", at.last().unwrap_or(&"?".to_string()))
            .red()
            .to_string(),
        Atom::Move { from, to } => format!(
            "~~ Moved:   {} → {}",
            from.last().unwrap_or(&"?".to_string()),
            to.last().unwrap_or(&"?".to_string())
        )
        .yellow()
        .to_string(),
        Atom::SemanticsPreserving { at, description } => format!(
            "~~ Reformat: {} ({})",
            at.last().unwrap_or(&"?".to_string()),
            description
        )
        .yellow()
        .to_string(),
        Atom::Directory { path } => {
            format!("++ Dir:     {}", path.last().unwrap_or(&"?".to_string()))
                .green()
                .to_string()
        }
        Atom::Blob { path, .. } => {
            format!("~~ Blob:    {}", path.last().unwrap_or(&"?".to_string()))
                .yellow()
                .to_string()
        }
        Atom::Mount { path, .. } => {
            format!("~~ Mount:   {}", path.last().unwrap_or(&"?".to_string()))
                .cyan()
                .to_string()
        }
        Atom::Conflict { at, .. } => {
            format!("!! Conflict: {}", at.last().unwrap_or(&"?".to_string()))
                .red()
                .to_string()
        }
    }
}
