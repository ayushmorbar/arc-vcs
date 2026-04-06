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
