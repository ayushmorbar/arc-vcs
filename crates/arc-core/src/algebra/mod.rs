/// Change-application algebra: replaying atoms onto a materialized state.
pub mod apply;
/// Commutativity check: determining whether two changes can be reordered.
pub mod commute;

use serde::{Deserialize, Serialize};

/// BLAKE3 content-addressed identity — 32-byte hash.
pub type Blake3Hash = [u8; 32];

/// Path segments addressing a node inside an AST (e.g. `["fn_foo", "body", "0"]`).
pub type NodePath = Vec<String>;

/// Opaque AST node content stored as serialized bytes.
pub type ASTNode = Vec<u8>;

/// The typed atom vocabulary for AST-level operations.
///
/// Every change is composed of one or more atoms that describe
/// *structural* modifications to an AST — never raw text diffs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Atom {
    /// Insert a new AST node at the given path.
    Insert {
        /// AST path of the insertion point (e.g. `["file", "main.rs", "fn_foo"]`).
        at: NodePath,
        /// Serialized content of the new node.
        content: ASTNode,
    },
    /// Delete the AST node (and its subtree) at the given path.
    Delete {
        /// AST path of the node to remove.
        at: NodePath,
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
}

impl Atom {
    /// Returns every AST path this atom touches.
    /// Used by the commutativity checker to detect overlap.
    pub fn paths(&self) -> Vec<&NodePath> {
        match self {
            Atom::Insert { at, .. } => vec![at],
            Atom::Delete { at } => vec![at],
            Atom::Move { from, to } => vec![from, to],
            Atom::SemanticsPreserving { at, .. } => vec![at],
            Atom::Directory { path } => vec![path],
            Atom::Blob { path, .. } => vec![path],
        }
    }
}
