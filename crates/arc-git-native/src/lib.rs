//! Zero-dependency, in-memory Git object synthesis boundaries for `arc`.
//!
//! This crate defines the JIT translation contract from `arc` DAG state to
//! Git-compatible object streams without depending on third-party Git engines.

#![warn(missing_docs)]

use std::borrow::Cow;

use arc_core::store::ChangeGraph;

/// A 20-byte SHA-1 object id used for Git object addressing.
pub type GitOid = [u8; 20];

/// Kind tag for synthesized Git objects.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GitObjectKind {
    /// Raw file content object.
    Blob,
    /// Directory listing object.
    Tree,
    /// Commit metadata object.
    Commit,
    /// Annotated tag object.
    Tag,
}

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
