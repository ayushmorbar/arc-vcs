//! Shared utilities for all tree-sitter language plugins.
//!
//! Provides path generation, top-level item collection, and a [`plugin!`] macro
//! that generates a complete [`LanguagePlugin`] implementation from a single
//! grammar import and unparse priority list.

use std::collections::HashMap;

use arc_algebra_types::{Atom, NodePath};
use arc_store_cas::ObjectStore;
use tree_sitter::Node;

/// Raw AST node content (serialized bytes).
pub(crate) type ASTNode = Vec<u8>;

/// Generate a semantic [`NodePath`] for a tree-sitter node by walking up to the
/// root. For each ancestor, checks `node.child_by_field_name("name")` (or
/// `"pattern"`) to extract a semantic name — producing segments like
/// `"function_item[main]"`. Nodes without a semantic name use their kind plus
/// a sibling-index suffix (e.g. `"block#0"`) to disambiguate.
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
fn node_segment(node: &Node, source: &[u8]) -> String {
    let kind = node.kind();

    if let Some(name_node) =
        node.child_by_field_name("name").or_else(|| node.child_by_field_name("pattern"))
    {
        let name_text = name_node.utf8_text(source).unwrap_or("?");
        return format!("{kind}[{name_text}]");
    }

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

/// Walk the tree and collect all *top-level named* children of the root node.
/// Each entry maps a [`NodePath`] to the node's full source text.
pub fn collect_top_level_items(root: Node, source: &[u8]) -> HashMap<NodePath, ASTNode> {
    let mut map = HashMap::new();
    let mut cursor = root.walk();
    for child in root.named_children(&mut cursor) {
        let path = generate_path(child, source);
        let content = source[child.start_byte()..child.end_byte()].to_vec();
        map.insert(path, content);
    }
    map
}

/// Compute semantic diff atoms from two maps of top-level AST items.
///
/// - Keys in `old_map` but not `new_map` → [`Atom::Delete`]
/// - Keys in `new_map` but not `old_map` → [`Atom::Insert`]
/// - Keys in both but content differs → [`Atom::Delete`] + [`Atom::Insert`]
///
/// All content is written to `store` as blobs and the resulting hashes are
/// carried by the returned atoms. Atoms are sorted for deterministic output.
pub fn diff_atoms(
    old_map: &HashMap<NodePath, ASTNode>,
    new_map: &HashMap<NodePath, ASTNode>,
    store: &ObjectStore,
) -> Result<Vec<Atom>, String> {
    let mut atoms = Vec::new();

    // Keys in old but not in new → Delete
    for (path, old_content) in old_map {
        if !new_map.contains_key(path) {
            let prior_hash = store
                .write_blob(old_content)
                .map_err(|e| format!("CAS write error for Delete at {path:?}: {e}"))?;
            atoms.push(Atom::Delete { at: path.clone(), prior_hash });
        }
    }

    // Keys in new but not in old → Insert
    for (path, content) in new_map {
        if !old_map.contains_key(path) {
            let content_hash = store
                .write_blob(content)
                .map_err(|e| format!("CAS write error for Insert at {path:?}: {e}"))?;
            atoms.push(Atom::Insert { at: path.clone(), content_hash });
        }
    }

    // Keys in both but content differs → Delete + Insert
    for (path, old_content) in old_map {
        if let Some(new_content) = new_map.get(path)
            && old_content != new_content
        {
            let prior_hash = store
                .write_blob(old_content)
                .map_err(|e| format!("CAS write error for Delete at {path:?}: {e}"))?;
            let content_hash = store
                .write_blob(new_content)
                .map_err(|e| format!("CAS write error for Insert at {path:?}: {e}"))?;
            atoms.push(Atom::Delete { at: path.clone(), prior_hash });
            atoms.push(Atom::Insert { at: path.clone(), content_hash });
        }
    }

    // Sort atoms for deterministic output.
    sort_atoms(&mut atoms);

    Ok(atoms)
}

/// Sort atoms deterministically by their path prefix.
pub fn sort_atoms(atoms: &mut [Atom]) {
    atoms.sort_by(|a, b| {
        fn key(atom: &Atom) -> String {
            match atom {
                Atom::Insert { at, .. }
                | Atom::Delete { at, .. }
                | Atom::SemanticsPreserving { at, .. }
                | Atom::Conflict { at, .. } => at.join("/"),
                Atom::Move { from, .. } => from.join("/"),
                Atom::Directory { path } => path.join("/"),
                Atom::Blob { path, .. } => format!("file/{path}"),
                Atom::Mount { path, .. } => path.join("/"),
            }
        }
        key(a).cmp(&key(b))
    });
}

/// Sort unparse items by priority. Each entry in `priorities` is a `(prefix,
/// rank)` tuple — items whose path segment at index 2 starts with `prefix` get
/// `rank`; items not matching any prefix get rank `u8::MAX`.
pub fn sort_unparse_items(
    items: &mut Vec<(&NodePath, &Vec<u8>)>,
    priorities: &[(&str, u8)],
) {
    items.sort_by(|(a, _), (b, _)| {
        fn sort_key<'a>(path: &'a NodePath, priorities: &[(&str, u8)]) -> (u8, &'a NodePath) {
            let kind = path.get(2).map(|s| s.as_str()).unwrap_or("");
            let rank = priorities
                .iter()
                .find(|(prefix, _)| kind.starts_with(prefix))
                .map_or(u8::MAX, |(_, rank)| *rank);
            (rank, path)
        }
        sort_key(a, priorities).cmp(&sort_key(b, priorities))
    });
}

