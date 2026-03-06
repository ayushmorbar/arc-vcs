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
    },
    /// Show the change log (placeholder).
    Log,
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
        Command::Snap { message } => {
            let mut repo = Repository::open(".")?;
            let (author, signing_key) = load_identity()?;
            repo.set_identity(author, signing_key);
            match repo.snap(&message)? {
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
            println!("(log not yet implemented)");
        }
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
