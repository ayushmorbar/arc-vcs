//! Git commit synthesis primitives.

use crate::hash::{GitObjectKind, GitOid, git_hash};

/// Author/committer identity line data for Git commit payloads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitSignature {
    /// Display name (e.g. `John Doe`).
    pub name: String,
    /// Email address (without brackets).
    pub email: String,
    /// Unix epoch seconds.
    pub timestamp: i64,
    /// Timezone offset in Git format (e.g. `+0000`, `-0500`).
    pub tz_offset: String,
}

/// Canonical commit payload input for JIT Git synthesis.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitCommit {
    /// Root tree object id.
    pub tree: GitOid,
    /// Parent commit object ids.
    pub parents: Vec<GitOid>,
    /// Author identity/timestamp metadata.
    pub author: GitSignature,
    /// Committer identity/timestamp metadata.
    pub committer: GitSignature,
    /// Commit message bytes interpreted as UTF-8 text.
    pub message: String,
}

/// Synthesize a canonical Git commit payload and its object id.
pub fn synthesize_commit(commit: &GitCommit) -> (Vec<u8>, GitOid) {
    let mut payload = Vec::new();

    payload.extend_from_slice(b"tree ");
    payload.extend_from_slice(commit.tree.to_string().as_bytes());
    payload.push(b'\n');

    for parent in &commit.parents {
        payload.extend_from_slice(b"parent ");
        payload.extend_from_slice(parent.to_string().as_bytes());
        payload.push(b'\n');
    }

    push_signature_line(&mut payload, b"author", &commit.author);
    push_signature_line(&mut payload, b"committer", &commit.committer);
    payload.push(b'\n');
    payload.extend_from_slice(commit.message.as_bytes());

    let oid = git_hash(GitObjectKind::Commit, &payload);
    (payload, oid)
}

fn push_signature_line(out: &mut Vec<u8>, label: &[u8], sig: &GitSignature) {
    out.extend_from_slice(label);
    out.push(b' ');
    out.extend_from_slice(sig.name.as_bytes());
    out.extend_from_slice(b" <");
    out.extend_from_slice(sig.email.as_bytes());
    out.extend_from_slice(b"> ");
    out.extend_from_slice(sig.timestamp.to_string().as_bytes());
    out.push(b' ');
    out.extend_from_slice(sig.tz_offset.as_bytes());
    out.push(b'\n');
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use super::{GitCommit, GitSignature, synthesize_commit};
    use crate::hash::{GitObjectKind, GitOid, git_hash};

    #[test]
    fn standard_commit_matches_oracle_hash() {
        let tree =
            GitOid::from_str("bc225ea23f53f06c0c5bd3ba2be85c2120d68417").expect("valid tree oid");
        let signature = GitSignature {
            name: "John Doe".to_string(),
            email: "john@example.com".to_string(),
            timestamp: 1_700_000_000,
            tz_offset: "+0000".to_string(),
        };
        let commit = GitCommit {
            tree,
            parents: Vec::new(),
            author: signature.clone(),
            committer: signature,
            message: "Initial commit\n".to_string(),
        };

        let (_payload, oid) = synthesize_commit(&commit);
        assert_eq!(oid.to_string(), "6d8d7f05efa6402573bc77cb801dfca82261b952");
    }

    #[test]
    fn commit_with_parents_payload_format() {
        let tree = git_hash(GitObjectKind::Tree, b"");
        let parent = git_hash(GitObjectKind::Commit, b"parent payload");
        let sig = GitSignature {
            name: "Alice".to_string(),
            email: "alice@example.com".to_string(),
            timestamp: 1_700_000_000,
            tz_offset: "+0000".to_string(),
        };
        let commit = GitCommit {
            tree,
            parents: vec![parent],
            author: sig.clone(),
            committer: sig,
            message: "Second commit\n".to_string(),
        };

        let (payload, oid) = synthesize_commit(&commit);
        let payload_str = String::from_utf8_lossy(&payload);
        assert!(payload_str.starts_with("tree "));
        assert!(payload_str.contains(&format!("parent {}", parent)));
        assert!(payload_str.contains("author Alice <alice@example.com>"));
        assert!(payload_str.contains("committer Alice <alice@example.com>"));
        assert!(payload_str.ends_with("\nSecond commit\n"));
        assert_eq!(oid.to_string().len(), 40);
    }

    #[test]
    fn commit_with_two_parents() {
        let tree = git_hash(GitObjectKind::Tree, b"");
        let p1 = git_hash(GitObjectKind::Commit, b"p1");
        let p2 = git_hash(GitObjectKind::Commit, b"p2");
        let sig = GitSignature {
            name: "Bob".to_string(),
            email: "bob@example.com".to_string(),
            timestamp: 1_700_000_100,
            tz_offset: "+0200".to_string(),
        };
        let commit = GitCommit {
            tree,
            parents: vec![p1, p2],
            author: sig.clone(),
            committer: sig,
            message: "Merge commit\n".to_string(),
        };

        let (payload, _oid) = synthesize_commit(&commit);
        let payload_str = String::from_utf8_lossy(&payload);
        let parent_count = payload_str.matches("parent ").count();
        assert_eq!(parent_count, 2);
    }

    #[test]
    fn commit_empty_message() {
        let tree = git_hash(GitObjectKind::Tree, b"");
        let sig = GitSignature {
            name: "X".to_string(),
            email: "x@x".to_string(),
            timestamp: 0,
            tz_offset: "+0000".to_string(),
        };
        let commit = GitCommit {
            tree,
            parents: vec![],
            author: sig.clone(),
            committer: sig,
            message: String::new(),
        };

        let (payload, oid) = synthesize_commit(&commit);
        assert_eq!(oid.to_string().len(), 40);
        assert!(payload.ends_with(b"\n"));
    }

    #[test]
    fn git_signature_struct_fields() {
        let sig = GitSignature {
            name: "Test User".to_string(),
            email: "test@example.com".to_string(),
            timestamp: 9999999999,
            tz_offset: "-0500".to_string(),
        };
        assert_eq!(sig.name, "Test User");
        assert_eq!(sig.email, "test@example.com");
        assert_eq!(sig.timestamp, 9999999999);
        assert_eq!(sig.tz_offset, "-0500");
    }

    #[test]
    fn git_commit_struct_fields() {
        let tree = git_hash(GitObjectKind::Tree, b"");
        let sig = GitSignature {
            name: "Y".to_string(),
            email: "y@y".to_string(),
            timestamp: 1,
            tz_offset: "+0000".to_string(),
        };
        let commit = GitCommit {
            tree,
            parents: vec![],
            author: sig.clone(),
            committer: sig,
            message: "msg\n".to_string(),
        };
        assert_eq!(commit.tree, tree);
        assert!(commit.parents.is_empty());
        assert_eq!(commit.message, "msg\n");
    }

    #[test]
    fn synthesize_commit_deterministic() {
        let sig = GitSignature {
            name: "Deterministic".to_string(),
            email: "d@d".to_string(),
            timestamp: 12345,
            tz_offset: "+0000".to_string(),
        };
        let commit = GitCommit {
            tree: git_hash(GitObjectKind::Tree, b"x"),
            parents: vec![],
            author: sig.clone(),
            committer: sig,
            message: "deterministic\n".to_string(),
        };
        let (_, oid1) = synthesize_commit(&commit);
        let (_, oid2) = synthesize_commit(&commit);
        assert_eq!(oid1, oid2);
    }
}
