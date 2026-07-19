//! BLUF: `arc-git` is the Git ingress edge for the `arc` DAG.
//!
//! It reads legacy Git repositories and emits deterministic commit/tree/blob
//! structures that upstream `arc` crates can translate into Spacetime-DAG
//! changes and CRDT-algebra operations.
//!
//! ## Purity and I/O boundary
//!
//! This crate is an I/O boundary by design:
//! - It performs filesystem reads of `.git` refs, loose objects, and packfiles.
//! - It performs pure parsing/walking after bytes are loaded.
//! - It does not mutate repository state.
//!
//! ## Why this crate exists
//!
//! The `arc` architecture keeps Git compatibility concerns outside algebra and
//! provenance layers. `arc-git` isolates SHA-1 object decoding and history walk
//! logic so Ed25519 provenance and CRDT semantics remain independent from Git
//! storage internals.
//!
//! ## Example
//!
//! ```no_run
//! use std::path::Path;
//!
//! let analysis = arc_git::analyze_git_repo(Path::new("."))?;
//! println!("HEAD={} commits={}", analysis.head_hex, analysis.commit_count);
//! # Ok::<(), anyhow::Error>(())
//! ```

use std::path::PathBuf;

use bytes::Bytes;

mod domain;
mod ingress;

// -- types --------------------------------------------------------------------

/// A 20-byte SHA-1 object identifier - Git's native hash format.
pub type GitOid = [u8; 20];

/// Git object type tag.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ObjKind {
    Commit,
    Tree,
    Blob,
    Tag,
}

/// Decoded Git object: kind + raw payload (header already stripped).
///
/// This type is intentionally crate-private so raw Git storage bytes are
/// translated into `GitCommit`/`GitTree` domain structures before crossing
/// the arc-git boundary.
pub(crate) struct RawObject {
    pub(crate) kind: ObjKind,
    pub(crate) data: Bytes,
}

/// Parsed metadata extracted from a single Git commit object.
#[derive(Debug, Clone)]
pub struct GitCommit {
    /// SHA-1 hash of this commit.
    pub oid: GitOid,
    /// SHA-1 of the root tree object.
    pub tree: GitOid,
    /// Parent commit OIDs (empty for root commits).
    pub parents: Vec<GitOid>,
    /// Author name.
    pub author_name: String,
    /// Author email.
    pub author_email: String,
    /// Author-date as a Unix timestamp (seconds since epoch).
    pub author_timestamp: i64,
    /// Committer name.
    pub committer_name: String,
    /// Committer email.
    pub committer_email: String,
    /// Full commit message (subject + body).
    pub message: String,
}

/// Summary returned after analysing a legacy Git repository.
#[derive(Debug)]
pub struct GitAnalysis {
    /// Filesystem path that was analysed.
    pub path: PathBuf,
    /// HEAD commit as a 40-char lowercase hex string.
    pub head_hex: String,
    /// Total number of reachable commits.
    pub commit_count: usize,
    /// All reachable commits in topological order, **oldest first**.
    pub commits: Vec<GitCommit>,
}

/// A single entry inside a Git tree object.
///
/// Each entry corresponds to either a file (`blob`) or a subdirectory
/// (`tree`). The `mode` string follows Git conventions.
#[derive(Debug, Clone)]
pub struct TreeEntry {
    /// Octal mode as a UTF-8 string (e.g. `"100644"`).
    pub mode: String,
    /// File or directory name (not a full path).
    pub name: String,
    /// 40-char lowercase hex SHA-1 of the pointed-to object.
    pub oid: String,
}

/// Structured representation of a Git tree object.
#[derive(Debug, Clone)]
pub struct GitTree {
    /// All file and directory entries listed in this tree.
    pub entries: Vec<TreeEntry>,
}

pub use domain::{oid_hex, parse_tree};
#[cfg(test)]
pub(crate) use ingress::{
    TEST_BACKEND_AUTO, TEST_BACKEND_FORCE_MMAP_FAIL, TEST_BACKEND_LEGACY_ONLY,
    TEST_BACKEND_MMAP_ONLY, TEST_TRAVERSAL_AUTO, TEST_TRAVERSAL_COMMIT_GRAPH_ONLY,
    TEST_TRAVERSAL_LEGACY_ONLY, set_test_backend_override, set_test_traversal_override,
};
pub use ingress::{
    analyze_git_repo, extract_tree_to_memory, list_branch_heads, read_blob, read_git_user_config,
    resolve_git_dir,
};

