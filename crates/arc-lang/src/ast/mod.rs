/// Rust language plugin: tree-sitter AST diffing and source reconstruction.
pub mod rust_plugin;

use std::collections::HashMap;

use arc_algebra_types::{Atom, NodePath};
use arc_store_cas::ObjectStore;

/// Trait for language-specific AST parsing and diffing.
///
/// Each supported language implements this trait to provide tree-sitter
/// based parsing and semantic diff generation.
pub trait LanguagePlugin {
    /// The language name (e.g. `"rust"`).
    fn name(&self) -> &str;

    /// Parse source code into a tree-sitter tree.
    fn parse(&self, source: &str) -> Result<tree_sitter::Tree, String>;

    /// Diff two source files and produce a list of atomic AST operations.
    ///
    /// Every `Insert` atom's content is written to `store` as a blob, and the
    /// returned atom carries the resulting `content_hash`. Every `Delete` atom
    /// likewise stores the removed node's bytes in `store` as `prior_hash`.
    fn diff(&self, old_src: &str, new_src: &str, store: &ObjectStore) -> Result<Vec<Atom>, String>;

    /// Reconstruct source code for `filepath` from the materialized state.
    ///
    /// Finds all state entries whose keys start with `["file", filepath]`,
    /// sorts them with a priority order (attributes first, then use
    /// declarations, then everything else alphabetically), and concatenates
    /// the content of each top-level item separated by double newlines.
    fn unparse(&self, state: &HashMap<NodePath, Vec<u8>>, filepath: &str)
    -> Result<String, String>;
}
