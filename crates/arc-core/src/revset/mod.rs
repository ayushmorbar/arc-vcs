//! Revset parser module.

/// Lazy revset compiler and iterators.
pub mod engine;
/// PEG-based revset parser and AST conversion.
pub mod parser;

pub use engine::{RevsetIterator, compile};
pub use parser::{RevsetExpression, parse};
