use clap::{Parser, Subcommand};

use arc_cli::interop::git::import_repo;
use arc_cli::repo::Repository;
use arc_cli::sync::{fetch, pull};
use arc_core::ai::MockResolver;
use arc_core::algebra::Blake3Hash;
use arc_core::store::author::{Author, load_identity, save_identity};
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
        /// Description of the change.
        #[arg(short, long)]
        message: String,
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
    Tag {
        /// Tag name (e.g. "v1.0.0").
        name: String,
        /// Full 64-character hex hash of the change to tag.
        hash: String,
    },
    /// List all tags in the repository.
    Tags,
    /// Semantically revert a change by rolling back its AST atoms.
    Revert {
        /// Full 64-character hex hash of the change to revert.
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
    /// Undo the last view-mutating operation using the operation log.
    Undo,
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

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Command::Init { path } => {
            let target = path.unwrap_or_else(|| ".".to_string());
            Repository::init(&target)?;
            println!("Initialized empty arc repository in {target}/.arc");
        }
        Command::Snap {
            message,
            interactive,
        } => {
            let mut repo = Repository::open(".")?;
            let (author, signing_key) = load_identity()?;
            repo.set_identity(author, signing_key);
            match repo.snap(&message, interactive)? {
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
                println!("No changes yet.");
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
            println!("Cherry-picked {} into current view.", &hash[..8]);
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
        },
        Command::Tag { name, hash } => {
            let hash_bytes = hex_to_hash(&hash)?;
            let mut repo = Repository::open(".")?;
            let (author, signing_key) = load_identity()?;
            repo.set_identity(author, signing_key);
            repo.create_tag(&name, &hash_bytes)?;
            println!("Tagged {} as '{name}'", &hash[..8]);
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
            let hash_bytes = hex_to_hash(&hash)?;
            let mut repo = Repository::open(".")?;
            let (author, signing_key) = load_identity()?;
            repo.set_identity(author, signing_key);
            let revert_id = repo.revert(&hash_bytes)?;
            let hex: String = revert_id.iter().map(|b| format!("{b:02x}")).collect();
            println!("Reverted {} \u{2192} new change {}", &hash[..8], &hex[..8]);
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
            repo.undo()?;
        }
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
        Atom::Delete { at } => {
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
