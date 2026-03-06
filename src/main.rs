use clap::{Parser, Subcommand};

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
    }

    Ok(())
}
