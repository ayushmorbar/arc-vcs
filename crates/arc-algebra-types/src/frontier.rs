//! BLUF: `Frontier` is an ordered newtype over `HashSet<Blake3Hash>`
//! representing a set of DAG head hashes — the tips of a branch or view.
//!
//! It provides delta-computation primitives for sync negotiation:
//! which changes the peer has that we don't, and vice versa.

extern crate alloc;

#[cfg(test)]
use alloc::format;
use alloc::{vec, vec::Vec};
use core::{
    fmt,
    ops::{BitAnd, BitOr, Sub},
    slice,
};

use serde::{Deserialize, Serialize};

use crate::Blake3Hash;

/// An ordered frontier: the set of head hashes at a DAG's tips.
///
/// This is the canonical representation exchanged during sync to
/// negotiate which changes need to be transferred.
///
/// # Invariants
///
/// - No duplicate hashes (enforced by `HashSet`).
/// - `is_empty()` implies the DAG has no changes or the frontier is unconstrained.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Frontier {
    heads: Vec<Blake3Hash>,
}

impl Frontier {
    /// Create an empty frontier (no heads).
    pub const fn empty() -> Self {
        Self { heads: Vec::new() }
    }

    /// Create a frontier from a single head hash.
    pub fn single(hash: Blake3Hash) -> Self {
        Self { heads: vec![hash] }
    }

    /// Create a frontier from a `Vec` of hashes.
    ///
    /// Duplicates are removed and the result is sorted for deterministic
    /// serialization.
    pub fn new(mut heads: Vec<Blake3Hash>) -> Self {
        heads.sort();
        heads.dedup();
        Self { heads }
    }

    /// Create a frontier from an iterator of hashes.
    pub fn from_hashes<I: IntoIterator<Item = Blake3Hash>>(iter: I) -> Self {
        let heads: Vec<Blake3Hash> = iter.into_iter().collect();
        Self::new(heads)
    }

    /// Return `true` if the frontier contains no heads.
    pub fn is_empty(&self) -> bool {
        self.heads.is_empty()
    }

    /// Number of heads in the frontier.
    pub fn len(&self) -> usize {
        self.heads.len()
    }

    /// Iterate over the head hashes in deterministic order.
    pub fn iter(&self) -> slice::Iter<'_, Blake3Hash> {
        self.heads.iter()
    }

    /// Return the raw head hashes.
    pub fn as_slice(&self) -> &[Blake3Hash] {
        &self.heads
    }

    /// Check whether `hash` is contained in the frontier.
    pub fn contains(&self, hash: &Blake3Hash) -> bool {
        self.heads.iter().any(|h| h == hash)
    }

    /// Compute the set of hashes reachable from `other` but *not* reachable
    /// from `self`, given a reachability function for `self`.
    ///
    /// `reachable_from_self` should return `true` if `hash` is reachable
    /// from any head in `self`.
    ///
    /// Returns `None` if the caller cannot determine reachability (e.g.
    /// the CAS is unreachable).
    pub fn compute_missing<F>(&self, other: &Frontier, reachable_from_self: F) -> Vec<Blake3Hash>
    where
        F: Fn(&Blake3Hash) -> bool,
    {
        other.heads.iter().filter(|h| !reachable_from_self(h)).copied().collect()
    }

    /// Merge two frontiers: union of their heads.
    pub fn merge(&self, other: &Frontier) -> Frontier {
        let combined: Vec<Blake3Hash> =
            self.heads.iter().chain(other.heads.iter()).copied().collect();
        Self::new(combined)
    }

    /// Returns `true` if every head in `other` is reachable from some
    /// head in `self`.
    pub fn covers<F>(&self, other: &Frontier, reachable_from_self: F) -> bool
    where
        F: Fn(&Blake3Hash) -> bool,
    {
        other.heads.iter().all(reachable_from_self)
    }

    /// Compute the intersection of two frontiers (head hashes in both).
    pub fn intersection(&self, other: &Frontier) -> Frontier {
        Self::new(self.heads.iter().filter(|h| other.contains(h)).copied().collect())
    }

    /// Compute the difference: heads in `self` that are not in `other`.
    pub fn difference(&self, other: &Frontier) -> Frontier {
        Self::new(self.heads.iter().filter(|h| !other.contains(h)).copied().collect())
    }
}

// ── Operator overloads ────────────────────────────────────────────────

impl BitOr for &Frontier {
    type Output = Frontier;

    fn bitor(self, rhs: Self) -> Self::Output {
        self.merge(rhs)
    }
}

impl BitAnd for &Frontier {
    type Output = Frontier;

    fn bitand(self, rhs: Self) -> Self::Output {
        self.intersection(rhs)
    }
}

impl Sub for &Frontier {
    type Output = Frontier;

    fn sub(self, rhs: Self) -> Self::Output {
        self.difference(rhs)
    }
}

// ── Display ───────────────────────────────────────────────────────────

impl fmt::Display for Frontier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Frontier(")?;
        for (i, h) in self.heads.iter().enumerate() {
            if i > 0 {
                write!(f, " ")?;
            }
            for b in h.iter().take(8) {
                write!(f, "{b:02x}")?;
            }
            write!(f, "…")?;
        }
        write!(f, ")")
    }
}

