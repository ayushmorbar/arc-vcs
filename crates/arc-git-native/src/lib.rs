//! Zero-dependency, in-memory Git object synthesis boundaries for `arc`.
//!
//! This crate defines the JIT translation contract from `arc` DAG state to
//! Git-compatible object streams without depending on third-party Git engines.

#![warn(missing_docs)]

use std::borrow::Cow;

use arc_core::store::ChangeGraph;

pub mod hash;
pub mod commit;
pub mod tree;

pub use hash::{GitObjectKind, GitOid, git_hash};
pub use commit::{GitCommit, GitSignature, synthesize_commit};
pub use tree::{GitTreeEntry, synthesize_tree};

/// In-memory representation of a synthesized Git object.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitObject<'a> {
    /// Git object id (SHA-1 of canonical header + payload).
    pub oid: GitOid,
    /// Git object kind.
    pub kind: GitObjectKind,
    /// Canonical serialized payload bytes.
    pub payload: Cow<'a, [u8]>,
}

/// JIT synthesis boundary from `arc` change graphs to Git object streams.
///
/// This boundary is infallible by design: implementors must pre-validate
/// inputs and emit a deterministic stream for the provided graph snapshot.
pub trait GitSynthesizer {
    /// Stream type emitted by this synthesizer.
    type Stream<'a>: Iterator<Item = GitObject<'a>>
    where
        Self: 'a;

    /// Synthesize an in-memory stream of Git objects from `arc` graph state.
    fn synthesize<'a>(&'a self, graph: &'a ChangeGraph) -> Self::Stream<'a>;
}
