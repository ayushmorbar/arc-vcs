use std::io::{self, Write};
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result};
use arc_ai::{LlmResolver, MockResolver};
use arc_store_types::author::generate_transient_keypair_seed;

use crate::repo::Repository;

/// Execute the interactive `arc tour` onboarding flow.
pub fn run_tour() -> Result<()> {
    println!("Welcome to arc. Let's show you the future of version control.");
    thread::sleep(Duration::from_millis(500));

    let temp = tempfile::tempdir().context("failed to create temporary tour directory")?;
    let previous_cwd =
        std::env::current_dir().context("failed to read current working directory")?;
    std::env::set_current_dir(temp.path()).context("failed to switch into tour directory")?;
    let _cwd_guard = CwdGuard::new(previous_cwd);

    let tour_result = (|| -> Result<()> {
        let mut repo = Repository::init(".")?;
        let (author, seed) = generate_transient_keypair_seed("arc-tour");
        let signing_key = ed25519_dalek::SigningKey::from_bytes(&seed);
        repo.set_identity(author, signing_key);

        println!("\n[1/4] Setting up a tiny project...");
        thread::sleep(Duration::from_millis(700));
        std::fs::write(
            "math.rs",
            "pub fn add(a: i32, b: i32) -> i32 {\n    a + b\n}\n",
        )?;
        if repo.snapshot()? {
            let _ = repo.finalize_snapshot("Initial commit")?;
            let _ = repo.fork_empty_snapshot()?;
        }

        println!("\n[2/4] Simulating two parallel developers...");
        thread::sleep(Duration::from_millis(700));
        repo.create_view("feature-a")?;
        repo.switch_view("feature-a")?;
        std::fs::write(
            "math.rs",
            "pub fn sum(a: i32, b: i32) -> i32 {\n    a + b\n}\n",
        )?;
        if repo.snapshot()? {
            let _ = repo.finalize_snapshot("Rename add to sum")?;
            let _ = repo.fork_empty_snapshot()?;
        }

        repo.switch_view("main")?;
        repo.create_view("feature-b")?;
        repo.switch_view("feature-b")?;
        std::fs::write(
            "math.rs",
            "pub fn add(a: i32, b: i32) -> i32 {\n    println!(\"adding {a} and {b}\");\n    a + b\n}\n",
        )?;
        if repo.snapshot()? {
            let _ = repo.finalize_snapshot("Add trace print")?;
            let _ = repo.fork_empty_snapshot()?;
        }

        repo.merge_view("feature-a")
            .context("failed to set up conflict state for tour")?;

        println!("\n[3/4] Conflict reveal...");
        thread::sleep(Duration::from_millis(700));
        println!(
            "Uh oh. Two developers edited the same function. In Git, your file would be broken with <<<<<<< markers right now."
        );

        let current_contents = std::fs::read_to_string("math.rs").unwrap_or_default();
        println!(
            "\nCurrent file content:\n---------------------\n{current_contents}\n---------------------"
        );
        println!(
            "In arc, conflicts are mathematical states, not broken text. Type 'arc resolve' to let the AI fix this."
        );

        println!("\n[4/4] Your turn. Type 'arc resolve' and press Enter:");
        loop {
            print!("> ");
            io::stdout().flush().ok();

            let mut line = String::new();
            let read = io::stdin().read_line(&mut line)?;
            if read == 0 {
                anyhow::bail!("interactive stdin required to continue the tour");
            }
            if line.trim() == "arc resolve" {
                break;
            }
            println!("Please type exactly: arc resolve");
        }

        if let Some(llm) = LlmResolver::from_env() {
            repo.resolve_conflict(&llm)?;
        } else {
            let mock = MockResolver;
            repo.resolve_conflict(&mock)?;
        }

        let merged_contents = std::fs::read_to_string("math.rs").unwrap_or_default();
        println!(
            "\nAI merged result:\n---------------------\n{merged_contents}\n---------------------"
        );
        println!("Tour complete. You just experienced arc conflict resolution.");

        Ok(())
    })();

    drop(temp);

    tour_result
}

struct CwdGuard {
    previous: std::path::PathBuf,
}

impl CwdGuard {
    fn new(previous: std::path::PathBuf) -> Self {
        Self { previous }
    }
}

impl Drop for CwdGuard {
    fn drop(&mut self) {
        let _ = std::env::set_current_dir(&self.previous);
    }
}

