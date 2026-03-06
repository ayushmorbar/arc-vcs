pub mod rust_plugin;

use crate::algebra::Atom;

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
    fn diff(&self, old_src: &str, new_src: &str) -> Result<Vec<Atom>, String>;
}
