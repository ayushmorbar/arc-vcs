/// Change-application algebra: replaying atoms onto a materialized state.
pub mod apply;
/// Commutativity check: determining whether two changes can be reordered.
pub mod commute;
/// Inversion algebra: producing the semantic inverse of a [`Change`].
pub mod inverse;
/// Sparse matcher primitives for AST-aware materialization boundaries.
pub mod sparse;

pub use arc_algebra_types::{Atom, Blake3Hash, NodePath};
