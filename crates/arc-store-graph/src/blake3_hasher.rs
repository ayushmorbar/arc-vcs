use std::hash::{BuildHasherDefault, Hasher};

use arc_algebra_types::Blake3Hash;

/// Zero-overhead hasher for [`Blake3Hash`] keys.
#[derive(Default)]
pub struct Blake3Hasher(u64);

impl Hasher for Blake3Hasher {
    #[inline]
    fn write(&mut self, bytes: &[u8]) {
        if bytes.len() >= 8 {
            self.0 = u64::from_le_bytes(bytes[..8].try_into().expect("slice length checked"));
        }
    }

    #[inline]
    fn finish(&self) -> u64 {
        self.0
    }
}

/// HashMap optimized for BLAKE3 digest keys.
pub type Blake3HashMap<V> =
    std::collections::HashMap<Blake3Hash, V, BuildHasherDefault<Blake3Hasher>>;

#[cfg(test)]
mod tests {
    use super::*;
    use std::hash::Hasher;

    #[test]
    fn blake3_hasher_default_zeroes() {
        let h = Blake3Hasher::default();
        assert_eq!(h.finish(), 0);
    }

    #[test]
    fn blake3_hasher_write_short_data() {
        let mut h = Blake3Hasher::default();
        h.write(b"ab");
        assert_eq!(h.finish(), 0);
    }

    #[test]
    fn blake3_hasher_write_8_bytes() {
        let mut h = Blake3Hasher::default();
        h.write(&[1, 2, 3, 4, 5, 6, 7, 8]);
        assert_eq!(h.finish(), u64::from_le_bytes([1, 2, 3, 4, 5, 6, 7, 8]));
    }

    #[test]
    fn blake3_hasher_write_longer_data_uses_first_8_bytes() {
        let mut h = Blake3Hasher::default();
        h.write(&[1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12]);
        assert_eq!(h.finish(), u64::from_le_bytes([1, 2, 3, 4, 5, 6, 7, 8]));
    }

    #[test]
    fn blake3_hasher_write_all_zeroes() {
        let mut h = Blake3Hasher::default();
        h.write(&[0u8; 32]);
        assert_eq!(h.finish(), 0);
    }

    #[test]
    fn blake3_hashmap_insert_and_get() {
        let mut map: Blake3HashMap<String> = Blake3HashMap::default();
        let hash: Blake3Hash = [42u8; 32];
        map.insert(hash, "hello".to_string());
        assert_eq!(map.get(&hash).unwrap(), "hello");
    }

    #[test]
    fn blake3_hashmap_contains_key() {
        let mut map: Blake3HashMap<i32> = Blake3HashMap::default();
        let hash: Blake3Hash = [1u8; 32];
        assert!(!map.contains_key(&hash));
        map.insert(hash, 42);
        assert!(map.contains_key(&hash));
    }

    #[test]
    fn blake3_hashmap_len() {
        let mut map: Blake3HashMap<i32> = Blake3HashMap::default();
        assert_eq!(map.len(), 0);
        map.insert([1u8; 32], 1);
        assert_eq!(map.len(), 1);
        map.insert([2u8; 32], 2);
        assert_eq!(map.len(), 2);
    }

    #[test]
    fn blake3_hashmap_overwrite_existing() {
        let mut map: Blake3HashMap<i32> = Blake3HashMap::default();
        let hash: Blake3Hash = [1u8; 32];
        map.insert(hash, 1);
        map.insert(hash, 2);
        assert_eq!(map.get(&hash).unwrap(), &2);
        assert_eq!(map.len(), 1);
    }

    #[test]
    fn blake3_hashmap_iteration() {
        let mut map: Blake3HashMap<i32> = Blake3HashMap::default();
        let h1: Blake3Hash = [1u8; 32];
        let h2: Blake3Hash = [2u8; 32];
        map.insert(h1, 10);
        map.insert(h2, 20);
        let mut values: Vec<i32> = map.values().copied().collect();
        values.sort();
        assert_eq!(values, vec![10, 20]);
    }
}