#[cfg(test)]
mod tests {
    use std::{
        collections::HashMap,
        path::Path,
        process::Command,
        sync::{Mutex, MutexGuard, OnceLock},
    };

    use super::*;

    fn backend_override_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    fn git(args: &[&str], dir: &Path) {
        let status = Command::new("git")
            .args(args)
            .current_dir(dir)
            .env("GIT_AUTHOR_NAME", "test")
            .env("GIT_AUTHOR_EMAIL", "test@test.com")
            .env("GIT_COMMITTER_NAME", "test")
            .env("GIT_COMMITTER_EMAIL", "test@test.com")
            .status()
            .unwrap_or_else(|e| panic!("git {args:?}: {e}"));
        assert!(status.success(), "git {args:?} failed with {status}");
    }

    fn git_output(args: &[&str], dir: &Path) -> String {
        let out = Command::new("git")
            .args(args)
            .current_dir(dir)
            .output()
            .unwrap_or_else(|e| panic!("git {args:?}: {e}"));
        assert!(out.status.success(), "git {args:?} failed with {}", out.status);
        String::from_utf8(out.stdout)
            .unwrap_or_else(|e| panic!("git {args:?} produced non-utf8 output: {e}"))
    }

    struct BackendOverrideGuard {
        previous_pack: u8,
        previous_traversal: u8,
        _lock: MutexGuard<'static, ()>,
    }

    impl BackendOverrideGuard {
        fn set(mode: u8) -> Self {
            Self::set_with(mode, TEST_TRAVERSAL_AUTO)
        }

        fn set_with(pack_mode: u8, traversal_mode: u8) -> Self {
            let lock = backend_override_lock()
                .lock()
                .expect("backend override mutex should not be poisoned");
            let previous_pack = set_test_backend_override(pack_mode);
            let previous_traversal = set_test_traversal_override(traversal_mode);
            Self { previous_pack, previous_traversal, _lock: lock }
        }
    }

    impl Drop for BackendOverrideGuard {
        fn drop(&mut self) {
            let _ = set_test_backend_override(self.previous_pack);
            let _ = set_test_traversal_override(self.previous_traversal);
        }
    }

    fn create_packed_repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path();

        git(&["init"], path);
        git(&["config", "user.email", "test@test.com"], path);
        git(&["config", "user.name", "test"], path);
        git(&["config", "core.autocrlf", "false"], path);

        for i in 0..8 {
            std::fs::write(path.join(format!("f{i}.txt")), format!("line-{i}\n")).unwrap();
            git(&["add", "."], path);
            git(&["commit", "-m", &format!("commit-{i}")], path);
        }

        std::fs::write(path.join("binary.bin"), vec![0x00, 0xFF, 0x10, 0x80, 0x01]).unwrap();
        git(&["add", "."], path);
        git(&["commit", "-m", "binary"], path);

