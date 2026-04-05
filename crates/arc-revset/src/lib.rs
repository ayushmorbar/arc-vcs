//! BLUF: This crate provides a functional query language (Revsets) for
//! filtering and traversing the `arc` Spacetime DAG. It contains a PEG parser
//! and a lazy evaluation engine.
//!
//! The parser converts user revset strings into an expression AST, and the
//! engine compiles/evaluates those expressions against `ChangeGraph` using
//! lazy iterators for traversal and set operations.
//!
//! # Example
//!
//! ```
//! use arc_revset::parse;
//!
//! let expr = parse("ancestors(@)").expect("revset should parse");
//! let debug = format!("{expr:?}");
//! assert!(debug.contains("ancestors"));
//! ```

#![warn(missing_docs)]

/// Lazy revset compilation and DAG evaluation iterators.
pub mod engine;
/// PEG parser and AST construction for revset expressions.
pub mod parser;

pub use engine::*;
pub use parser::*;
