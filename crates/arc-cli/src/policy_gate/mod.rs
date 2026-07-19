//! Semantic policy gate used by CLI ingress checks and AI lens workflows.

#[allow(missing_docs)]
pub mod evaluator;
#[allow(missing_docs)]
pub mod policy;
#[allow(missing_docs)]
pub mod resolver;

pub use evaluator::*;
pub use policy::*;
pub use resolver::*;
