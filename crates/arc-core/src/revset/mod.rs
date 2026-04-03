//! Revset parser module.

/// PEG-based revset parser and AST conversion.
pub mod parser;

pub use parser::{RevsetExpression, parse};
