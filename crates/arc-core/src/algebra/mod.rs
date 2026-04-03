/// Change-application algebra: replaying atoms onto a materialized state.
pub mod apply;
/// Commutativity check: determining whether two changes can be reordered.
pub mod commute;
/// Inversion algebra: producing the semantic inverse of a [`Change`].
pub mod inverse;

use serde::{Deserialize, Serialize};

/// BLAKE3 content-addressed identity — 32-byte hash.
pub type Blake3Hash = [u8; 32];

/// Path segments addressing a node inside an AST (e.g. `["fn_foo", "body", "0"]`).
pub type NodePath = Vec<String>;

/// The typed atom vocabulary for AST-level operations.
///
/// Every change is composed of one or more atoms that describe
/// *structural* modifications to an AST — never raw text diffs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Atom {
    /// Insert a new AST node at the given path.
    ///
    /// The node content is stored as a raw blob in `.arc/blobs/{hex(content_hash)}`.
    /// Use [`crate::store::cas::ObjectStore::write_blob`] to create the blob and
    /// obtain the hash before constructing this atom.
    Insert {
        /// AST path of the insertion point (e.g. `["file", "main.rs", "fn_foo"]`).
        at: NodePath,
        /// BLAKE3 hash of the serialized node content, stored in `.arc/blobs/`.
        content_hash: Blake3Hash,
    },
    /// Delete the AST node (and its subtree) at the given path.
    ///
    /// `prior_hash` is the BLAKE3 hash of the node's content *before* deletion,
    /// enabling lossless inversion via [`crate::algebra::inverse::invert_change`].
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
    ///
    /// `arc` tracks bare directories natively so that developers never need
    /// dummy `.gitkeep` files to preserve folder structure.
    Directory {
        /// Path key for the directory (e.g. `["dir", "src/utils"]`).
        path: NodePath,
    },
    /// Track a non-AST binary or text asset by its BLAKE3 content hash.
    ///
    /// Raw bytes live in `.arc/blobs/{hex(hash)}` on disk; the DAG only
    /// stores the 32-byte pointer so large binaries never bloat change objects.
    Blob {
        /// Path segments identifying the asset (e.g. `["file", "logo.png"]`).
        path: NodePath,
        /// BLAKE3 hash of the raw file bytes, used to fetch from `.arc/blobs/`.
        hash: Blake3Hash,
    },
    /// Declare that a sub-repository should be mounted at `path`.
    ///
    /// In the DAG the dependency's `url` and `target` view name are
    /// cryptographically bound into the parent `Change` signature, so they
    /// can never drift out of sync the way Git submodules do.
    /// `write_state_to_working_dir` creates a directory placeholder;
    /// `arc mount sync` fetches and checks out the remote view.
    Mount {
        /// Path segments for the mount-point directory (e.g. `["file", "libs/engine"]`).
        path: NodePath,
        /// URL or filesystem path of the remote `arc` repository.
        url: String,
        /// View name to check out inside the mounted sub-repository.
        target: String,
    },
    /// Represents an unresolved N-way conflict as first-class algebra.
    ///
    /// `bases` and `sides` reference AST snapshots in CAS by BLAKE3 hash.
    /// The anchor path identifies where this conflict projects in the tree.
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
    /// Used by the commutativity checker to detect overlap.
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
