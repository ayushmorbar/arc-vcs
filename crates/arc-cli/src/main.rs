use anyhow::Context as _;
use clap::{Parser, Subcommand};
use tracing_subscriber::EnvFilter;

use arc_cli::interop::git::import_repo;
use arc_cli::repo::{Repository, load_merged_config, save_global_config};
use arc_cli::sync::{fetch, pull};
use arc_core::ai::MockResolver;
use arc_core::algebra::Blake3Hash;
use arc_core::store::author::{Author, load_identity, save_identity};
use arc_core::store::oplog::OperationAgent;
use comfy_table::{Cell, Color, Table, presets};
use owo_colors::OwoColorize;

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
    Log,
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
    /// Hint command for users familiar with Git’s `git push`.
    ///
    /// arc is a P2P CRDT: there is no single “central” server to push to.
    /// Use `arc sync` or `arc pull <url>` to exchange state with remotes.
    Push {
        /// Optional remote name (only used in the hint message).
        remote: Option<String>,
    },
    /// Get or set arc configuration / global aliases.
    Config {
        #[command(subcommand)]
        action: ConfigAction,
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
    Resolve,
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

fn main() -> anyhow::Result<()> {
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
        Command::Init { path } => {
            let target = path.unwrap_or_else(|| ".".to_string());
            Repository::init(&target)?;
            println!("Initialized empty arc repository in {target}/.arc");
        }
        Command::Snap {
            message,
            auto_msg,
            interactive,
        } => {
            use std::io::Write;

            let mut repo = Repository::open(".")?;
            let (author, signing_key) = load_identity()?;
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

                let rt = tokio::runtime::Runtime::new()
                    .context("failed to start async runtime")?;
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

            match repo.snap(&final_message, interactive)? {
                Some(id) => {
                    let hex: String = id.iter().map(|b| format!("{b:02x}")).collect();
                    println!("snap {hex}");
                }
                None => {
                    println!("Nothing to snap — working directory matches history.");
                }
            }
        }
        Command::Log => {
            let mut repo = Repository::open(".")?;
            let (author, signing_key) = load_identity()?;
            repo.set_identity(author, signing_key);
            let changes = repo.log()?;
            if changes.is_empty() {
                println!("No changes yet. Use 'arc snap' to create your first change.");
            } else {
                let mut table = Table::new();
                table.load_preset(presets::NOTHING);
                for change in changes {
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
                    };
                    table.add_row(vec![
                        Cell::new(&hex[..8]).fg(Color::Cyan),
                        Cell::new(&author_str).fg(Color::Magenta),
                        Cell::new(&change.intent),
                    ]);
                }
                println!("{table}");
            }
        }
        Command::Status => {
            let mut repo = Repository::open(".")?;
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
                let resolver = MockResolver;
                let id = repo.resolve_conflict(&resolver)?;
                let hex: String = id.iter().map(|b| format!("{b:02x}")).collect();
                println!("Resolved conflict → {hex}");
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
                "Identity configured: {name} <{email}> (Ed25519 keypair active)\n\
                 Run 'arc auth whoami' to inspect your public key."
            );
        }
        Command::Diff { semantic } => {
            let mut repo = Repository::open(".")?;
            let view_name = repo.current_view_name()?;
            println!("On view: {}", view_name.cyan().bold());
            let (atoms, old_texts) = repo.diff_info()?;
            if atoms.is_empty() {
                println!("Working directory clean — nothing to diff.");
            } else if semantic {
                arc_cli::semantic_diff::group_and_render_semantic(&atoms)?;
            } else {
                arc_cli::semantic_diff::group_and_render(
                    &atoms,
                    &old_texts,
                    &repo.work_root,
                )?;
            }
        }
        Command::Push { remote } => {
            let repo = Repository::open(".")?;
            let remote_name = remote.as_deref().unwrap_or("origin");
            let remotes = repo.list_remotes()?;
            let url = remotes.get(remote_name).cloned().ok_or_else(|| {
                anyhow::anyhow!(
                    "Remote '{}' not found. Add one with:\n  arc remote add {} <url>",
                    remote_name,
                    remote_name
                )
            })?;
            let rt = tokio::runtime::Runtime::new()?;
            rt.block_on(async {
                let client = arc_core::network::NetworkClient::new()?;
                client.push(remote_name, &url).await
            })?;
        }
        Command::Config { action } => match action {
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
        },
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
        Atom::Insert { at, .. } => {
            format!("++ Added:   {}", at.last().unwrap_or(&"?".to_string()))
                .green()
                .to_string()
        }
        Atom::Delete { at, .. } => {
            format!("-- Deleted: {}", at.last().unwrap_or(&"?".to_string()))
                .red()
                .to_string()
        }
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
    }
}
