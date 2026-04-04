//! Revset parser module.

/// Lazy revset compiler and iterators.
pub mod engine;
/// PEG-based revset parser and AST conversion.
pub mod parser;

pub use engine::{
    RevsetChangeIdIterator, RevsetEvaluator, RevsetIterator, compile, compile_change_ids,
};
pub use parser::{RevsetExpression, parse};
