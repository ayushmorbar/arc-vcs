use std::collections::{BTreeMap, HashMap};

use anyhow::{Result, anyhow, bail};
use arc_core::algebra::{Atom, Blake3Hash};
use arc_core::store::author::Author;
use arc_core::store::change::Change;

use crate::object::{
    GIT_OBJECT_BLOB, GIT_OBJECT_COMMIT, GIT_OBJECT_TREE, GitIdentity, GitSha1, hash_blob,
    hash_commit, hash_tree,
};

const CONFLICT_EXPORT_ERROR: &str = "Cannot translate unresolved mathematical conflicts to legacy Git snapshots. Please resolve the conflict in arc before exporting.";

/// In-memory object database containing raw Git object bytes keyed by SHA-1.
#[derive(Debug, Default)]
pub struct GitOdb {
    objects: HashMap<GitSha1, (u8, Vec<u8>)>,
}

impl GitOdb {
    pub fn insert(&mut self, id: GitSha1, kind: u8, payload: Vec<u8>) {
        self.objects.insert(id, (kind, payload));
    }

    pub fn get(&self, id: &GitSha1) -> Option<&(u8, Vec<u8>)> {
        self.objects.get(id)
    }

    pub fn len(&self) -> usize {
        self.objects.len()
    }

    pub fn is_empty(&self) -> bool {
        self.objects.is_empty()
    }

    pub fn pack_objects(&self) -> Vec<(u8, &[u8])> {
        let mut ordered: Vec<(&GitSha1, &(u8, Vec<u8>))> = self.objects.iter().collect();
        ordered.sort_by_key(|(id, _)| **id);
        ordered
            .into_iter()
            .map(|(_, (kind, payload))| (*kind, payload.as_slice()))
            .collect()
    }
}

/// Mapping between arc change IDs and translated Git commit IDs.
#[derive(Debug, Default)]
pub struct GitMap {
    map: HashMap<Blake3Hash, GitSha1>,
}

impl GitMap {
    pub fn insert(&mut self, arc: Blake3Hash, git: GitSha1) {
        self.map.insert(arc, git);
    }

    pub fn get(&self, arc: &Blake3Hash) -> Option<GitSha1> {
        self.map.get(arc).copied()
    }
}

#[derive(Debug, Default)]
struct DirNode {
    dirs: BTreeMap<String, DirNode>,
    files: BTreeMap<String, String>,
}

/// Compile a flattened projected file-state map into Git tree/blob objects.
///
/// The input map is `path -> file_content` where paths use `/` separators.
pub fn compile_tree(state: &HashMap<String, String>, odb: &mut GitOdb) -> Result<GitSha1> {
    let root = build_tree(state)?;
    Ok(compile_dir(&root, odb))
}

fn build_tree(state: &HashMap<String, String>) -> Result<DirNode> {
    let mut root = DirNode::default();

    for (path, content) in state {
        let parts: Vec<&str> = path.split('/').filter(|p| !p.is_empty()).collect();
        if parts.is_empty() {
            continue;
        }

        let (parents, file) = parts.split_at(parts.len() - 1);
        let mut current = &mut root;
        for segment in parents {
            if current.files.contains_key(*segment) {
                return Err(anyhow!(
                    "invalid projected state: path segment '{}' is both file and directory",
                    segment
                ));
            }
            current = current.dirs.entry((*segment).to_string()).or_default();
        }

        let file_name = file[0].to_string();
        if current.dirs.contains_key(&file_name) {
            return Err(anyhow!(
                "invalid projected state: path '{}' collides with an existing directory",
                path
            ));
        }
        current.files.insert(file_name, content.clone());
    }

    Ok(root)
}

fn compile_dir(node: &DirNode, odb: &mut GitOdb) -> GitSha1 {
    let mut entries = Vec::new();

    for (name, content) in &node.files {
        let content_bytes = content.as_bytes();
        let (blob_id, payload) = hash_blob(content_bytes);
        odb.insert(blob_id, GIT_OBJECT_BLOB, payload);
        entries.push((name.clone(), blob_id, 0o100644));
    }

    for (name, child) in &node.dirs {
        let tree_id = compile_dir(child, odb);
        entries.push((name.clone(), tree_id, 0o040000));
    }

    let (tree_id, payload) = hash_tree(&entries);
    odb.insert(tree_id, GIT_OBJECT_TREE, payload);
    tree_id
}

/// Compile an arc change + projected tree into a Git commit and record mapping.
pub struct CommitCompileInput<'a> {
    pub change: &'a Change,
    pub root_tree: GitSha1,
    pub parent_commits: &'a [GitSha1],
    pub author: &'a GitIdentity,
    pub committer: &'a GitIdentity,
    pub projected_state_has_conflict: bool,
}

pub fn compile_commit(
    input: CommitCompileInput<'_>,
    odb: &mut GitOdb,
    map: &mut GitMap,
) -> Result<GitSha1> {
    let change = input.change;
    let has_conflict_atom = change
        .atoms
        .iter()
        .any(|atom| matches!(atom, Atom::Conflict { .. }));

    if has_conflict_atom || input.projected_state_has_conflict {
        bail!(CONFLICT_EXPORT_ERROR);
    }

    let trailer_message = format!(
        "{}\n\nArc-Change-Id: blake3:{}\nArc-Author-Type: {}\nArc-Signature: {}",
        change.intent,
        blake3_hex(&change.id),
        arc_author_type(&change.author),
        signature_hex(&change.signature.0),
    );

    let (commit_id, payload) = hash_commit(
        input.root_tree,
        input.parent_commits,
        input.author,
        input.committer,
        &trailer_message,
    );
    odb.insert(commit_id, GIT_OBJECT_COMMIT, payload);
    map.insert(change.id, commit_id);
    Ok(commit_id)
}

