//! Revset parser module.

/// Lazy revset compiler and iterators.
pub mod engine;
/// PEG-based revset parser and AST conversion.
pub mod parser;

pub use engine::{
    ReferenceResolver, RevsetChangeIdIterator, RevsetEvaluator, RevsetIterator, compile,
    compile_change_ids, compile_change_ids_with_refs,
};
pub use parser::{RevsetExpression, parse};
