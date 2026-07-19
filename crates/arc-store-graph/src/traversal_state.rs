//! Compact per-node traversal metadata using bitflags.

use std::collections::HashMap;
use std::hash::Hash;

use bitflags::bitflags;

bitflags! {
    /// Compact marker bits for DAG traversal passes.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
    pub struct TraversalBits: u8 {
        /// Node has been seen in the current traversal.
        const SEEN = 1 << 0;
        /// Node was queued for processing.
        const QUEUED = 1 << 1;
        /// Node is known common between sides.
        const COMMON = 1 << 2;
        /// Node has been fully processed.
        const POPPED = 1 << 3;
        /// Node was advertised from a trusted seed set.
        const ADVERTISED = 1 << 4;
    }
}

/// Metadata tracked per node during high-throughput traversals.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TraversalMeta {
    /// Original time-to-live assigned to this node.
    pub original_ttl: u16,
    /// Mutable time-to-live value adjusted while traversing.
    pub ttl: u16,
    /// Compact traversal flags.
    pub bits: TraversalBits,
}

/// Sparse map of node metadata keyed by node id.
#[derive(Debug, Clone, Default)]
pub struct TraversalState<K> {
    entries: HashMap<K, TraversalMeta>,
}

impl<K> TraversalState<K>
where
    K: Eq + Hash,
{
    /// Create an empty traversal state map.
    pub fn new() -> Self {
        Self { entries: HashMap::new() }
    }

    /// Return metadata for `key`, if present.
    pub fn get(&self, key: &K) -> Option<&TraversalMeta> {
        self.entries.get(key)
    }

    /// Return mutable metadata for `key`, inserting defaults if needed.
    pub fn get_or_default_mut(&mut self, key: K) -> &mut TraversalMeta {
        self.entries.entry(key).or_default()
    }

    /// Mark `key` with `bits`.
    pub fn mark(&mut self, key: K, bits: TraversalBits) {
        self.entries.entry(key).or_default().bits.insert(bits);
    }

    /// Clear `bits` on `key` if it exists.
    pub fn clear(&mut self, key: &K, bits: TraversalBits) {
        if let Some(meta) = self.entries.get_mut(key) {
            meta.bits.remove(bits);
        }
    }

    /// Check if `key` contains all `bits`.
    pub fn contains(&self, key: &K, bits: TraversalBits) -> bool {
        self.entries.get(key).is_some_and(|meta| meta.bits.contains(bits))
    }

    /// Update TTL metadata for `key` if `original_ttl` should increase.
    pub fn refresh_ttl(&mut self, key: K, original_ttl: u16, ttl: u16) {
        let meta = self.entries.entry(key).or_default();
        if original_ttl > meta.original_ttl {
            meta.original_ttl = original_ttl;
            meta.ttl = ttl;
        }
    }

    /// Number of tracked nodes.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Return true if no nodes are tracked.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marks_and_clears_bits() {
        let mut state = TraversalState::<u32>::new();
        state.mark(1, TraversalBits::SEEN | TraversalBits::QUEUED);
        assert!(state.contains(&1, TraversalBits::SEEN));
        assert!(state.contains(&1, TraversalBits::QUEUED));

        state.clear(&1, TraversalBits::QUEUED);
        assert!(state.contains(&1, TraversalBits::SEEN));
        assert!(!state.contains(&1, TraversalBits::QUEUED));
    }

    #[test]
    fn ttl_refresh_only_moves_forward() {
        let mut state = TraversalState::<u32>::new();
        state.refresh_ttl(1, 5, 4);
        state.refresh_ttl(1, 3, 2);

        let meta = state.get(&1).expect("entry should exist");
        assert_eq!(meta.original_ttl, 5);
        assert_eq!(meta.ttl, 4);

        state.refresh_ttl(1, 7, 6);
        let meta = state.get(&1).expect("entry should exist");
        assert_eq!(meta.original_ttl, 7);
        assert_eq!(meta.ttl, 6);
    }

    #[test]
    fn default_state_is_empty() {
        let state = TraversalState::<u32>::new();
        assert!(state.is_empty());
        assert_eq!(state.len(), 0);
    }
}
