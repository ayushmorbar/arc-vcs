//! Language plugin infrastructure for arc.
//!
//! This crate provides AST-level diffing and source reconstruction for
//! programming languages. Each language implements the [`ast::LanguagePlugin`]
//! trait backed by [tree-sitter](https://tree-sitter.github.io/tree-sitter/).
//!
//! # Current plugins
//!
//! - [`ast::RustPlugin`] — Rust (`.rs`).
//! - [`ast::TypeScriptPlugin`] — TypeScript (`.ts`, `.tsx`).
//! - [`ast::JavaScriptPlugin`] — JavaScript (`.js`, `.jsx`).
//! - [`ast::PythonPlugin`] — Python (`.py`).
//! - [`ast::JavaPlugin`] — Java (`.java`).
//! - [`ast::CPlugin`] — C (`.c`, `.h`).
//! - [`ast::CppPlugin`] — C++ (`.cpp`, `.cc`, `.hpp`).
//! - [`ast::GoPlugin`] — Go (`.go`).
//! - [`ast::RubyPlugin`] — Ruby (`.rb`).
//! - [`ast::PhpPlugin`] — PHP (`.php`).
//! - [`ast::CSharpPlugin`] — C# (`.cs`).
//! - [`ast::BashPlugin`] — Bash (`.sh`, `.bash`).
//! - [`ast::JsonPlugin`] — JSON (`.json`).
//! - [`ast::fallback::TextFallbackPlugin`] — Fallback for unknown extensions.

#![warn(missing_docs)]

/// AST diffing and source reconstruction across supported programming languages.
pub mod ast;

/// Zero-copy lexical event stream parsing primitives.
pub mod event_stream;

/// Borrowed/owned dual byte-value wrappers.
pub mod value;

pub use value::{UnescapeError, unescape_lazy};
