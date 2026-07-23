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

#![no_std]

extern crate alloc;

use alloc::{
    format,
    string::{String, ToString},
    vec,
    vec::Vec,
};
use core::fmt;

use serde::{Deserialize, Serialize};

/// BLAKE3 content-addressed identity - 32-byte hash.
pub type Blake3Hash = [u8; 32];

/// Path segments addressing a node inside an AST (e.g. `["fn_foo", "body", "0"]`).
pub type NodePath = Vec<String>;

/// Address of a mounted sub-graph in the global arc spacetime.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpacetimeCoordinate {
    /// Organization, team, or tenant namespace.
    pub namespace: String,
    /// Repository name inside `namespace`.
    pub repo: String,
    /// Target content-addressed head hash.
    pub hash: blake3::Hash,
}

impl SpacetimeCoordinate {
    /// Parse `arc://<namespace>/<repo>@<64-hex-hash>` into a coordinate.
    pub fn from_uri(uri: &str) -> Result<Self, String> {
        let body = uri
            .strip_prefix("arc://")
            .ok_or_else(|| "coordinate must start with 'arc://'".to_string())?;
        let (repo_part, hash_hex) =
            body.split_once('@').ok_or_else(|| "coordinate must include '@<hash>'".to_string())?;
        let (namespace, repo) = repo_part
            .split_once('/')
            .ok_or_else(|| "coordinate must include '<namespace>/<repo>'".to_string())?;
        if repo.contains('/') {
            return Err("coordinate repo must not contain '/'".to_string());
        }
        if namespace.is_empty() || repo.is_empty() {
            return Err("namespace and repo must be non-empty".to_string());
        }
        let hash = blake3::Hash::from_hex(hash_hex)
            .map_err(|_| "coordinate hash must be 64 lowercase/uppercase hex chars".to_string())?;
        Ok(Self { namespace: namespace.to_string(), repo: repo.to_string(), hash })
    }

