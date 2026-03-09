use std::hash::{BuildHasherDefault, Hasher};

use crate::algebra::Blake3Hash;

/// Zero-overhead hasher for [`Blake3Hash`] keys.
///
/// `Blake3Hash` bytes are already uniformly distributed cryptographic output,
/// so we extract the first 8 bytes directly as the `u64` bucket index.
/// This bypasses SipHash mixing entirely — ~10 ns saved per lookup in
/// hot-path DAG traversals (topological sort, ancestor BFS, merge-base).
///
/// # Safety invariant
///
/// Only use this hasher when the key type is `Blake3Hash` (or another
/// cryptographic digest with guaranteed uniform distribution).  DO NOT use
/// it for user-controlled strings or small integer keys — those would
/// cluster and degrade hash-table performance.
#[derive(Default)]
pub struct Blake3Hasher(u64);

impl Hasher for Blake3Hasher {
    #[inline]
    fn write(&mut self, bytes: &[u8]) {
        // A Blake3Hash write always calls write(&[u8; 32]).  We take the
        // first 8 bytes as a little-endian u64 — sufficient entropy for the
        // full 64-bit bucket space.
        if bytes.len() >= 8 {
            // SAFETY: we just checked len >= 8, so the slice index is valid.
            self.0 = u64::from_le_bytes(bytes[..8].try_into().unwrap());
        }
    }

    #[inline]
    fn finish(&self) -> u64 {
        self.0
    }
}

/// A `HashMap` whose keys are [`Blake3Hash`] values, backed by the
/// zero-overhead [`Blake3Hasher`] instead of the default SipHash-1-3.
pub type Blake3HashMap<V> =
    std::collections::HashMap<Blake3Hash, V, BuildHasherDefault<Blake3Hasher>>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_blake3_hasher_roundtrip() {
        let mut map: Blake3HashMap<u32> = Blake3HashMap::default();
        let key: Blake3Hash = *blake3::hash(b"arc").as_bytes();
        map.insert(key, 42);
        assert_eq!(map.get(&key), Some(&42));
    }

    #[test]
    fn test_blake3_hasher_distinct_keys() {
        let mut map: Blake3HashMap<&str> = Blake3HashMap::default();
        let k1: Blake3Hash = *blake3::hash(b"a").as_bytes();
        let k2: Blake3Hash = *blake3::hash(b"b").as_bytes();
        map.insert(k1, "first");
        map.insert(k2, "second");
        assert_eq!(map.get(&k1), Some(&"first"));
        assert_eq!(map.get(&k2), Some(&"second"));
        assert_ne!(k1, k2);
    }
}
