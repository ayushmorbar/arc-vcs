use std::collections::HashMap;

use tree_sitter::{Node, Parser};

use crate::algebra::{ASTNode, Atom, NodePath};
use crate::ast::LanguagePlugin;

/// Rust language plugin backed by `tree-sitter-rust`.
pub struct RustPlugin;

impl RustPlugin {
    pub fn new() -> Self {
        Self
    }
}

impl Default for RustPlugin {
    fn default() -> Self {
        Self::new()
    }
}

impl LanguagePlugin for RustPlugin {
    fn name(&self) -> &str {
        "rust"
    }

    fn parse(&self, source: &str) -> Result<tree_sitter::Tree, String> {
        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_rust::LANGUAGE.into())
            .map_err(|e| format!("failed to set language: {e}"))?;
        parser
            .parse(source, None)
            .ok_or_else(|| "tree-sitter parse returned None".to_string())
    }

    fn diff(&self, old_src: &str, new_src: &str) -> Result<Vec<Atom>, String> {
        let old_tree = self.parse(old_src)?;
        let new_tree = self.parse(new_src)?;

        let old_map = collect_leaf_nodes(old_tree.root_node(), old_src.as_bytes());
        let new_map = collect_leaf_nodes(new_tree.root_node(), new_src.as_bytes());

        let mut atoms = Vec::new();

        // Keys in old but not in new → Delete
        for path in old_map.keys() {
            if !new_map.contains_key(path) {
                atoms.push(Atom::Delete { at: path.clone() });
            }
        }

        // Keys in new but not in old → Insert
        for (path, content) in &new_map {
            if !old_map.contains_key(path) {
                atoms.push(Atom::Insert {
                    at: path.clone(),
                    content: content.clone(),
                });
            }
        }

        // Keys in both but content differs → Delete + Insert
        for (path, old_content) in &old_map {
            if let Some(new_content) = new_map.get(path)
                && old_content != new_content
            {
                atoms.push(Atom::Delete { at: path.clone() });
                atoms.push(Atom::Insert {
                    at: path.clone(),
                    content: new_content.clone(),
                });
            }
        }

        // Sort atoms for deterministic output.
        atoms.sort_by(|a, b| {
            fn key(atom: &Atom) -> &NodePath {
                match atom {
                    Atom::Insert { at, .. }
                    | Atom::Delete { at }
                    | Atom::SemanticsPreserving { at, .. } => at,
                    Atom::Move { from, .. } => from,
                }
            }
            key(a).cmp(key(b))
        });

        Ok(atoms)
    }
}

/// Generate a semantic `NodePath` for a tree-sitter node by walking up to the root.
///
/// For each ancestor, checks `node.child_by_field_name("name")` (or `"pattern"`)
/// to extract a semantic name — producing segments like `"function_item[main]"`.
/// Nodes without a semantic name use their kind plus a sibling-index suffix
/// (e.g. `"block#0"`) to disambiguate.
pub fn generate_path(node: Node, source: &[u8]) -> NodePath {
    let mut segments = Vec::new();
    let mut current = node;

    loop {
        let segment = node_segment(&current, source);
        segments.push(segment);
        match current.parent() {
            Some(p) => current = p,
            None => break,
        }
    }

    segments.reverse();
    segments
}

/// Create a single path segment for a node.
///
/// If the node has a `name` (or `pattern`) child field, use its text to
/// produce a segment like `"function_item[main]"`. Otherwise, count
/// same-kind preceding siblings to produce `"block#0"`.
fn node_segment(node: &Node, source: &[u8]) -> String {
    let kind = node.kind();

    // Try to find a semantic name from a `name` or `pattern` child field.
    if let Some(name_node) = node
        .child_by_field_name("name")
        .or_else(|| node.child_by_field_name("pattern"))
    {
        let name_text = name_node.utf8_text(source).unwrap_or("?");
        return format!("{kind}[{name_text}]");
    }

    // For nodes without a semantic name, count same-kind preceding siblings.
    if let Some(parent) = node.parent() {
        let mut idx = 0u32;
        let mut cursor = parent.walk();
        for child in parent.children(&mut cursor) {
            if child.id() == node.id() {
                return format!("{kind}#{idx}");
            }
            if child.kind() == kind {
                idx += 1;
            }
        }
    }

    kind.to_string()
}

/// Walk the tree and collect all *leaf* nodes into a map of `NodePath → content bytes`.
///
/// A leaf is any node with `child_count() == 0`. The content (`ASTNode` / `Vec<u8>`)
/// is extracted by slicing `source[node.start_byte()..node.end_byte()]`.
fn collect_leaf_nodes(root: Node, source: &[u8]) -> HashMap<NodePath, ASTNode> {
    let mut map = HashMap::new();
    let mut stack = vec![root];

    while let Some(node) = stack.pop() {
        if node.child_count() == 0 {
            let path = generate_path(node, source);
            let content = source[node.start_byte()..node.end_byte()].to_vec();
            map.insert(path, content);
        } else {
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                stack.push(child);
            }
        }
    }

    map
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::LanguagePlugin;

    #[test]
    fn test_rust_ast_diff() {
        let plugin = RustPlugin::new();
        let old_src = "fn main() { let x = 1; }";
        let new_src = "fn main() { let x = 1; let y = 2; }";

        let atoms = plugin.diff(old_src, new_src).unwrap();

        let has_insert = atoms.iter().any(|a| matches!(a, Atom::Insert { .. }));
        assert!(
            has_insert,
            "expected at least one Insert atom for `let y = 2;`, got: {atoms:?}"
        );

        let has_y_insert = atoms.iter().any(|a| {
            if let Atom::Insert { content, .. } = a {
                String::from_utf8_lossy(content).contains('y')
            } else {
                false
            }
        });
        assert!(
            has_y_insert,
            "expected an Insert containing 'y', got: {atoms:?}"
        );
    }

    #[test]
    fn test_path_generation() {
        let plugin = RustPlugin::new();
        let src = "fn main() { let x = 1; }";
        let tree = plugin.parse(src).unwrap();

        let root = tree.root_node();
        let fn_node = root.child(0).expect("expected function_item");
        let path = generate_path(fn_node, src.as_bytes());

        let has_main = path.iter().any(|seg| seg.contains("main"));
        assert!(
            has_main,
            "function path must contain 'main', got: {path:?}"
        );

        let has_fn_kind = path.iter().any(|seg| seg.contains("function_item"));
        assert!(
            has_fn_kind,
            "path must contain 'function_item', got: {path:?}"
        );
    }

    #[test]
    fn test_no_diff_identical_sources() {
        let plugin = RustPlugin::new();
        let src = r#"fn hello() { println!("hi"); }"#;
        let atoms = plugin.diff(src, src).unwrap();
        assert!(
            atoms.is_empty(),
            "identical sources must produce no atoms, got: {atoms:?}"
        );
    }

    #[test]
    fn test_delete_atom_on_removal() {
        let plugin = RustPlugin::new();
        let old_src = "fn main() { let x = 1; let y = 2; }";
        let new_src = "fn main() { let x = 1; }";

        let atoms = plugin.diff(old_src, new_src).unwrap();

        let has_delete = atoms.iter().any(|a| matches!(a, Atom::Delete { .. }));
        assert!(
            has_delete,
            "expected at least one Delete atom when removing `let y = 2;`, got: {atoms:?}"
        );
    }
}
