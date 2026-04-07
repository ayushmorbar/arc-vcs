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

    use crate::hash::GitOid;

    use super::{GitCommit, GitSignature, synthesize_commit};

    #[test]
    fn standard_commit_matches_oracle_hash() {
        let tree = GitOid::from_str("bc225ea23f53f06c0c5bd3ba2be85c2120d68417")
            .expect("valid tree oid");
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
}