        git(&["gc", "--aggressive", "--prune=now"], path);
        dir
    }

    fn create_commit_graph_repo() -> tempfile::TempDir {
        let dir = create_packed_repo();
        git(&["commit-graph", "write", "--reachable"], dir.path());
        dir
    }

    /// `analyze_git_repo` must return the correct commit count and metadata.
    #[test]
    fn test_analyze_git_repo_basic() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path();

        git(&["init"], path);
        git(&["config", "user.email", "test@test.com"], path);
        git(&["config", "user.name", "test"], path);
        git(&["config", "core.autocrlf", "false"], path);
        std::fs::write(path.join("a.rs"), "fn a() {}").unwrap();
        git(&["add", "."], path);
        git(&["commit", "-m", "first commit"], path);

        let analysis = analyze_git_repo(path).unwrap();

        assert_eq!(analysis.commit_count, 1, "must report exactly 1 commit");
        assert_eq!(analysis.commits.len(), 1);
        assert_eq!(analysis.commits[0].message, "first commit");
        assert_eq!(analysis.head_hex.len(), 40, "HEAD hex must be 40 chars");
    }

    /// `extract_tree_to_memory` must return the exact bytes for all files in
    /// the tree, including files in subdirectories.
    #[test]
    fn test_extract_tree_to_memory() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path();

        git(&["init"], path);
        git(&["config", "user.email", "test@test.com"], path);
        git(&["config", "user.name", "test"], path);
        git(&["config", "core.autocrlf", "false"], path);

        std::fs::write(path.join("root.rs"), b"fn root() {}" as &[u8]).unwrap();
        std::fs::create_dir_all(path.join("sub")).unwrap();
        std::fs::write(path.join("sub").join("nested.rs"), b"fn nested() {}" as &[u8]).unwrap();

        git(&["add", "."], path);
        git(&["commit", "-m", "initial"], path);

        let analysis = analyze_git_repo(path).unwrap();
        let git_dir = resolve_git_dir(path).unwrap();
        let tree_oid = analysis.commits[0].tree;

        let mut files: HashMap<String, Vec<u8>> = HashMap::new();
        extract_tree_to_memory(&git_dir, &tree_oid, "", &mut files).unwrap();

        assert!(files.contains_key("root.rs"), "root-level file must be extracted");
        assert!(files.contains_key("sub/nested.rs"), "nested file must be extracted");
        assert_eq!(files["root.rs"], b"fn root() {}", "root.rs bytes must match exactly");
        assert_eq!(
            files["sub/nested.rs"], b"fn nested() {}",
            "sub/nested.rs bytes must match exactly"
        );
    }

    /// `list_branch_heads` must return the correct branch name and its tip OID.
    #[test]
    fn test_list_branch_heads() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path();

        git(&["init"], path);
        git(&["config", "user.email", "test@test.com"], path);
        git(&["config", "user.name", "test"], path);
        git(&["config", "core.autocrlf", "false"], path);
        std::fs::write(path.join("a.rs"), "fn a() {}").unwrap();
        git(&["add", "."], path);
        git(&["commit", "-m", "first"], path);

        let heads = list_branch_heads(path).unwrap();
        assert_eq!(heads.len(), 1, "must find exactly one branch");

        let branch_name = heads.keys().next().unwrap();
        assert!(
            branch_name == "main" || branch_name == "master",
            "branch name must be main or master, got: {branch_name}"
        );

        // The branch tip OID must match the HEAD reported by analyze_git_repo.
        let analysis = analyze_git_repo(path).unwrap();
        let tip_hex = oid_hex(heads.values().next().unwrap());
        assert_eq!(tip_hex, analysis.head_hex, "branch tip OID must equal HEAD");
    }

    /// Packed repositories should be ingested through the mmap-first index path
    /// while preserving commit ordering and metadata parity.
    #[test]
    fn test_analyze_git_repo_with_packed_objects() {
        let dir = create_packed_repo();
        let path = dir.path();

        let git_dir = resolve_git_dir(path).unwrap();
        let pack_dir = git_dir.join("objects").join("pack");
        let has_idx = std::fs::read_dir(&pack_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .any(|e| e.file_name().to_string_lossy().ends_with(".idx"));
        assert!(has_idx, "git gc should produce at least one pack index file");

        let head_hex = git_output(&["rev-parse", "HEAD"], path).trim().to_string();
        let analysis = analyze_git_repo(path).unwrap();

        assert_eq!(analysis.head_hex, head_hex, "head OID must match git rev-parse");
        assert_eq!(analysis.commit_count, 9, "all reachable commits must be returned");
        assert_eq!(
            analysis.commits.first().unwrap().message,
            "commit-0",
            "oldest-first ordering must be preserved after packing"
        );
        assert_eq!(
            analysis.commits.last().unwrap().message,
            "binary",
            "latest commit must remain at the end"
        );

        let mut files: HashMap<String, Vec<u8>> = HashMap::new();
        let tree = analysis.commits.last().unwrap().tree;
        extract_tree_to_memory(&git_dir, &tree, "", &mut files).unwrap();
        assert_eq!(
            files.get("binary.bin").unwrap(),
            &vec![0x00, 0xFF, 0x10, 0x80, 0x01],
            "binary payload must round-trip exactly through packed object decode"
        );
    }

    #[test]
    fn test_mmap_only_backend_on_packed_repo() {
        let _guard = BackendOverrideGuard::set(TEST_BACKEND_MMAP_ONLY);
        let dir = create_packed_repo();
        let analysis = analyze_git_repo(dir.path()).unwrap();
        assert_eq!(analysis.commit_count, 9);
        assert_eq!(analysis.commits.last().unwrap().message, "binary");
    }

    #[test]
    fn test_forced_mmap_failure_falls_back_to_legacy() {
        let _guard = BackendOverrideGuard::set(TEST_BACKEND_FORCE_MMAP_FAIL);
        let dir = create_packed_repo();
        let analysis = analyze_git_repo(dir.path()).unwrap();
        assert_eq!(analysis.commit_count, 9);
        assert_eq!(analysis.commits.last().unwrap().message, "binary");
    }

    #[test]
    fn test_mmap_and_legacy_backends_are_parity_equivalent() {
        let dir = create_packed_repo();

        let mmap_analysis = {
            let _guard = BackendOverrideGuard::set(TEST_BACKEND_MMAP_ONLY);
            analyze_git_repo(dir.path()).unwrap()
        };

        let legacy_analysis = {
            let _guard = BackendOverrideGuard::set(TEST_BACKEND_LEGACY_ONLY);
            analyze_git_repo(dir.path()).unwrap()
        };

        assert_eq!(mmap_analysis.head_hex, legacy_analysis.head_hex);
        assert_eq!(mmap_analysis.commit_count, legacy_analysis.commit_count);
        let mmap_messages: Vec<&str> =
            mmap_analysis.commits.iter().map(|c| c.message.as_str()).collect();
        let legacy_messages: Vec<&str> =
            legacy_analysis.commits.iter().map(|c| c.message.as_str()).collect();
        assert_eq!(mmap_messages, legacy_messages);
    }

    #[test]
    fn test_commit_graph_and_legacy_traversal_are_parity_equivalent() {
        let dir = create_commit_graph_repo();

        let graph_analysis = {
            let _guard =
                BackendOverrideGuard::set_with(TEST_BACKEND_AUTO, TEST_TRAVERSAL_COMMIT_GRAPH_ONLY);
            analyze_git_repo(dir.path()).unwrap()
        };

        let legacy_analysis = {
            let _guard =
                BackendOverrideGuard::set_with(TEST_BACKEND_AUTO, TEST_TRAVERSAL_LEGACY_ONLY);
            analyze_git_repo(dir.path()).unwrap()
        };

        assert_eq!(graph_analysis.head_hex, legacy_analysis.head_hex);
        assert_eq!(graph_analysis.commit_count, legacy_analysis.commit_count);
        let graph_oids: Vec<GitOid> = graph_analysis.commits.iter().map(|c| c.oid).collect();
        let legacy_oids: Vec<GitOid> = legacy_analysis.commits.iter().map(|c| c.oid).collect();
        assert_eq!(graph_oids, legacy_oids);
        let graph_messages: Vec<&str> =
            graph_analysis.commits.iter().map(|c| c.message.as_str()).collect();
        let legacy_messages: Vec<&str> =
            legacy_analysis.commits.iter().map(|c| c.message.as_str()).collect();
        assert_eq!(graph_messages, legacy_messages);
    }

    #[test]
    fn test_stale_commit_graph_falls_back_to_legacy() {
        let dir = create_commit_graph_repo();
        let path = dir.path();

        std::fs::write(path.join("post_graph.txt"), "latest\n").unwrap();
        git(&["add", "."], path);
        git(&["commit", "-m", "post-graph"], path);

        let analysis = analyze_git_repo(path).unwrap();
        assert_eq!(analysis.commits.last().unwrap().message, "post-graph");

        let legacy = {
            let _guard =
                BackendOverrideGuard::set_with(TEST_BACKEND_AUTO, TEST_TRAVERSAL_LEGACY_ONLY);
            analyze_git_repo(path).unwrap()
        };
        assert_eq!(analysis.head_hex, legacy.head_hex);
        assert_eq!(analysis.commit_count, legacy.commit_count);
        let analysis_messages: Vec<&str> =
            analysis.commits.iter().map(|c| c.message.as_str()).collect();
        let legacy_messages: Vec<&str> =
            legacy.commits.iter().map(|c| c.message.as_str()).collect();
        assert_eq!(analysis_messages, legacy_messages);
    }
}
