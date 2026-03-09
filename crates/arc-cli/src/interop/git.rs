use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::time::Duration;

use indicatif::ProgressBar;

use crate::repo::{Repository, prefix_atom_path};
use arc_core::git_bridge;
use arc_core::store::author::Author;
use arc_core::store::change::Change;
use arc_core::store::view::View;
use arc_lang::ast::LanguagePlugin;
use arc_lang::ast::rust_plugin::RustPlugin;

/// Import a Git repository's history into an `arc` repository.
///
/// Walks the Git DAG in topological order (parents before children),
/// converts each commit's `.rs` file changes into semantic `arc` atoms
/// via [`RustPlugin::diff`], and maps Git branches to `arc` views.
///
/// **Known limitation (Phase 11 debt):** Merge commits are diffed against
/// only their first parent, which may duplicate atoms already introduced
/// by the second parent. This is acceptable for linear / rebased histories.
pub fn import_repo(
    git_path: impl AsRef<Path>,
    arc_repo: &mut Repository,
    author: &Author,
    signing_key: &ed25519_dalek::SigningKey,
) -> anyhow::Result<usize> {
    let git_path = git_path.as_ref();
    let analysis = git_bridge::analyze_git_repo(git_path)?;
    let git_dir = git_bridge::resolve_git_dir(git_path)?;

    let plugin = RustPlugin::new();

    // commit OID → arc Blake3Hash
    let mut oid_to_arc: HashMap<[u8; 20], arc_core::algebra::Blake3Hash> = HashMap::new();
    // commit OID → tree OID (to retrieve parent file snapshots)
    let mut oid_to_tree: HashMap<[u8; 20], [u8; 20]> = HashMap::new();

    let pb = ProgressBar::new_spinner();
    pb.enable_steady_tick(Duration::from_millis(80));
    pb.set_message("Importing git history...");

    for commit in &analysis.commits {
        pb.set_message(format!(
            "Importing commit {}...",
            &git_bridge::oid_hex(&commit.oid)[..8]
        ));

        // Map parent OIDs to arc deps.
        let deps: HashSet<arc_core::algebra::Blake3Hash> = commit
            .parents
            .iter()
            .filter_map(|p| oid_to_arc.get(p).copied())
            .collect();

        // Load parent file snapshot (empty for root commits).
        let old_files: HashMap<String, Vec<u8>> =
            match commit.parents.first().and_then(|p| oid_to_tree.get(p)) {
                Some(tree_oid) => {
                    let mut files = HashMap::new();
                    git_bridge::extract_tree_to_memory(&git_dir, tree_oid, "", &mut files)?;
                    files
                }
                None => HashMap::new(),
            };

        // Load current commit file snapshot.
        let mut new_files: HashMap<String, Vec<u8>> = HashMap::new();
        git_bridge::extract_tree_to_memory(&git_dir, &commit.tree, "", &mut new_files)?;

        let mut all_atoms = Vec::new();

        // Changed or added .rs files.
        for (path, new_bytes) in &new_files {
            if !path.ends_with(".rs") {
                continue;
            }
            let new_src = String::from_utf8_lossy(new_bytes).into_owned();
            let old_src = old_files
                .get(path)
                .map(|b| String::from_utf8_lossy(b).into_owned())
                .unwrap_or_default();
            if old_src == new_src {
                continue;
            }
            if let Ok(atoms) = plugin.diff(&old_src, &new_src, &arc_repo.store) {
                for atom in atoms {
                    all_atoms.push(prefix_atom_path(atom, path));
                }
            }
        }

        // Deleted .rs files.
        for (path, old_bytes) in &old_files {
            if !path.ends_with(".rs") || new_files.contains_key(path) {
                continue;
            }
            let old_src = String::from_utf8_lossy(old_bytes).into_owned();
            if let Ok(atoms) = plugin.diff(&old_src, "", &arc_repo.store) {
                for atom in atoms {
                    all_atoms.push(prefix_atom_path(atom, path));
                }
            }
        }

        let intent = format!(
            "git-author: {} <{}>\n\n{}",
            commit.author_name, commit.author_email, commit.message
        );
        let change = Change::new(deps, all_atoms, intent, author.clone(), signing_key);
        arc_repo
            .store
            .write_change(&change)
            .map_err(|e| anyhow::anyhow!("CAS write error: {e}"))?;
        arc_repo.graph_add_change(change.clone());

        oid_to_arc.insert(commit.oid, change.id);
        oid_to_tree.insert(commit.oid, commit.tree);
    }

    pb.finish_with_message(format!("Imported {} commits.", analysis.commit_count));

    // Map Git branches to arc Views.
    for (branch_name, branch_oid) in git_bridge::list_branch_heads(git_path)? {
        if let Some(&arc_id) = oid_to_arc.get(&branch_oid) {
            let view = View::new(&branch_name, HashSet::from([arc_id]));
            view.save(&arc_repo.shared_root)
                .map_err(|e| anyhow::anyhow!("failed to save view '{branch_name}': {e}"))?;
        }
    }

    Ok(analysis.commit_count)
}