fn arc_author_type(author: &Author) -> &'static str {
    match author {
        Author::Human { .. } => "Human",
        Author::AI { .. } => "AI",
        Author::Server { .. } => "Server",
        Author::Transient { .. } => "Transient",
    }
}

fn blake3_hex(hash: &Blake3Hash) -> String {
    hash.iter().map(|b| format!("{b:02x}")).collect()
}

fn signature_hex(signature: &[u8; 64]) -> String {
    signature.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use std::collections::{HashMap, HashSet};

    use arc_core::store::author::test_keypair;

    use super::*;

    #[test]
    fn compiles_tree_and_commit_into_in_memory_odb() {
        let mut state = HashMap::new();
        state.insert("src/main.rs".to_string(), "fn main() {}\n".to_string());
        state.insert("README.md".to_string(), "# arc\n".to_string());

        let mut odb = GitOdb::default();
        let root_tree = compile_tree(&state, &mut odb).unwrap();

        let (author, signing_key) = test_keypair();
        let change = Change::new(
            HashSet::new(),
            vec![Atom::Insert {
                at: vec!["file".to_string(), "README.md".to_string()],
                content_hash: [9u8; 32],
            }],
            "export snapshot",
            author,
            &signing_key,
        );

        let ident = GitIdentity {
            name: "Test User".to_string(),
            email: "test@example.com".to_string(),
            timestamp: 1_766_517_296,
            timezone: "+0000".to_string(),
        };

        let mut map = GitMap::default();
        let commit_id = compile_commit(
            CommitCompileInput {
                change: &change,
                root_tree,
                parent_commits: &[],
                author: &ident,
                committer: &ident,
                projected_state_has_conflict: false,
            },
            &mut odb,
            &mut map,
        )
        .unwrap();

        assert!(odb.get(&commit_id).is_some(), "commit must be stored");
        assert!(odb.get(&root_tree).is_some(), "root tree must be stored");
        assert_eq!(odb.len(), 5, "2 blobs + 2 trees + 1 commit expected");
        assert_eq!(map.get(&change.id), Some(commit_id));

        let (tree_kind, _) = odb.get(&root_tree).expect("tree object must exist");
        assert_eq!(*tree_kind, GIT_OBJECT_TREE);

        let (commit_kind, commit_payload) = odb.get(&commit_id).expect("commit object must exist");
        assert_eq!(*commit_kind, GIT_OBJECT_COMMIT);
        let commit_body = String::from_utf8_lossy(commit_payload);
        assert!(commit_body.contains("Arc-Change-Id: blake3:"));
        assert!(commit_body.contains("Arc-Author-Type: Human"));
        assert!(commit_body.contains("Arc-Signature:"));
    }

    #[test]
    fn blocks_unresolved_conflicts() {
        let (author, signing_key) = test_keypair();
        let change = Change::new(
            HashSet::new(),
            vec![Atom::Conflict {
                bases: vec![[1u8; 32]],
                sides: vec![[2u8; 32], [3u8; 32]],
                at: vec!["file".to_string(), "a.rs".to_string()],
            }],
            "has conflict",
            author,
            &signing_key,
        );

        let ident = GitIdentity {
            name: "Test User".to_string(),
            email: "test@example.com".to_string(),
            timestamp: 1_766_517_296,
            timezone: "+0000".to_string(),
        };

        let mut odb = GitOdb::default();
        let mut map = GitMap::default();
        let err = compile_commit(
            CommitCompileInput {
                change: &change,
                root_tree: GitSha1([0u8; 20]),
                parent_commits: &[],
                author: &ident,
                committer: &ident,
                projected_state_has_conflict: false,
            },
            &mut odb,
            &mut map,
        )
        .unwrap_err();

        assert_eq!(err.to_string(), CONFLICT_EXPORT_ERROR);
    }

    #[test]
    fn blocks_projected_state_conflict_flag() {
        let (author, signing_key) = test_keypair();
        let change = Change::new(
            HashSet::new(),
            vec![Atom::Insert {
                at: vec!["file".to_string(), "ok.rs".to_string()],
                content_hash: [1u8; 32],
            }],
            "projected conflict",
            author,
            &signing_key,
        );

        let ident = GitIdentity {
            name: "Test User".to_string(),
            email: "test@example.com".to_string(),
            timestamp: 1_766_517_296,
            timezone: "+0000".to_string(),
        };

        let mut odb = GitOdb::default();
        let mut map = GitMap::default();
        let err = compile_commit(
            CommitCompileInput {
                change: &change,
                root_tree: GitSha1([0u8; 20]),
                parent_commits: &[],
                author: &ident,
                committer: &ident,
                projected_state_has_conflict: true,
            },
            &mut odb,
            &mut map,
        )
        .unwrap_err();

        assert_eq!(err.to_string(), CONFLICT_EXPORT_ERROR);
    }

    #[test]
    fn rejects_file_directory_name_collisions() {
        let mut state = HashMap::new();
        state.insert("a".to_string(), "file".to_string());
        state.insert("a/b.txt".to_string(), "child".to_string());

        let mut odb = GitOdb::default();
        let err = compile_tree(&state, &mut odb).unwrap_err();
        assert!(
            err.to_string().contains("both file and directory")
                || err
                    .to_string()
                    .contains("collides with an existing directory"),
            "unexpected error: {err}"
        );
    }
}
