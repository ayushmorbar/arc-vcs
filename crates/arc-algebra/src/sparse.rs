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
        let patterns = patterns.iter().filter_map(SparsePattern::new).collect::<Vec<_>>();
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

    use super::{SparseMatcher, SparsePattern};

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
            at: vec!["file".to_string(), "src/lib.rs".to_string(), "fn_foo".to_string()],
            content_hash: [7u8; 32],
        };
        assert!(matcher.matches_atom(&atom));
    }

    // ── SparsePattern::new normalization ──────────────────────────────────

    #[test]
    fn sparse_pattern_backslash_normalized() {
        let p = SparsePattern::new("src\\lib").unwrap();
        assert_eq!(p.as_str(), "src/lib");
    }

    #[test]
    fn sparse_pattern_leading_slash_stripped() {
        let p = SparsePattern::new("/src/main.rs").unwrap();
        assert_eq!(p.as_str(), "src/main.rs");
    }

    #[test]
    fn sparse_pattern_dot_slash_stripped() {
        let p = SparsePattern::new("./src/main.rs").unwrap();
        assert_eq!(p.as_str(), "src/main.rs");
    }

    #[test]
    fn sparse_pattern_double_dot_slash_stripped() {
        let p = SparsePattern::new("././src/lib.rs").unwrap();
        assert_eq!(p.as_str(), "src/lib.rs");
    }

    #[test]
    fn sparse_pattern_trailing_slash_stripped() {
        let p = SparsePattern::new("src/").unwrap();
        assert_eq!(p.as_str(), "src");
    }

    #[test]
    fn sparse_pattern_empty_returns_none() {
        assert!(SparsePattern::new("").is_none());
        assert!(SparsePattern::new("   ").is_none());
        assert!(SparsePattern::new("/").is_none());
        assert!(SparsePattern::new("./").is_none());
        assert!(SparsePattern::new("///").is_none());
    }

    #[test]
    fn sparse_pattern_whitespace_trimmed() {
        let p = SparsePattern::new("  src/main.rs  ").unwrap();
        assert_eq!(p.as_str(), "src/main.rs");
    }

    // ── SparseMatcher accessors ───────────────────────────────────────────

    #[test]
    fn sparse_matcher_is_full_with_no_patterns() {
        let matcher = SparseMatcher::from_patterns(&[]);
        assert!(matcher.is_full());
    }

    #[test]
    fn sparse_matcher_is_full_with_valid_pattern() {
        let matcher = SparseMatcher::from_patterns(&["src".to_string()]);
        assert!(!matcher.is_full());
    }

    #[test]
    fn sparse_matcher_patterns_accessor() {
        let matcher = SparseMatcher::from_patterns(&["src".to_string(), "lib".to_string()]);
        let patterns = matcher.patterns();
        assert_eq!(patterns.len(), 2);
        assert_eq!(patterns[0].as_str(), "src");
        assert_eq!(patterns[1].as_str(), "lib");
    }

    #[test]
    fn sparse_matcher_filters_empty_patterns() {
        let matcher =
            SparseMatcher::from_patterns(&["src".to_string(), "".to_string(), "lib".to_string()]);
        let patterns = matcher.patterns();
        assert_eq!(patterns.len(), 2);
    }

    // ── SparseMatcher is_full matches all ─────────────────────────────────

    #[test]
    fn sparse_matcher_is_full_matches_all_file_paths() {
        let matcher = SparseMatcher::from_patterns(&[]);
        assert!(matcher.matches_file_path("anything.txt"));
        assert!(matcher.matches_file_path("deep/nested/path.rs"));
    }

    #[test]
    fn sparse_matcher_is_full_matches_all_node_paths() {
        let matcher = SparseMatcher::from_patterns(&[]);
        assert!(matcher.matches_node_path(&vec!["file".into(), "any.rs".into()]));
    }

    #[test]
    fn sparse_matcher_is_full_matches_all_atoms() {
        let matcher = SparseMatcher::from_patterns(&[]);
        let atom = Atom::Insert { at: vec!["f".into()], content_hash: [0u8; 32] };
        assert!(matcher.matches_atom(&atom));
    }

    // ── matches_node_path edge cases ──────────────────────────────────────

    #[test]
    fn sparse_matcher_rejects_short_node_path() {
        let matcher = SparseMatcher::from_patterns(&["src".to_string()]);
        assert!(!matcher.matches_node_path(&vec!["file".into()]));
        assert!(!matcher.matches_node_path(&vec![]));
    }

    #[test]
    fn sparse_matcher_rejects_non_file_dir_prefix() {
        let matcher = SparseMatcher::from_patterns(&["src".to_string()]);
        assert!(!matcher.matches_node_path(&vec!["module".into(), "src/lib.rs".into()]));
    }

    // ── matches_atom with non-matching atom ───────────────────────────────

    #[test]
    fn sparse_matcher_atom_no_match() {
        let matcher = SparseMatcher::from_patterns(&["src".to_string()]);
        let atom = Atom::Insert {
            at: vec!["file".into(), "tests/test.rs".into(), "fn_t".into()],
            content_hash: [0u8; 32],
        };
        assert!(!matcher.matches_atom(&atom));
    }

    #[test]
    fn sparse_matcher_atom_blob_returns_false() {
        let matcher = SparseMatcher::from_patterns(&["src".to_string()]);
        let atom = Atom::Blob {
            path: "img.png".into(),
            hash: blake3::Hash::from_bytes([0u8; 32]),
            size: 0,
        };
        assert!(!matcher.matches_atom(&atom));
    }
}
