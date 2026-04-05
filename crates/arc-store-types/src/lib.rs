pub mod author;
pub mod newtypes;
pub mod refs;
pub mod tag;

/// Local canonical 32-byte BLAKE3 hash type used by store primitives.
pub type Blake3Hash = [u8; 32];