/// Generate a complete [`LanguagePlugin`] implementation.
///
/// # Usage
///
/// ```ignore
/// plugin! {
///     pub struct PythonPlugin;
///     name: "python";
///     language: "python";
///     priority: [("import_statement", 0), ("decorated_definition", 1)];
/// }
/// ```
///
/// This generates the struct, `new()`, `Default`, and the full
/// `LanguagePlugin` trait implementation including `diff` and `unparse`.
#[macro_export]
macro_rules! plugin {
    (
        $(#[$attr:meta])*
        pub struct $name:ident;
        name: $lang_name:expr;
        language: $lang_id:expr;
        priority: [ $( ($prefix:expr, $rank:expr) ),* $(,)? ];
    ) => {
        $(#[$attr])*
        pub struct $name;

        impl $name {
               /// Create a new instance of this plugin.
               pub fn new() -> Self { Self }
        }

        impl Default for $name {
            fn default() -> Self { Self::new() }
        }

        impl $crate::ast::LanguagePlugin for $name {
            fn name(&self) -> &str { $lang_name }

            fn parse(&self, source: &str) -> Result<tree_sitter::Tree, String> {
                let mut parser = tree_sitter::Parser::new();
                let lang = tree_sitter_language_pack::get_language($lang_id)
                    .map_err(|e| format!("failed to get language '{}': {}", $lang_id, e))?;
                parser
                    .set_language(&lang)
                    .map_err(|e| format!("failed to set language: {e}"))?;
                parser
                    .parse(source, None)
                    .ok_or_else(|| "tree-sitter parse returned None".to_string())
            }

            fn diff(
                &self,
                old_src: &str,
                new_src: &str,
                store: &arc_store_cas::ObjectStore,
            ) -> Result<Vec<arc_algebra_types::Atom>, String> {
                let old_tree = self.parse(old_src)?;
                let new_tree = self.parse(new_src)?;

                let old_map =
                    $crate::ast::common::collect_top_level_items(old_tree.root_node(), old_src.as_bytes());
                let new_map =
                    $crate::ast::common::collect_top_level_items(new_tree.root_node(), new_src.as_bytes());

                $crate::ast::common::diff_atoms(&old_map, &new_map, store)
            }

            fn unparse(
                &self,
                state: &std::collections::HashMap<arc_algebra_types::NodePath, Vec<u8>>,
                filepath: &str,
            ) -> Result<String, String> {
                let prefix = ["file".to_string(), filepath.to_string()];

                let mut items: Vec<(&arc_algebra_types::NodePath, &Vec<u8>)> = state
                    .iter()
                    .filter(|(key, _)| key.len() > prefix.len() && key[..prefix.len()] == prefix[..])
                    .collect();

                if items.is_empty() {
                    return Ok(String::new());
                }

                let priorities: &[(&str, u8)] = &[ $( ($prefix, $rank), )*];
                $crate::ast::common::sort_unparse_items(&mut items, priorities);

                let parts: Vec<String> = items
                    .iter()
                    .map(|(_, content)| String::from_utf8_lossy(content).to_string())
                    .collect();

                Ok(parts.join("\n\n"))
            }
        }
    };
}
