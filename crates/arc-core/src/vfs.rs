//! Pure virtual filesystem boundary over arc's BLAKE3 CAS.
//!
//! This trait is the implementation seam for platform adapters such as FUSE
//! (Linux) and ProjFS (Windows). Implementations can project CAS-addressed
//! content into host filesystem views while keeping core semantics stable.

use std::io::Read;
use std::path::Path;

use arc_algebra_types::SpacetimeCoordinate;
use crate::git_types::GitOid;

/// Virtualized metadata returned from a VFS-backed path view.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VfsMetadata {
    /// Backing CAS object id when available.
    pub oid: Option<GitOid>,
    /// Logical byte length for regular files.
    pub len: u64,
    /// Whether this entry is a directory.
    pub is_dir: bool,
    /// Indicates this entry is synthesized by a virtual projection.
    pub is_virtual: bool,
}

/// Virtual filesystem abstraction for projecting CAS objects into working views.
pub trait Vfs {
    /// Project a CAS object into a concrete path.
    fn materialize(&self, oid: &GitOid, path: &Path) -> Result<(), crate::error::Error>;

    /// Stream content directly from CAS without forcing disk materialization.
    fn read_at_oid(&self, oid: &GitOid) -> Box<dyn Read>;

    /// Resolve a mounted sub-graph stream by spacetime coordinate.
    ///
    /// VFS implementations must intercept mounted path resolution and route
    /// reads for `Mount` atoms directly to the CAS backend represented by
    /// `coord`, without requiring intermediate working-copy materialization.
    fn resolve_mount(
        &self,
        coord: &SpacetimeCoordinate,
    ) -> Result<Box<dyn Read>, crate::error::Error>;

    /// Read virtual metadata from the projected namespace.
    fn stat(&self, path: &Path) -> Result<VfsMetadata, crate::error::Error>;
}