// ── Default ───────────────────────────────────────────────────────────

impl Default for Frontier {
    fn default() -> Self {
        Self::empty()
    }
}

// ── IntoIterator ──────────────────────────────────────────────────────

impl IntoIterator for Frontier {
    type Item = Blake3Hash;
    type IntoIter = alloc::vec::IntoIter<Blake3Hash>;

    fn into_iter(self) -> Self::IntoIter {
        self.heads.into_iter()
    }
}

// ── From conversions ──────────────────────────────────────────────────

impl From<Vec<Blake3Hash>> for Frontier {
    fn from(v: Vec<Blake3Hash>) -> Self {
        Self::new(v)
    }
}

impl From<Frontier> for Vec<Blake3Hash> {
    fn from(f: Frontier) -> Self {
        f.heads
    }
}

// ── Tests ─────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_frontier() {
        let f = Frontier::empty();
        assert!(f.is_empty());
        assert_eq!(f.len(), 0);
    }

    #[test]
    fn single_head() {
        let h = [1u8; 32];
        let f = Frontier::single(h);
        assert_eq!(f.len(), 1);
        assert!(f.contains(&h));
    }

    #[test]
    fn new_deduplicates() {
        let h = [1u8; 32];
        let f = Frontier::new(vec![h, h, h]);
        assert_eq!(f.len(), 1);
    }

    #[test]
    fn merge_union() {
        let a = Frontier::single([1u8; 32]);
        let b = Frontier::single([2u8; 32]);
        let merged = a.merge(&b);
        assert_eq!(merged.len(), 2);
    }

    #[test]
    fn merge_overlapping() {
        let a = Frontier::single([1u8; 32]);
        let b = Frontier::new(vec![[1u8; 32], [2u8; 32]]);
        let merged = a.merge(&b);
        assert_eq!(merged.len(), 2);
    }

    #[test]
    fn intersection_basic() {
        let a = Frontier::new(vec![[1u8; 32], [2u8; 32]]);
        let b = Frontier::new(vec![[2u8; 32], [3u8; 32]]);
        let inter = a.intersection(&b);
        assert_eq!(inter.len(), 1);
        assert!(inter.contains(&[2u8; 32]));
    }

    #[test]
    fn difference_basic() {
        let a = Frontier::new(vec![[1u8; 32], [2u8; 32]]);
        let b = Frontier::single([2u8; 32]);
        let diff = a.difference(&b);
        assert_eq!(diff.len(), 1);
        assert!(diff.contains(&[1u8; 32]));
    }

    #[test]
    fn compute_missing_basic() {
        let local = Frontier::single([1u8; 32]);
        let remote = Frontier::new(vec![[1u8; 32], [2u8; 32]]);
        let missing = local.compute_missing(&remote, |h| local.contains(h));
        assert_eq!(missing, vec![[2u8; 32]]);
    }

    #[test]
    fn covers_all_heads() {
        let local = Frontier::new(vec![[1u8; 32], [2u8; 32]]);
        let remote = Frontier::single([1u8; 32]);
        assert!(local.covers(&remote, |h| local.contains(h)));
    }

    #[test]
    fn covers_missing_head() {
        let local = Frontier::single([1u8; 32]);
        let remote = Frontier::new(vec![[1u8; 32], [2u8; 32]]);
        assert!(!local.covers(&remote, |h| local.contains(h)));
    }

    #[test]
    fn operator_or() {
        let a = Frontier::single([1u8; 32]);
        let b = Frontier::single([2u8; 32]);
        let c = &a | &b;
        assert_eq!(c.len(), 2);
    }

    #[test]
    fn operator_and() {
        let a = Frontier::new(vec![[1u8; 32], [2u8; 32]]);
        let b = Frontier::new(vec![[2u8; 32], [3u8; 32]]);
        let c = &a & &b;
        assert_eq!(c.len(), 1);
        assert!(c.contains(&[2u8; 32]));
    }

    #[test]
    fn operator_sub() {
        let a = Frontier::new(vec![[1u8; 32], [2u8; 32]]);
        let b = Frontier::single([2u8; 32]);
        let c = &a - &b;
        assert_eq!(c.len(), 1);
        assert!(c.contains(&[1u8; 32]));
    }

    #[test]
    fn display_format() {
        let f = Frontier::single([0xAA; 32]);
        let s = format!("{f}");
        assert!(s.contains("Frontier("));
        assert!(s.contains("aa"));
    }

    #[test]
    fn default_is_empty() {
        let f = Frontier::default();
        assert!(f.is_empty());
    }

    #[test]
    fn into_iter_yields_all() {
        let f = Frontier::new(vec![[1u8; 32], [2u8; 32]]);
        let collected: Vec<_> = f.into_iter().collect();
        assert_eq!(collected.len(), 2);
    }

    #[test]
    fn from_vec_deduplicates() {
        let f: Frontier = vec![[1u8; 32], [1u8; 32]].into();
        assert_eq!(f.len(), 1);
    }
}
