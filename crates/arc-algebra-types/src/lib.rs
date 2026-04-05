//! BLUF: `arc-algebra-types` defines the canonical atom vocabulary for `arc`.
//!
//! It provides the pure data model used by higher layers to describe
//! Spacetime-DAG operations as structural AST edits and conflict algebra.
//!
//! ## Purity and I/O boundary
//!
//! This crate is pure compute and data types only:
//! - No filesystem I/O
//! - No network I/O
//! - No cryptographic key material handling
//!
//! ## Why this crate exists
//!
//! CRDT and replay semantics depend on stable, language-agnostic primitives.
//! Centralizing `Atom` and hash aliases here prevents dependency cycles and
//! keeps operation algebra reusable across store, network, and CLI layers.
//!
//! ## Example
//!
//! ```
//! use arc_algebra_types::Atom;
//!
//! let op = Atom::Insert {
//!     at: vec!["file".into(), "src/main.rs".into(), "fn_foo".into()],
//!     content_hash: [0u8; 32],
//! };
//! assert_eq!(op.paths().len(), 1);
//! ```

use serde::{Deserialize, Serialize};

/// BLAKE3 content-addressed identity - 32-byte hash.
pub type Blake3Hash = [u8; 32];

/// Path segments addressing a node inside an AST (e.g. `["fn_foo", "body", "0"]`).
pub type NodePath = Vec<String>;

/// The typed atom vocabulary for AST-level operations.
///
/// Every change is composed of one or more atoms that describe
/// *structural* modifications to an AST - never raw text diffs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Atom {
    /// Insert a new AST node at the given path.
    ///
    /// The node content is stored as a raw blob in `.arc/blobs/{hex(content_hash)}`.
    Insert {
        /// AST path of the insertion point (e.g. `["file", "main.rs", "fn_foo"]`).
        at: NodePath,
        /// BLAKE3 hash of the serialized node content, stored in `.arc/blobs/`.
        content_hash: Blake3Hash,
    },
    /// Delete the AST node (and its subtree) at the given path.
    ///
    /// `prior_hash` is the BLAKE3 hash of the node's content *before* deletion,
    /// enabling lossless inversion.
    Delete {
        /// AST path of the node to remove.
        at: NodePath,
        /// BLAKE3 hash of the removed node's content (for inversion).
        prior_hash: Blake3Hash,
    },
    /// Move (rename / refactor) a node from one path to another.
    Move {
        /// Source path of the node being moved.
        from: NodePath,
        /// Destination path after the move.
        to: NodePath,
    },
    /// A semantics-preserving transformation rooted at the given path.
    SemanticsPreserving {
        /// AST path of the node being reformatted.
        at: NodePath,
        /// Human-readable description of the transformation.
        description: String,
    },
    /// Record the existence of an empty directory.
    Directory {
        /// Path key for the directory (e.g. `["dir", "src/utils"]`).
        path: NodePath,
    },
    /// Track a non-AST binary or text asset by its BLAKE3 content hash.
    Blob {
        /// Path segments identifying the asset (e.g. `["file", "logo.png"]`).
        path: NodePath,
        /// BLAKE3 hash of the raw file bytes.
        hash: Blake3Hash,
    },
    /// Declare that a sub-repository should be mounted at `path`.
    Mount {
        /// Path segments for the mount-point directory.
        path: NodePath,
        /// URL or filesystem path of the remote `arc` repository.
        url: String,
        /// View name to check out inside the mounted sub-repository.
        target: String,
    },
    /// Represents an unresolved N-way conflict as first-class algebra.
    Conflict {
        /// Common ancestor state hashes.
        bases: Vec<Blake3Hash>,
        /// Divergent side state hashes.
        sides: Vec<Blake3Hash>,
        /// AST path where the conflict is anchored.
        at: NodePath,
    },
}

impl Atom {
    /// Returns every AST path this atom touches.
    pub fn paths(&self) -> Vec<&NodePath> {
        match self {
            Atom::Insert { at, .. } => vec![at],
            Atom::Delete { at, .. } => vec![at],
            Atom::Move { from, to } => vec![from, to],
            Atom::SemanticsPreserving { at, .. } => vec![at],
            Atom::Directory { path } => vec![path],
            Atom::Blob { path, .. } => vec![path],
            Atom::Mount { path, .. } => vec![path],
            Atom::Conflict { at, .. } => vec![at],
        }
    }
}
