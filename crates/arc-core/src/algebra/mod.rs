pub mod apply;
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
    Insert { at: NodePath, content: ASTNode },
    /// Delete the AST node (and its subtree) at the given path.
    Delete { at: NodePath },
    /// Move (rename / refactor) a node from one path to another.
    Move { from: NodePath, to: NodePath },
    /// A semantics-preserving transformation rooted at the given path.
    SemanticsPreserving { at: NodePath, description: String },
    /// Record the existence of an empty directory.
    ///
    /// `arc` tracks bare directories natively so that developers never need
    /// dummy `.gitkeep` files to preserve folder structure.
    Directory { path: NodePath },
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
        }
    }
}
