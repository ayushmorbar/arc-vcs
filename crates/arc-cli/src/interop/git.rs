use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::time::Duration;

use git2::Sort;
use indicatif::ProgressBar;

use crate::repo::{Repository, prefix_atom_path};
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
) -> anyhow::Result<()> {
    let git_repo = git2::Repository::open(git_path.as_ref())
        .map_err(|e| anyhow::anyhow!("failed to open git repo: {e}"))?;

    let mut revwalk = git_repo.revwalk()?;
    revwalk.push_head()?;
    revwalk.set_sorting(Sort::TOPOLOGICAL | Sort::REVERSE)?;

    let plugin = RustPlugin::new();

    // Git OID → arc Blake3Hash mapping.
    let mut oid_map: HashMap<git2::Oid, arc_core::algebra::Blake3Hash> = HashMap::new();

    let pb = ProgressBar::new_spinner();
    pb.enable_steady_tick(Duration::from_millis(80));
    pb.set_message("Importing git history...");

    for oid_result in revwalk {
        let oid = oid_result?;
        pb.set_message(format!("Importing commit {}...", &format!("{oid}")[..8]));
        let commit = git_repo.find_commit(oid)?;

        // Map parent OIDs to arc deps.
        let deps: HashSet<arc_core::algebra::Blake3Hash> = commit
            .parent_ids()
            .filter_map(|parent_oid| oid_map.get(&parent_oid).copied())
            .collect();

        // Get the current commit's tree.
        let new_tree = commit.tree()?;

        // Get the parent tree (empty tree for root commits).
        let parent_tree = if commit.parent_count() > 0 {
            Some(commit.parent(0)?.tree()?)
        } else {
            None
        };

        // Diff parent tree ↔ current tree.
        let diff = git_repo.diff_tree_to_tree(parent_tree.as_ref(), Some(&new_tree), None)?;

        let mut all_atoms = Vec::new();

        diff.foreach(
            &mut |delta, _| {
                let new_file = delta.new_file();
                let path_str = match new_file.path().and_then(|p| p.to_str()) {
                    Some(p) if p.ends_with(".rs") => p.to_string(),
                    _ => return true, // skip non-Rust files
                };

                let new_src = if new_file.id().is_zero() {
                    String::new()
                } else {
                    match git_repo.find_blob(new_file.id()) {
                        Ok(blob) => String::from_utf8_lossy(blob.content()).to_string(),
                        Err(_) => return true,
                    }
                };

                let old_file = delta.old_file();
                let old_src = if old_file.id().is_zero() {
                    String::new()
                } else {
                    match git_repo.find_blob(old_file.id()) {
                        Ok(blob) => String::from_utf8_lossy(blob.content()).to_string(),
                        Err(_) => return true,
                    }
                };

                // Generate semantic atoms via the language plugin.
                if let Ok(atoms) = plugin.diff(&old_src, &new_src) {
                    for atom in atoms {
                        all_atoms.push(prefix_atom_path(atom, &path_str));
                    }
                }

                true // continue iteration
            },
            None, // binary_cb
            None, // hunk_cb
            None, // line_cb
        )?;

        let message = commit.message().unwrap_or("(no message)").to_string();

        let change = Change::new(deps, all_atoms, message, author.clone(), signing_key);
        arc_repo
            .store
            .write_change(&change)
            .map_err(|e| anyhow::anyhow!("CAS write error: {e}"))?;
        arc_repo.graph.add_change(change.clone());

        oid_map.insert(oid, change.id);
    }
    pb.finish_with_message(format!("Imported {} commits.", oid_map.len()));

    // Map Git branches to arc Views.
    for reference in git_repo.references()? {
        let reference = reference?;
        let ref_name = match reference.name() {
            Some(n) => n.to_string(),
            None => continue,
        };

        // Only map local branches (refs/heads/*).
        if let Some(branch_name) = ref_name.strip_prefix("refs/heads/")
            && let Some(target_oid) = reference.target()
            && let Some(&arc_id) = oid_map.get(&target_oid)
        {
            let view = View::new(branch_name, HashSet::from([arc_id]));
            view.save(&arc_repo.shared_root)
                .map_err(|e| anyhow::anyhow!("failed to save view '{branch_name}': {e}"))?;
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use arc_lang::ast::LanguagePlugin;

    /// Create a Git commit with a tree containing a single file.
    fn git_commit(
        repo: &git2::Repository,
        path: &str,
        content: &str,
        parent: Option<&git2::Commit>,
        message: &str,
    ) -> git2::Oid {
        let mut index = repo.index().unwrap();
        let blob_oid = repo.blob(content.as_bytes()).unwrap();
        index
            .add(&git2::IndexEntry {
                ctime: git2::IndexTime::new(0, 0),
                mtime: git2::IndexTime::new(0, 0),
                dev: 0,
                ino: 0,
                mode: 0o100644,
                uid: 0,
                gid: 0,
                file_size: content.len() as u32,
                id: blob_oid,
                flags: 0,
                flags_extended: 0,
                path: path.as_bytes().to_vec(),
            })
            .unwrap();
        let tree_oid = index.write_tree().unwrap();
        let tree = repo.find_tree(tree_oid).unwrap();

        let sig = git2::Signature::now("test", "test@test.com").unwrap();
        let parents: Vec<&git2::Commit> = parent.into_iter().collect();
        repo.commit(Some("HEAD"), &sig, &sig, message, &tree, &parents)
            .unwrap()
    }

    #[test]
    fn test_git_import() {
        let git_dir = tempfile::tempdir().unwrap();
        let arc_dir = tempfile::tempdir().unwrap();

        // Initialize a bare-bones Git repository.
        let git_repo = git2::Repository::init(git_dir.path()).unwrap();

        // First commit: add main.rs with fn a().
        let oid1 = git_commit(&git_repo, "main.rs", "fn a() {}", None, "add a");
        let commit1 = git_repo.find_commit(oid1).unwrap();

        // Second commit: add fn b().
        git_commit(
            &git_repo,
            "main.rs",
            "fn a() {}\n\nfn b() {}",
            Some(&commit1),
            "add b",
        );

        // Import into a fresh arc repository.
        let arc_path = arc_dir.path().join("imported");
        let mut arc_repo = Repository::init(&arc_path).unwrap();
        let (author, signing_key) = arc_core::store::author::test_keypair();
        import_repo(git_dir.path(), &mut arc_repo, &author, &signing_key).unwrap();

        // Verify the arc graph has exactly 2 changes.
        assert_eq!(arc_repo.graph.len(), 2, "arc graph must have 2 changes");

        // Discover the branch name that Git created (could be "master" or "main").
        let head = git_repo.head().unwrap();
        let branch_name = head.shorthand().unwrap();

        // Hydrate and materialize the imported view.
        arc_repo.hydrate(branch_name).unwrap();
        let state = arc_repo.materialize(branch_name).unwrap();

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
