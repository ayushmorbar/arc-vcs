use clap::{Parser, Subcommand};

use arc::ai::MockResolver;
use arc::interop::git::import_repo;
use arc::network::sync::{fetch, pull};
use arc::store::author::{load_identity, save_identity, Author};
use arc::store::repo::Repository;

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

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Command::Init { path } => {
            let target = path.unwrap_or_else(|| ".".to_string());
            Repository::init(&target)?;
            println!("Initialized empty arc repository in {target}/.arc");
        }
        Command::Snap { message, interactive } => {
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
                for change in changes {
                    let hex: String = change.id.iter().map(|b| format!("{b:02x}")).collect();
                    let author_str = match &change.author {
                        arc::store::author::Author::Human { name, email, .. } => {
                            format!("{name} <{email}>")
                        }
                        arc::store::author::Author::AI { model, human_sponsor } => {
                            let sponsor: String =
                                human_sponsor.iter().map(|b| format!("{b:02x}")).collect();
                            format!("{model} | sponsor:{}", &sponsor[..8])
                        }
                    };
                    println!("{} — {} — {}", &hex[..8], author_str, change.intent);
                }
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
                    let hash_hex: String =
                        change.id.iter().map(|b| format!("{b:02x}")).collect();
                    let short_hash = &hash_hex[..8];
                    let author_str = match &change.author {
                        arc::store::author::Author::Human { name, email, .. } => {
                            format!("{name} <{email}>")
                        }
                        arc::store::author::Author::AI { model, human_sponsor } => {
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
                    Author::AI { model, human_sponsor } => {
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
            rt.block_on(arc::network::server::serve(port))?;
        }
    }

    Ok(())
}

/// Parse a 64-character hex string into a [`arc::algebra::Blake3Hash`].
fn hex_to_hash(hex: &str) -> anyhow::Result<arc::algebra::Blake3Hash> {
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

/// Human-readable single-line description of an [`arc::algebra::Atom`].
fn atom_display_label(atom: &arc::algebra::Atom) -> String {
    use arc::algebra::Atom;
    match atom {
        Atom::Insert { at, .. } => {
            let node = at.last().cloned().unwrap_or_else(|| "?".to_string());
            let file = at.get(1).cloned().unwrap_or_else(|| "?".to_string());
            format!("+ {node}  ({file})")
        }
        Atom::Delete { at } => {
            let node = at.last().cloned().unwrap_or_else(|| "?".to_string());
            let file = at.get(1).cloned().unwrap_or_else(|| "?".to_string());
            format!("- {node}  ({file})")
        }
        Atom::Move { from, to } => {
            format!(
                "~ {} → {}",
                from.last().cloned().unwrap_or_else(|| "?".to_string()),
                to.last().cloned().unwrap_or_else(|| "?".to_string())
            )
        }
        Atom::SemanticsPreserving { at, description } => {
            let node = at.last().cloned().unwrap_or_else(|| "?".to_string());
            format!("~ {node}  ({description})")
        }
        Atom::Directory { path } => {
            let dir = path.last().cloned().unwrap_or_else(|| "?".to_string());
            format!("d {dir}")
        }
    }
}