    /// Render this coordinate as `arc://<namespace>/<repo>@<hash>`.
    pub fn to_uri(&self) -> String {
        format!("arc://{}/{}@{}", self.namespace, self.repo, self.hash.to_hex())
    }
}

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
        /// Repository-relative file path identifying the asset (e.g. `assets/logo.png`).
        path: String,
        /// BLAKE3 hash of the raw file bytes.
        hash: blake3::Hash,
        /// Blob size in bytes.
        size: u64,
    },
    /// Declare that a sub-graph should be mounted at `path`.
    Mount {
        /// Path segments for the mount-point directory.
        path: NodePath,
        /// Spacetime coordinate of the mounted repository graph.
        coordinate: SpacetimeCoordinate,
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

/// Compatibility alias for older naming that referred to semantic atoms.
pub type SemanticAtom = Atom;

impl Atom {
    /// Returns every AST path this atom touches.
    pub fn paths(&self) -> Vec<&NodePath> {
        match self {
            Atom::Insert { at, .. } => vec![at],
            Atom::Delete { at, .. } => vec![at],
            Atom::Move { from, to } => vec![from, to],
            Atom::SemanticsPreserving { at, .. } => vec![at],
            Atom::Directory { path } => vec![path],
            Atom::Blob { .. } => Vec::new(),
            Atom::Mount { path, .. } => vec![path],
            Atom::Conflict { at, .. } => vec![at],
        }
    }
}

impl fmt::Display for Atom {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Atom::Insert { at, .. } => write!(f, "Insert {}", at.join("/")),
            Atom::Delete { at, .. } => write!(f, "Delete {}", at.join("/")),
            Atom::Move { from, to } => write!(f, "Move {} -> {}", from.join("/"), to.join("/")),
            Atom::SemanticsPreserving { at, description } => {
                write!(f, "SemanticsPreserving {} {}", at.join("/"), description)
            }
            Atom::Directory { path } => write!(f, "Directory {}", path.join("/")),
            Atom::Blob { path, size, .. } => write!(f, "Blob {} {}B", path, size),
            Atom::Mount { path, coordinate } => {
                write!(f, "Mount {} {}", path.join("/"), coordinate.to_uri())
            }
            Atom::Conflict { bases, sides, at } => {
                write!(f, "Conflict {}/{} {}", bases.len(), sides.len(), at.join("/"))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── SpacetimeCoordinate ──────────────────────────────────────────────

    #[test]
    fn spacetime_coordinate_roundtrip_uri() {
        let uri = "arc://org/repo@aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let coord = SpacetimeCoordinate::from_uri(uri).expect("coordinate should parse");
        assert_eq!(coord.namespace, "org");
        assert_eq!(coord.repo, "repo");
        assert_eq!(coord.to_uri(), uri);
    }

    #[test]
    fn spacetime_coordinate_rejects_nested_repo() {
        let uri =
            "arc://org/repo/sub@aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        assert!(SpacetimeCoordinate::from_uri(uri).is_err());
    }

    #[test]
    fn spacetime_coordinate_from_uri_no_arc_prefix() {
        let result = SpacetimeCoordinate::from_uri("foo/bar@abcd");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("arc://"));
    }

    #[test]
    fn spacetime_coordinate_from_uri_no_hash_separator() {
        let result = SpacetimeCoordinate::from_uri("arc://org/repo");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("@"));
    }

    #[test]
    fn spacetime_coordinate_from_uri_no_namespace_repo() {
        let result = SpacetimeCoordinate::from_uri(
            "arc://norepo@aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        );
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("namespace"));
    }

    #[test]
    fn spacetime_coordinate_from_uri_empty_namespace() {
        let result = SpacetimeCoordinate::from_uri(
            "arc:///@aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        );
        assert!(result.is_err());
    }

    #[test]
    fn spacetime_coordinate_from_uri_invalid_hash() {
        let result = SpacetimeCoordinate::from_uri("arc://org/repo@not-a-hash");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("hex"));
    }

    // ── Atom::paths() ────────────────────────────────────────────────────

    #[test]
    fn atom_paths_insert() {
        let atom =
            Atom::Insert { at: vec!["a".to_string(), "b".to_string()], content_hash: [0u8; 32] };
        assert_eq!(atom.paths(), vec![&vec!["a".to_string(), "b".to_string()]]);
    }

    #[test]
    fn atom_paths_delete() {
        let atom = Atom::Delete { at: vec!["x".to_string()], prior_hash: [0u8; 32] };
        assert_eq!(atom.paths(), vec![&vec!["x".to_string()]]);
    }

    #[test]
    fn atom_paths_move() {
        let atom = Atom::Move { from: vec!["old".to_string()], to: vec!["new".to_string()] };
        let paths = atom.paths();
        assert_eq!(paths.len(), 2);
        assert_eq!(paths[0], &vec!["old".to_string()]);
        assert_eq!(paths[1], &vec!["new".to_string()]);
    }

    #[test]
    fn atom_paths_directory() {
        let atom = Atom::Directory { path: vec!["d".to_string()] };
        assert_eq!(atom.paths(), vec![&vec!["d".to_string()]]);
    }

    #[test]
    fn atom_paths_blob() {
        let atom =
            Atom::Blob { path: "x.png".into(), hash: blake3::Hash::from_bytes([0u8; 32]), size: 0 };
        assert!(atom.paths().is_empty());
    }

    #[test]
    fn atom_paths_mount() {
        let atom = Atom::Mount {
            path: vec!["m".to_string()],
            coordinate: SpacetimeCoordinate {
                namespace: "n".into(),
                repo: "r".into(),
                hash: blake3::Hash::from_bytes([0u8; 32]),
            },
        };
        assert_eq!(atom.paths(), vec![&vec!["m".to_string()]]);
    }

    #[test]
    fn atom_paths_conflict() {
        let atom = Atom::Conflict { bases: vec![], sides: vec![], at: vec!["c".to_string()] };
        assert_eq!(atom.paths(), vec![&vec!["c".to_string()]]);
    }

    #[test]
    fn atom_paths_semantics_preserving() {
        let atom =
            Atom::SemanticsPreserving { at: vec!["sp".to_string()], description: "fmt".into() };
        assert_eq!(atom.paths(), vec![&vec!["sp".to_string()]]);
    }

    // ── Debug trait ──────────────────────────────────────────────────────

    #[test]
    fn atom_debug_format() {
        let atom = Atom::Insert { at: vec!["f".to_string()], content_hash: [1u8; 32] };
        let debug = format!("{atom:?}");
        assert!(debug.contains("Insert"));
    }

    // ── SpacetimeCoordinate Debug/Display ────────────────────────────────

    #[test]
    fn spacetime_coordinate_debug_format() {
        let coord = SpacetimeCoordinate {
            namespace: "n".into(),
            repo: "r".into(),
            hash: blake3::Hash::from_bytes([0u8; 32]),
        };
        let debug = format!("{coord:?}");
        assert!(debug.contains("SpacetimeCoordinate"));
    }

    // ── Display for Atom ────────────────────────────────────────────────

    #[test]
    fn atom_display_insert() {
        let atom = Atom::Insert {
            at: vec!["file".to_string(), "main.rs".to_string()],
            content_hash: [0u8; 32],
        };
        let s = format!("{atom}");
        assert!(s.contains("Insert"));
        assert!(s.contains("main.rs"));
    }

    #[test]
    fn atom_display_delete() {
        let atom = Atom::Delete {
            at: vec!["file".to_string(), "old.rs".to_string()],
            prior_hash: [0u8; 32],
        };
        let s = format!("{atom}");
        assert!(s.contains("Delete"));
        assert!(s.contains("old.rs"));
    }

    #[test]
    fn atom_display_move() {
        let atom = Atom::Move { from: vec!["a.rs".to_string()], to: vec!["b.rs".to_string()] };
        let s = format!("{atom}");
        assert!(s.contains("Move"));
        assert!(s.contains("a.rs"));
        assert!(s.contains("b.rs"));
    }

    #[test]
    fn atom_display_directory() {
        let atom = Atom::Directory { path: vec!["dir".to_string()] };
        let s = format!("{atom}");
        assert!(s.contains("Directory"));
    }

    #[test]
    fn atom_display_blob() {
        let atom = Atom::Blob {
            path: "img.png".into(),
            hash: blake3::Hash::from_bytes([0u8; 32]),
            size: 1024,
        };
        let s = format!("{atom}");
        assert!(s.contains("Blob"));
    }

    #[test]
    fn atom_display_mount() {
        let atom = Atom::Mount {
            path: vec!["lib".to_string()],
            coordinate: SpacetimeCoordinate {
                namespace: "org".into(),
                repo: "dep".into(),
                hash: blake3::Hash::from_bytes([0u8; 32]),
            },
        };
        let s = format!("{atom}");
        assert!(s.contains("Mount"));
    }

    #[test]
    fn atom_display_conflict() {
        let atom = Atom::Conflict {
            bases: vec![[0u8; 32]],
            sides: vec![[1u8; 32]],
            at: vec!["file".to_string(), "conflict.rs".to_string()],
        };
        let s = format!("{atom}");
        assert!(s.contains("Conflict"));
    }

    #[test]
    fn atom_display_semantics_preserving() {
        let atom = Atom::SemanticsPreserving {
            at: vec!["file".to_string(), "fmt.rs".to_string()],
            description: "reformat".into(),
        };
        let s = format!("{atom}");
        assert!(s.contains("SemanticsPreserving"));
    }
}
