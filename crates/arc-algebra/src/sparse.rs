use arc_algebra_types::{Atom, NodePath};

/// One normalized sparse include prefix.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SparsePattern(String);

impl SparsePattern {
    /// Create a normalized sparse pattern.
    pub fn new(input: impl AsRef<str>) -> Option<Self> {
        let mut s = input.as_ref().trim().replace('\\', "/");
        while s.starts_with("./") {
            s = s[2..].to_string();
        }
        s = s.trim_start_matches('/').trim_end_matches('/').to_string();
        if s.is_empty() {
            return None;
        }
        Some(Self(s))
    }

    /// Return the normalized pattern text.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Check whether `path` is inside this sparse include prefix.
    pub fn matches_path(&self, path: &str) -> bool {
        let p = normalize_path(path);
        if p == self.0 {
            return true;
        }
        p.starts_with(&self.0) && p.as_bytes().get(self.0.len()) == Some(&b'/')
    }
}

/// Sparse matcher over file-level paths and AST `NodePath` keys.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SparseMatcher {
    patterns: Vec<SparsePattern>,
}

impl SparseMatcher {
    /// Build a matcher from raw sparse patterns.
    ///
    /// Empty pattern lists mean "full checkout" and therefore match all paths.
    pub fn from_patterns(patterns: &[String]) -> Self {
        let patterns = patterns
            .iter()
            .filter_map(SparsePattern::new)
            .collect::<Vec<_>>();
        Self { patterns }
    }

    /// True when no sparse boundary is active.
    pub fn is_full(&self) -> bool {
        self.patterns.is_empty()
    }

    /// Return normalized sparse patterns.
    pub fn patterns(&self) -> &[SparsePattern] {
        &self.patterns
    }

    /// Match a repository-relative file path (e.g. `src/main.rs`).
    pub fn matches_file_path(&self, file_path: &str) -> bool {
        if self.is_full() {
            return true;
        }
        self.patterns.iter().any(|p| p.matches_path(file_path))
    }

    /// Match a semantic materialized-state key (`NodePath`).
    pub fn matches_node_path(&self, path: &NodePath) -> bool {
        if self.is_full() {
            return true;
        }
        if path.len() < 2 {
            return false;
        }
        matches!(path[0].as_str(), "file" | "dir") && self.matches_file_path(&path[1])
    }

    /// Match an atom by checking all paths the atom touches.
    pub fn matches_atom(&self, atom: &Atom) -> bool {
        if self.is_full() {
            return true;
        }
        atom.paths().iter().any(|p| self.matches_node_path(p))
    }
}

fn normalize_path(path: &str) -> String {
    let mut s = path.trim().replace('\\', "/");
    while s.starts_with("./") {
        s = s[2..].to_string();
    }
    s.trim_start_matches('/').trim_end_matches('/').to_string()
}

#[cfg(test)]
mod tests {
    use arc_algebra_types::Atom;

    use super::SparseMatcher;

    #[test]
    fn sparse_boundary_matches_segment_not_prefix_collision() {
        let matcher = SparseMatcher::from_patterns(&["app/".to_string()]);
        assert!(matcher.matches_file_path("app/main.rs"));
        assert!(matcher.matches_file_path("app"));
        assert!(!matcher.matches_file_path("app2/main.rs"));
    }

    #[test]
    fn sparse_matches_semantic_node_path() {
        let matcher = SparseMatcher::from_patterns(&["src".to_string()]);
        assert!(matcher.matches_node_path(&vec![
            "file".to_string(),
            "src/lib.rs".to_string(),
            "fn_foo".to_string()
        ]));
        assert!(!matcher.matches_node_path(&vec![
            "file".to_string(),
            "tests/test.rs".to_string(),
            "fn_t".to_string()
        ]));
    }

    #[test]
    fn sparse_matches_atom_when_any_path_is_in_scope() {
        let matcher = SparseMatcher::from_patterns(&["src".to_string()]);
        let atom = Atom::Insert {
            at: vec![
                "file".to_string(),
                "src/lib.rs".to_string(),
                "fn_foo".to_string(),
            ],
            content_hash: [7u8; 32],
        };
        assert!(matcher.matches_atom(&atom));
    }
}
