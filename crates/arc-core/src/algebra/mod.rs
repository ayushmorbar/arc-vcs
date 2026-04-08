pub use arc_algebra::*;

/// Semantic policy engine: config loading and delta-impact evaluation traits.
pub mod policy;
/// tree-sitter-based delta-impact evaluator.
pub mod evaluator;
/// AI adapter resolver pipeline for policy errors.
pub mod resolver;

pub use arc_algebra_types::{Atom, Blake3Hash, NodePath, SpacetimeCoordinate};
