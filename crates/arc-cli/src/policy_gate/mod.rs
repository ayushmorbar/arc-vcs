//! Semantic policy gate used by CLI ingress checks and AI lens workflows.

pub mod evaluator;
pub mod policy;
pub mod resolver;

pub use evaluator::*;
pub use policy::*;
pub use resolver::*;
