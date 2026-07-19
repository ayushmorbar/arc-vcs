//! Git tree synthesis primitives.

use std::cmp::Ordering;

use crate::hash::{GitObjectKind, GitOid, git_hash};

/// A single entry in a synthesized Git tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitTreeEntry {
    /// File mode in Git tree format (e.g. 0o100644 for regular files).
    pub mode: u32,
    /// Entry name (path component only).
    pub name: String,
    /// Target object id for this entry.
    pub oid: GitOid,
}

impl GitTreeEntry {
    #[inline]
    fn is_dir(&self) -> bool {
        self.mode == 0o040000
    }
}

impl Ord for GitTreeEntry {
    fn cmp(&self, other: &Self) -> Ordering {
        let name_cmp =
            cmp_git_tree_sort_key(&self.name, self.is_dir(), &other.name, other.is_dir());
        if name_cmp != Ordering::Equal {
            return name_cmp;
        }

        // Deterministic tie-breakers for total ordering.
        self.mode.cmp(&other.mode).then_with(|| self.oid.as_bytes().cmp(&other.oid.as_bytes()))
    }
}

impl PartialOrd for GitTreeEntry {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

fn cmp_git_tree_sort_key(a_name: &str, a_is_dir: bool, b_name: &str, b_is_dir: bool) -> Ordering {
    let a = a_name.as_bytes();
    let b = b_name.as_bytes();
    let mut i = 0usize;

    loop {
        let ac = sort_key_byte(a, i, a_is_dir);
        let bc = sort_key_byte(b, i, b_is_dir);
        match (ac, bc) {
            (Some(x), Some(y)) if x == y => {
                i += 1;
            }
            (Some(x), Some(y)) => return x.cmp(&y),
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
        }
    }
}

#[inline]
fn sort_key_byte(name: &[u8], idx: usize, is_dir: bool) -> Option<u8> {
    if idx < name.len() {
        return Some(name[idx]);
    }
    if idx == name.len() && is_dir {
        return Some(b'/');
    }
    None
}

/// Synthesize a canonical Git tree payload and its object id.
pub fn synthesize_tree(mut entries: Vec<GitTreeEntry>) -> (Vec<u8>, GitOid) {
    entries.sort();

    let mut payload = Vec::new();
    for entry in entries {
        let mode = format!("{:o}", entry.mode);
        payload.extend_from_slice(mode.as_bytes());
        payload.push(b' ');
        payload.extend_from_slice(entry.name.as_bytes());
        payload.push(0);
        payload.extend_from_slice(&entry.oid.as_bytes());
    }

    let oid = git_hash(GitObjectKind::Tree, &payload);
    (payload, oid)
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use super::{GitTreeEntry, synthesize_tree};
    use crate::hash::GitOid;

    fn oid(fill: u8) -> GitOid {
        GitOid::from_bytes([fill; 20])
    }

    #[test]
    fn arcane_sort_respects_directory_trailing_slash_rule() {
        let file_foo = GitTreeEntry { mode: 0o100644, name: "foo".to_string(), oid: oid(1) };
        let file_foo_c = GitTreeEntry { mode: 0o100644, name: "foo.c".to_string(), oid: oid(2) };

        let mut file_entries = [file_foo_c.clone(), file_foo.clone()];
        file_entries.sort();
        assert_eq!(file_entries[0].name, "foo");
        assert_eq!(file_entries[1].name, "foo.c");

        let dir_foo = GitTreeEntry { mode: 0o040000, name: "foo".to_string(), oid: oid(3) };
        let mut mixed_entries = [file_foo_c, dir_foo];
        mixed_entries.sort();
        assert_eq!(mixed_entries[0].name, "foo.c");
        assert_eq!(mixed_entries[1].name, "foo");
    }

    #[test]
    fn single_entry_tree_matches_git_oracle() {
        let hello_blob = GitOid::from_str("8ab686eafeb1f44702738c8b0f24f2567c36da6d")
            .expect("valid hello blob oid");
        let entry = GitTreeEntry { mode: 0o100644, name: "hello.txt".to_string(), oid: hello_blob };

        let (_payload, tree_oid) = synthesize_tree(vec![entry]);
        assert_eq!(tree_oid.to_string(), "bc225ea23f53f06c0c5bd3ba2be85c2120d68417");
    }
}
