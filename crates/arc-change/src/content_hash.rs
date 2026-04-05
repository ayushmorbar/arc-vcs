use std::collections::HashSet;
use std::hash::Hash;

use serde::Serialize;

use arc_algebra_types::{Atom, Blake3Hash};
use arc_store_types::author::Author;

pub use arc_content_hash_derive::ContentHash;

/// Stable hasher type used by `ContentHash` implementors.
pub type Hasher = blake3::Hasher;

/// Deterministic field-level hashing contract for content-addressed IDs.
///
/// Implementations must feed the hasher in a stable order across processes
/// and platforms for semantically identical values.
pub trait ContentHash {
    /// Append this value's canonical bytes to an in-progress BLAKE3 hasher.
    fn hash_update(&self, state: &mut Hasher);

    /// Compute this value's standalone BLAKE3 content hash.
    fn content_hash(&self) -> Blake3Hash {
        let mut hasher = Hasher::new();
        self.hash_update(&mut hasher);
        *hasher.finalize().as_bytes()
    }
}

fn hash_serialized<T: Serialize>(value: &T, state: &mut Hasher) {
    let bytes = bincode::serialize(value).expect("serialization must succeed for ContentHash");
    (bytes.len() as u64).hash_update(state);
    state.update(&bytes);
}

impl ContentHash for Atom {
    fn hash_update(&self, state: &mut Hasher) {
        hash_serialized(self, state);
    }
}

impl ContentHash for Author {
    fn hash_update(&self, state: &mut Hasher) {
        hash_serialized(self, state);
    }
}

impl<T: ContentHash + ?Sized> ContentHash for &T {
    fn hash_update(&self, state: &mut Hasher) {
        (*self).hash_update(state);
    }
}

impl ContentHash for bool {
    fn hash_update(&self, state: &mut Hasher) {
        state.update(&[*self as u8]);
    }
}

impl ContentHash for u8 {
    fn hash_update(&self, state: &mut Hasher) {
        state.update(&[*self]);
    }
}

impl ContentHash for u16 {
    fn hash_update(&self, state: &mut Hasher) {
        state.update(&self.to_le_bytes());
    }
}

impl ContentHash for u32 {
    fn hash_update(&self, state: &mut Hasher) {
        state.update(&self.to_le_bytes());
    }
}

impl ContentHash for u64 {
    fn hash_update(&self, state: &mut Hasher) {
        state.update(&self.to_le_bytes());
    }
}

impl ContentHash for i32 {
    fn hash_update(&self, state: &mut Hasher) {
        state.update(&self.to_le_bytes());
    }
}

impl ContentHash for i64 {
    fn hash_update(&self, state: &mut Hasher) {
        state.update(&self.to_le_bytes());
    }
}

impl ContentHash for usize {
    fn hash_update(&self, state: &mut Hasher) {
        (*self as u64).hash_update(state);
    }
}

impl<const N: usize> ContentHash for [u8; N] {
    fn hash_update(&self, state: &mut Hasher) {
        (N as u64).hash_update(state);
        state.update(self);
    }
}

impl ContentHash for str {
    fn hash_update(&self, state: &mut Hasher) {
        let bytes = self.as_bytes();
        (bytes.len() as u64).hash_update(state);
        state.update(bytes);
    }
}

impl ContentHash for String {
    fn hash_update(&self, state: &mut Hasher) {
        self.as_str().hash_update(state);
    }
}

impl<T: ContentHash> ContentHash for [T] {
    fn hash_update(&self, state: &mut Hasher) {
        (self.len() as u64).hash_update(state);
        for item in self {
            item.hash_update(state);
        }
    }
}

impl<T: ContentHash> ContentHash for Vec<T> {
    fn hash_update(&self, state: &mut Hasher) {
        self.as_slice().hash_update(state);
    }
}

impl<T: ContentHash> ContentHash for Option<T> {
    fn hash_update(&self, state: &mut Hasher) {
        match self {
            None => 0u8.hash_update(state),
            Some(value) => {
                1u8.hash_update(state);
                value.hash_update(state);
            }
        }
    }
}

impl<T: ContentHash + Eq + Hash> ContentHash for HashSet<T> {
    fn hash_update(&self, state: &mut Hasher) {
        let mut digests: Vec<Blake3Hash> = self.iter().map(ContentHash::content_hash).collect();
        digests.sort();
        (digests.len() as u64).hash_update(state);
        for digest in digests {
            digest.hash_update(state);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Clone, PartialEq, Eq, ContentHash)]
    struct DemoNode {
        label: String,
        weight: u32,
        enabled: bool,
    }

    #[derive(Debug, Clone, PartialEq, Eq, ContentHash)]
    enum DemoKind {
        Unit,
        Named { name: String },
        Tuple(u32),
    }

    #[test]
    fn hash_set_is_deterministic_across_insertion_order() {
        let mut left = HashSet::new();
        left.insert("a".to_string());
        left.insert("b".to_string());

        let mut right = HashSet::new();
        right.insert("b".to_string());
        right.insert("a".to_string());

        assert_eq!(left.content_hash(), right.content_hash());
    }

    #[test]
    fn option_hash_is_tagged() {
        let none: Option<u32> = None;
        let some = Some(0u32);
        assert_ne!(none.content_hash(), some.content_hash());
    }

    #[test]
    fn derive_struct_hash_is_deterministic() {
        let node = DemoNode {
            label: "arc".to_string(),
            weight: 7,
            enabled: true,
        };
        assert_eq!(node.content_hash(), node.content_hash());
    }

    #[test]
    fn enum_variant_ordinals_separate_same_payload() {
        let named = DemoKind::Named {
            name: "same".to_string(),
        };
        let tuple = DemoKind::Tuple(123);
        assert_ne!(named.content_hash(), tuple.content_hash());
    }

    #[test]
    fn enum_unit_variant_hashes_uniquely() {
        assert_ne!(
            DemoKind::Unit.content_hash(),
            DemoKind::Tuple(0).content_hash()
        );
    }
}
