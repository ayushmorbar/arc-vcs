use std::collections::HashMap;

use arc_algebra_types::{Atom, NodePath};
use arc_store_cas::ObjectStore;

use crate::ast::{LanguagePlugin, common::sort_atoms};

/// Universal text fallback plugin for unrecognized file extensions.
///
/// When `arc` encounters a file extension it does not have a Tree-sitter
/// grammar for (e.g. `.md`, `.env`, `.txt`), this plugin provides line-level
/// diffing. It generates standard `Insert`/`Delete` atoms at the line level,
/// wrapped in a generic `NodePath`.
pub struct TextFallbackPlugin;

impl TextFallbackPlugin {
    /// Create a new `TextFallbackPlugin` instance.
    pub fn new() -> Self {
        Self
    }
}

impl Default for TextFallbackPlugin {
    fn default() -> Self {
        Self::new()
    }
}

impl LanguagePlugin for TextFallbackPlugin {
    fn name(&self) -> &str {
        "text"
    }

    fn parse(&self, _source: &str) -> Result<tree_sitter::Tree, String> {
        // Fallback plugin does not use tree-sitter parsing.
        // This method should not be called directly; use `diff` instead.
        Err("TextFallbackPlugin does not support tree-sitter parsing".to_string())
    }

    fn diff(&self, old_src: &str, new_src: &str, store: &ObjectStore) -> Result<Vec<Atom>, String> {
        let old_lines: Vec<&str> = old_src.lines().collect();
        let new_lines: Vec<&str> = new_src.lines().collect();

        let mut atoms = Vec::new();

        // Simple line-level diff: find lines in old but not in new (Delete)
        // and lines in new but not in old (Insert).
        // For a production implementation, consider using a proper LCS algorithm,
        // but for now we'll do a straightforward comparison.

        // Track which lines from old are still present in new
        let mut old_used = vec![false; old_lines.len()];
        let mut new_used = vec![false; new_lines.len()];

        // First pass: find exact matches
        for (i, old_line) in old_lines.iter().enumerate() {
            for (j, new_line) in new_lines.iter().enumerate() {
                if !new_used[j] && old_line == new_line {
                    old_used[i] = true;
                    new_used[j] = true;
                    break;
                }
            }
        }

        // Generate Delete atoms for unmatched old lines
        for (i, old_line) in old_lines.iter().enumerate() {
            if !old_used[i] {
                let line_num = i + 1;
                let path: NodePath =
                    vec!["file".to_string(), "line".to_string(), line_num.to_string()];
                let content = old_line.as_bytes();
                let prior_hash = store
                    .write_blob(content)
                    .map_err(|e| format!("CAS write error for Delete at {path:?}: {e}"))?;
                atoms.push(Atom::Delete { at: path, prior_hash });
            }
        }

        // Generate Insert atoms for unmatched new lines
        for (j, new_line) in new_lines.iter().enumerate() {
            if !new_used[j] {
                let line_num = j + 1;
                let path: NodePath =
                    vec!["file".to_string(), "line".to_string(), line_num.to_string()];
                let content = new_line.as_bytes();
                let content_hash = store
                    .write_blob(content)
                    .map_err(|e| format!("CAS write error for Insert at {path:?}: {e}"))?;
                atoms.push(Atom::Insert { at: path, content_hash });
            }
        }

        // Sort atoms for deterministic output
        sort_atoms(&mut atoms);

        Ok(atoms)
    }

    fn unparse(
        &self,
        state: &HashMap<NodePath, Vec<u8>>,
        _filepath: &str,
    ) -> Result<String, String> {
        // Collect all line entries from state
        let mut lines: Vec<(&NodePath, &Vec<u8>)> = state
            .iter()
            .filter(|(key, _)| key.len() >= 2 && key[0] == "file" && key[1] == "line")
            .collect();

        if lines.is_empty() {
            return Ok(String::new());
        }

        // Sort by line number
        lines.sort_by(|(a, _), (b, _)| {
            let num_a = a.get(2).and_then(|s| s.parse::<usize>().ok());
            let num_b = b.get(2).and_then(|s| s.parse::<usize>().ok());
            num_a.cmp(&num_b)
        });

        // Concatenate lines with newlines
        let parts: Vec<String> =
            lines.iter().map(|(_, content)| String::from_utf8_lossy(content).to_string()).collect();

        Ok(parts.join("\n"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::LanguagePlugin;

    fn make_store() -> (tempfile::TempDir, arc_store_cas::ObjectStore) {
        let dir = tempfile::tempdir().unwrap();
        let store = arc_store_cas::ObjectStore::new(dir.path());
        (dir, store)
    }

    #[test]
    fn test_fallback_plugin_name() {
        let plugin = TextFallbackPlugin::new();
        assert_eq!(plugin.name(), "text");
    }

    #[test]
    fn test_fallback_diff_identical() {
        let plugin = TextFallbackPlugin::new();
        let (_dir, store) = make_store();
        let src = "line1\nline2\nline3";
        let atoms = plugin.diff(src, src, &store).unwrap();
        assert!(atoms.is_empty(), "identical sources must produce no atoms");
    }

    #[test]
    fn test_fallback_diff_add_line() {
        let plugin = TextFallbackPlugin::new();
        let (_dir, store) = make_store();
        let old_src = "line1\nline2";
        let new_src = "line1\nline2\nline3";
        let atoms = plugin.diff(old_src, new_src, &store).unwrap();

        let has_insert = atoms.iter().any(|a| matches!(a, Atom::Insert { .. }));
        assert!(has_insert, "expected at least one Insert atom for added line");
    }

    #[test]
    fn test_fallback_diff_remove_line() {
        let plugin = TextFallbackPlugin::new();
        let (_dir, store) = make_store();
        let old_src = "line1\nline2\nline3";
        let new_src = "line1\nline3";
        let atoms = plugin.diff(old_src, new_src, &store).unwrap();

        let has_delete = atoms.iter().any(|a| matches!(a, Atom::Delete { .. }));
        assert!(has_delete, "expected at least one Delete atom for removed line");
    }

    #[test]
    fn test_fallback_unparse() {
        let plugin = TextFallbackPlugin::new();
        let mut state = HashMap::new();
        state.insert(
            vec!["file".to_string(), "line".to_string(), "1".to_string()],
            b"first line".to_vec(),
        );
        state.insert(
            vec!["file".to_string(), "line".to_string(), "2".to_string()],
            b"second line".to_vec(),
        );

        let result = plugin.unparse(&state, "test.txt").unwrap();
        assert_eq!(result, "first line\nsecond line");
    }
}
