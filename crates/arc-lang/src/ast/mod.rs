/// Shared utilities for all tree-sitter language plugins.
pub mod common;

/// Text fallback plugin for files without a tree-sitter grammar.
pub mod fallback;

use std::collections::HashMap;

use arc_algebra_types::{Atom, NodePath};
use arc_store_cas::ObjectStore;
/// Re-export shared path generation for backward compatibility.
pub use common::generate_path;

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

// ──────────────────────────────────────────────────────────────────────
// Language plugins generated via the `plugin!` macro from `common.rs`.
// Each plugin defines: struct, `new()`, `Default`, and `LanguagePlugin`.
// ──────────────────────────────────────────────────────────────────────

crate::plugin! {
    /// Rust language plugin.
    pub struct RustPlugin;
    name: "rust";
    language: "rust";
    priority: [
        ("attribute_item", 0),
        ("inner_attribute_item", 0),
        ("use_declaration", 1),
    ];
}

crate::plugin! {
    /// TypeScript language plugin.
    pub struct TypeScriptPlugin;
    name: "typescript";
    language: "typescript";
    priority: [("import_statement", 0), ("import_clause", 1)];
}

crate::plugin! {
    /// Javascript language plugin.
    pub struct JavaScriptPlugin;
    name: "javascript";
    language: "javascript";
    priority: [("import_statement", 0)];
}

crate::plugin! {
    /// Python language plugin.
    pub struct PythonPlugin;
    name: "python";
    language: "python";
    priority: [("import_statement", 0), ("import_from_statement", 0), ("decorated_definition", 1)];
}

crate::plugin! {
    /// Java language plugin.
    pub struct JavaPlugin;
    name: "java";
    language: "java";
    priority: [("import_declaration", 0), ("package_declaration", 1)];
}

crate::plugin! {
    /// C language plugin.
    pub struct CPlugin;
    name: "c";
    language: "c";
    priority: [("preproc_include", 0), ("preproc_def", 0)];
}

crate::plugin! {
    /// C++ language plugin.
    pub struct CppPlugin;
    name: "cpp";
    language: "cpp";
    priority: [("preproc_include", 0), ("preproc_def", 0), ("using_declaration", 1)];
}

crate::plugin! {
    /// Go language plugin.
    pub struct GoPlugin;
    name: "go";
    language: "go";
    priority: [("import_declaration", 0)];
}

crate::plugin! {
    /// Ruby language plugin.
    pub struct RubyPlugin;
    name: "ruby";
    language: "ruby";
    priority: [("call", 0)];
}

crate::plugin! {
    /// PHP language plugin.
    pub struct PhpPlugin;
    name: "php";
    language: "php";
    priority: [("use_declaration", 0), ("namespace_definition", 1)];
}

crate::plugin! {
    /// C# language plugin.
    pub struct CSharpPlugin;
    name: "csharp";
    language: "csharp";
    priority: [("using_directive", 0), ("namespace_declaration", 1)];
}

crate::plugin! {
    /// Bash language plugin.
    pub struct BashPlugin;
    name: "bash";
    language: "bash";
    priority: [];
}

crate::plugin! {
    /// JSON language plugin.
    pub struct JsonPlugin;
    name: "json";
    language: "json";
    priority: [];
}

crate::plugin! {
    /// Swift language plugin.
    pub struct SwiftPlugin;
    name: "swift";
    language: "swift";
    priority: [("import_declaration", 0)];
}

crate::plugin! {
    /// Kotlin language plugin.
    pub struct KotlinPlugin;
    name: "kotlin";
    language: "kotlin";
    priority: [("import_header", 0), ("package_header", 1)];
}

crate::plugin! {
    /// Scala language plugin.
    pub struct ScalaPlugin;
    name: "scala";
    language: "scala";
    priority: [("import_declaration", 0), ("package_clause", 1)];
}

crate::plugin! {
    /// Lua language plugin.
    pub struct LuaPlugin;
    name: "lua";
    language: "lua";
    priority: [("require_statement", 0)];
}

crate::plugin! {
    /// YAML language plugin.
    pub struct YamlPlugin;
    name: "yaml";
    language: "yaml";
    priority: [];
}

crate::plugin! {
    /// TOML language plugin.
    pub struct TomlPlugin;
    name: "toml";
    language: "toml";
    priority: [];
}

