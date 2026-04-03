use sha1::{Digest, Sha1};

/// 20-byte SHA-1 object id used by Git objects.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Ord, PartialOrd)]
pub struct GitSha1(pub [u8; 20]);

impl GitSha1 {
    /// Lowercase hexadecimal representation.
    pub fn to_hex(self) -> String {
        let mut out = String::with_capacity(40);
        for byte in self.0 {
            out.push_str(&format!("{byte:02x}"));
        }
        out
    }

    /// Parse a 40-character lowercase/uppercase hex object id.
    pub fn from_hex(hex: &str) -> Option<Self> {
        if hex.len() != 40 {
            return None;
        }
        let mut out = [0u8; 20];
        for (idx, chunk) in hex.as_bytes().chunks_exact(2).enumerate() {
            let pair = std::str::from_utf8(chunk).ok()?;
            out[idx] = u8::from_str_radix(pair, 16).ok()?;
        }
        Some(Self(out))
    }
}

/// Hash raw content as a Git blob object.
///
/// Git blob bytes are: `blob <len>\0<content>`.
pub fn hash_blob(content: &[u8]) -> GitSha1 {
    hash_object("blob", content)
}

/// Hash a Git tree from `(name, object_id, mode)` entries.
///
/// Each entry is encoded as: `<mode-octal> <name>\0<20-byte object id>`.
pub fn hash_tree(entries: &[(String, GitSha1, u32)]) -> GitSha1 {
    let mut sorted = entries.to_vec();
    sorted.sort_by(|(name_a, _, mode_a), (name_b, _, mode_b)| {
        tree_compare_key(name_a, *mode_a).cmp(&tree_compare_key(name_b, *mode_b))
    });

    let mut body = Vec::new();
    for (name, id, mode) in sorted {
        body.extend_from_slice(format!("{:o} ", mode).as_bytes());
        body.extend_from_slice(name.as_bytes());
        body.push(0);
        body.extend_from_slice(&id.0);
    }

    hash_object("tree", &body)
}

/// Identity line used by Git commit objects.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitIdentity {
    pub name: String,
    pub email: String,
    pub timestamp: i64,
    pub timezone: String,
}

/// Hash a Git commit object.
///
/// Commit body format:
///
/// ```text
/// tree <tree_sha1>
/// parent <parent_sha1>
/// author <name> <email> <timestamp> <timezone>
/// committer <name> <email> <timestamp> <timezone>
///
/// <message>
/// ```
pub fn hash_commit(
    tree: GitSha1,
    parents: &[GitSha1],
    author: &GitIdentity,
    committer: &GitIdentity,
    message: &str,
) -> GitSha1 {
    let mut body = String::new();
    body.push_str(&format!("tree {}\n", tree.to_hex()));
    for parent in parents {
        body.push_str(&format!("parent {}\n", parent.to_hex()));
    }
    body.push_str(&format!(
        "author {} <{}> {} {}\ncommitter {} <{}> {} {}\n\n{}",
        author.name,
        author.email,
        author.timestamp,
        author.timezone,
        committer.name,
        committer.email,
        committer.timestamp,
        committer.timezone,
        message
    ));

    hash_object("commit", body.as_bytes())
}

fn tree_compare_key(name: &str, mode: u32) -> Vec<u8> {
    let mut key = name.as_bytes().to_vec();
    if mode == 0o040000 {
        key.push(b'/');
    }
    key
}

fn hash_object(kind: &str, body: &[u8]) -> GitSha1 {
    let mut hasher = Sha1::new();
    hasher.update(format!("{kind} {}", body.len()).as_bytes());
    hasher.update([0]);
    hasher.update(body);

    let digest = hasher.finalize();
    let mut out = [0u8; 20];
    out.copy_from_slice(&digest[..20]);
    GitSha1(out)
}

#[cfg(test)]
mod tests {
    use super::{GitIdentity, GitSha1, hash_blob, hash_commit, hash_tree};

    #[test]
    fn hash_blob_matches_known_git_sha1() {
        let id = hash_blob(b"hello world\n");
        assert_eq!(
            id,
            GitSha1::from_hex("3b18e512dba79e4c8300dd08aeb37f8e728b8dad").unwrap()
        );
    }

    #[test]
    fn hash_tree_is_order_insensitive() {
        let file = GitSha1::from_hex("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa").unwrap();
        let dir = GitSha1::from_hex("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb").unwrap();

        let entries_a = vec![
            ("foo.bar".to_string(), file, 0o100644),
            ("foo".to_string(), dir, 0o040000),
        ];
        let entries_b = vec![
            ("foo".to_string(), dir, 0o040000),
            ("foo.bar".to_string(), file, 0o100644),
        ];

        assert_eq!(hash_tree(&entries_a), hash_tree(&entries_b));
    }

    #[test]
    fn hash_commit_supports_root_and_merge_shapes() {
        let tree = GitSha1::from_hex("1111111111111111111111111111111111111111").unwrap();
        let p1 = GitSha1::from_hex("2222222222222222222222222222222222222222").unwrap();
        let p2 = GitSha1::from_hex("3333333333333333333333333333333333333333").unwrap();
        let ident = GitIdentity {
            name: "JJ Fan".to_string(),
            email: "jjfan@example.com".to_string(),
            timestamp: 1_766_517_296,
            timezone: "+0000".to_string(),
        };

        let root = hash_commit(tree, &[], &ident, &ident, "root");
        let merge = hash_commit(tree, &[p1, p2], &ident, &ident, "merge");

        assert_ne!(root, merge);
    }
}
