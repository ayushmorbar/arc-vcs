//! Language plugin infrastructure for arc.
//!
//! This crate provides AST-level diffing and source reconstruction for
//! programming languages. Each language implements the [`ast::LanguagePlugin`]
//! trait backed by [tree-sitter](https://tree-sitter.github.io/tree-sitter/).
//!
//! # Current plugins
//!
//! - [`ast::rust_plugin::RustPlugin`] — full support for Rust source files.

#![warn(missing_docs)]

/// AST diffing and source reconstruction across supported programming languages.
pub mod ast;