crate::plugin! {
    /// HTML language plugin.
    pub struct HtmlPlugin;
    name: "html";
    language: "html";
    priority: [("style_element", 0), ("script_element", 0)];
}

crate::plugin! {
    /// CSS language plugin.
    pub struct CssPlugin;
    name: "css";
    language: "css";
    priority: [("import_statement", 0)];
}

crate::plugin! {
    /// SQL language plugin.
    pub struct SqlPlugin;
    name: "sql";
    language: "sql";
    priority: [];
}

crate::plugin! {
    /// Elixir language plugin.
    pub struct ElixirPlugin;
    name: "elixir";
    language: "elixir";
    priority: [("import", 0), ("require", 0)];
}

crate::plugin! {
    /// R language plugin.
    pub struct RPlugin;
    name: "r";
    language: "r";
    priority: [("library", 0), ("require", 0)];
}

crate::plugin! {
    /// Dockerfile language plugin.
    pub struct DockerfilePlugin;
    name: "dockerfile";
    language: "dockerfile";
    priority: [];
}

crate::plugin! {
    /// Haskell language plugin.
    pub struct HaskellPlugin;
    name: "haskell";
    language: "haskell";
    priority: [("import_declaration", 0)];
}

crate::plugin! {
    /// Perl language plugin.
    pub struct PerlPlugin;
    name: "perl";
    language: "perl";
    priority: [("use_statement", 0), ("require_statement", 0)];
}

crate::plugin! {
    /// Markdown block language plugin.
    pub struct MarkdownBlockPlugin;
    name: "markdown_block";
    language: "markdown";
    priority: [];
}

crate::plugin! {
    /// Markdown inline language plugin.
    pub struct MarkdownInlinePlugin;
    name: "markdown_inline";
    language: "markdown_inline";
    priority: [];
}

// ──────────────────────────────────────────────────────────────────────
// Plugin dispatch: map file extensions to the correct plugin.
// ──────────────────────────────────────────────────────────────────────

/// Return the appropriate [`LanguagePlugin`] for the given file path.
///
/// Dispatches based on file extension. Unknown extensions fall back to
/// [`fallback::TextFallbackPlugin`].
pub fn dispatch_plugin(filepath: &str) -> Box<dyn LanguagePlugin> {
    let path = std::path::Path::new(filepath);
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");

    // Special case: "Dockerfile" (no extension, just filename)
    let filename = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    if filename.eq_ignore_ascii_case("dockerfile") {
        return Box::new(DockerfilePlugin::new());
    }

    match ext {
        "rs" => Box::new(RustPlugin::new()),
        "ts" | "tsx" => Box::new(TypeScriptPlugin::new()),
        "js" | "jsx" => Box::new(JavaScriptPlugin::new()),
        "py" => Box::new(PythonPlugin::new()),
        "java" => Box::new(JavaPlugin::new()),
        "c" | "h" => Box::new(CPlugin::new()),
        "cpp" | "cc" | "cxx" | "hpp" => Box::new(CppPlugin::new()),
        "go" => Box::new(GoPlugin::new()),
        "rb" => Box::new(RubyPlugin::new()),
        "php" => Box::new(PhpPlugin::new()),
        "cs" => Box::new(CSharpPlugin::new()),
        "sh" | "bash" => Box::new(BashPlugin::new()),
        "json" => Box::new(JsonPlugin::new()),
        "swift" => Box::new(SwiftPlugin::new()),
        "kt" | "kts" => Box::new(KotlinPlugin::new()),
        "scala" | "sc" => Box::new(ScalaPlugin::new()),
        "lua" => Box::new(LuaPlugin::new()),
        "yml" | "yaml" => Box::new(YamlPlugin::new()),
        "toml" => Box::new(TomlPlugin::new()),
        "html" | "htm" => Box::new(HtmlPlugin::new()),
        "css" => Box::new(CssPlugin::new()),
        "sql" => Box::new(SqlPlugin::new()),
        "r" | "R" => Box::new(RPlugin::new()),
        "hs" => Box::new(HaskellPlugin::new()),
        "pl" | "pm" => Box::new(PerlPlugin::new()),
        "dockerfile" | "Dockerfile" => Box::new(DockerfilePlugin::new()),
        _ => Box::new(fallback::TextFallbackPlugin::new()),
    }
}