#[cfg(test)]
mod tests {
    use super::*;
    use arc_lang::ast::LanguagePlugin;
    use std::process::Command;

    /// Run a git command in `dir`, panicking on failure.
    fn git(args: &[&str], dir: &Path) {
        let status = Command::new("git")
            .args(args)
            .current_dir(dir)
            .env("GIT_AUTHOR_NAME", "test")
            .env("GIT_AUTHOR_EMAIL", "test@test.com")
            .env("GIT_COMMITTER_NAME", "test")
            .env("GIT_COMMITTER_EMAIL", "test@test.com")
            .status()
            .unwrap_or_else(|e| panic!("failed to spawn git {args:?}: {e}"));
        assert!(status.success(), "git {args:?} exited with {status}");
    }

    #[test]
    fn test_git_import() {
        let git_dir = tempfile::tempdir().unwrap();
        let arc_dir = tempfile::tempdir().unwrap();
        let git_path = git_dir.path();

        // Initialize a Git repository and make two commits.
        git(&["init"], git_path);
        git(&["config", "user.email", "test@test.com"], git_path);
        git(&["config", "user.name", "test"], git_path);

        std::fs::write(git_path.join("main.rs"), "fn a() {}").unwrap();
        git(&["add", "."], git_path);
        git(&["commit", "-m", "add a"], git_path);

        std::fs::write(git_path.join("main.rs"), "fn a() {}\n\nfn b() {}").unwrap();
        git(&["add", "."], git_path);
        git(&["commit", "-m", "add b"], git_path);

        // Import into a fresh arc repository.
        let arc_path = arc_dir.path().join("imported");
        let mut arc_repo = Repository::init(&arc_path).unwrap();
        let (author, signing_key) = arc_core::store::author::test_keypair();
        import_repo(git_path, &mut arc_repo, &author, &signing_key).unwrap();

        // Verify the arc graph has exactly 2 changes.
        assert_eq!(arc_repo.graph.load().len(), 2, "arc graph must have 2 changes");

        // Discover the branch name that Git created (could be "master" or "main").
        let output = Command::new("git")
            .args(["rev-parse", "--abbrev-ref", "HEAD"])
            .current_dir(git_path)
            .output()
            .expect("git rev-parse failed");
        let branch_name = String::from_utf8_lossy(&output.stdout).trim().to_string();

        // Verify the branch view was saved with at least one head.
        let imported_view = arc_core::store::view::View::load(&arc_path, &branch_name).unwrap();
        assert!(
            !imported_view.heads.is_empty(),
            "imported branch view '{branch_name}' must have at least one head after import"
        );

        // Hydrate and materialize the imported view.
        arc_repo.hydrate(&branch_name).unwrap();
        let state = arc_repo.materialize(&branch_name).unwrap();

        // Verify unparse reconstructs both functions.
        let plugin = RustPlugin::new();
        let source = plugin.unparse(&state, "main.rs").unwrap();
        assert!(
            source.contains("fn a()"),
            "unparsed source must contain fn a(), got: {source}"
        );
        assert!(
            source.contains("fn b()"),
            "unparsed source must contain fn b(), got: {source}"
        );
    }
}
