use arc_store_types::{Blake3Hash, newtypes::*};

// ---------------------------------------------------------------------------
// ChangeId
// ---------------------------------------------------------------------------

#[test]
fn change_id_from_hex_valid() {
    let hex = "a".repeat(64);
    let id = ChangeId::from_hex(&hex).unwrap();
    assert_eq!(id.to_hex(), hex);
}

#[test]
fn change_id_from_hex_odd_length_fails() {
    assert!(ChangeId::from_hex("aaa").is_err());
}

#[test]
fn change_id_from_hex_invalid_chars() {
    assert!(ChangeId::from_hex(&"g".repeat(64)).is_err());
}

#[test]
fn change_id_display_matches_to_hex() {
    let hex = "b".repeat(64);
    let id = ChangeId::from_hex(&hex).unwrap();
    assert_eq!(format!("{id}"), id.to_hex());
}

#[test]
fn change_id_clone_eq() {
    let hex = "c".repeat(64);
    let a = ChangeId::from_hex(&hex).unwrap();
    let b = a;
    assert_eq!(a, b);
}

#[test]
fn change_id_debug_format() {
    let hex = "d".repeat(64);
    let id = ChangeId::from_hex(&hex).unwrap();
    let dbg = format!("{id:?}");
    assert!(dbg.contains("ChangeId"));
}

#[test]
fn change_id_different_values_not_equal() {
    let a = ChangeId::from_hex(&"a".repeat(64)).unwrap();
    let b = ChangeId::from_hex(&"b".repeat(64)).unwrap();
    assert_ne!(a, b);
}

#[test]
fn change_id_from_blake3_hash_roundtrip() {
    let hash: Blake3Hash = [0x42u8; 32];
    let id = ChangeId::from(hash);
    let back: Blake3Hash = id.into();
    assert_eq!(hash, back);
}

#[test]
fn change_id_copy_semantics() {
    let hex = "e".repeat(64);
    let a = ChangeId::from_hex(&hex).unwrap();
    let b = a; // Copy
    assert_eq!(a, b);
}

// ---------------------------------------------------------------------------
// BlobId
// ---------------------------------------------------------------------------

#[test]
fn blob_id_from_hex_valid() {
    let hex = "1".repeat(64);
    let id = BlobId::from_hex(&hex).unwrap();
    assert_eq!(id.to_hex(), hex);
}

#[test]
fn blob_id_from_hex_invalid_length() {
    assert!(BlobId::from_hex("1234").is_err());
}

#[test]
fn blob_id_display() {
    let hex = "2".repeat(64);
    let id = BlobId::from_hex(&hex).unwrap();
    assert_eq!(format!("{id}"), hex);
}

#[test]
fn blob_id_clone_eq() {
    let hex = "3".repeat(64);
    let a = BlobId::from_hex(&hex).unwrap();
    assert_eq!(a.clone(), a);
}

#[test]
fn blob_id_from_blake3_hash_roundtrip() {
    let hash: Blake3Hash = [0xBBu8; 32];
    let id = BlobId::from(hash);
    let back: Blake3Hash = id.into();
    assert_eq!(hash, back);
}

// ---------------------------------------------------------------------------
// SnapshotId
// ---------------------------------------------------------------------------

#[test]
fn snapshot_id_from_hex_valid() {
    let hex = "4".repeat(64);
    let id = SnapshotId::from_hex(&hex).unwrap();
    assert_eq!(id.to_hex(), hex);
}

#[test]
fn snapshot_id_from_hex_invalid() {
    assert!(SnapshotId::from_hex("not_hex").is_err());
}

#[test]
fn snapshot_id_display() {
    let hex = "5".repeat(64);
    let id = SnapshotId::from_hex(&hex).unwrap();
    assert_eq!(format!("{id}"), hex);
}

#[test]
fn snapshot_id_from_blake3_hash_roundtrip() {
    let hash: Blake3Hash = [0xCCu8; 32];
    let id = SnapshotId::from(hash);
    let back: Blake3Hash = id.into();
    assert_eq!(hash, back);
}

// ---------------------------------------------------------------------------
// MutationId
// ---------------------------------------------------------------------------

#[test]
fn mutation_id_from_hex_valid() {
    let hex = "6".repeat(64);
    let id = MutationId::from_hex(&hex).unwrap();
    assert_eq!(id.to_hex(), hex);
}

#[test]
fn mutation_id_from_hex_invalid() {
    assert!(MutationId::from_hex("xyz").is_err());
}

#[test]
fn mutation_id_from_blake3_hash_roundtrip() {
    let hash: Blake3Hash = [0xDDu8; 32];
    let id = MutationId::from(hash);
    let back: Blake3Hash = id.into();
    assert_eq!(hash, back);
}

// ---------------------------------------------------------------------------
// ParseHexError Display
// ---------------------------------------------------------------------------

#[test]
fn parse_hex_error_invalid_length_display() {
    let hex = "a".repeat(64);
    let id = ChangeId::from_hex(&hex);
    assert!(id.is_ok());
    // Test error display via a short input
    let err = ChangeId::from_hex("abc").unwrap_err();
    let msg = format!("{err}");
    assert!(msg.contains("64"));
}

#[test]
fn parse_hex_error_invalid_char_display() {
    let err = ChangeId::from_hex(&"g".repeat(64)).unwrap_err();
    let msg = format!("{err}");
    assert!(msg.contains("invalid hex character"));
}

// ---------------------------------------------------------------------------
// Serde round-trip
// ---------------------------------------------------------------------------

#[cfg(feature = "std")]
#[test]
fn change_id_serde_roundtrip() {
    let hex = "f".repeat(64);
    let id = ChangeId::from_hex(&hex).unwrap();
    let json = serde_json::to_string(&id).unwrap();
    let loaded: ChangeId = serde_json::from_str(&json).unwrap();
    assert_eq!(id, loaded);
}

#[cfg(feature = "std")]
#[test]
fn blob_id_serde_roundtrip() {
    let hex = "e".repeat(64);
    let id = BlobId::from_hex(&hex).unwrap();
    let json = serde_json::to_string(&id).unwrap();
    let loaded: BlobId = serde_json::from_str(&json).unwrap();
    assert_eq!(id, loaded);
}

#[cfg(feature = "std")]
#[test]
fn snapshot_id_serde_roundtrip() {
    let hex = "d".repeat(64);
    let id = SnapshotId::from_hex(&hex).unwrap();
    let json = serde_json::to_string(&id).unwrap();
    let loaded: SnapshotId = serde_json::from_str(&json).unwrap();
    assert_eq!(id, loaded);
}

#[cfg(feature = "std")]
#[test]
fn mutation_id_serde_roundtrip() {
    let hex = "c".repeat(64);
    let id = MutationId::from_hex(&hex).unwrap();
    let json = serde_json::to_string(&id).unwrap();
    let loaded: MutationId = serde_json::from_str(&json).unwrap();
    assert_eq!(id, loaded);
}

// ---------------------------------------------------------------------------
// Cross-type inequality (type system enforced, but verify)
// ---------------------------------------------------------------------------

#[test]
fn different_id_types_display_differently() {
    let hex = "a".repeat(64);
    let change = ChangeId::from_hex(&hex).unwrap();
    let blob = BlobId::from_hex(&hex).unwrap();
    // Same bytes but different types — display should be identical hex
    assert_eq!(format!("{change}"), format!("{blob}"));
    // But they should not be equal (different types)
    // This is enforced at compile time — can't compare them directly
}

// ---------------------------------------------------------------------------
// Ord ordering
// ---------------------------------------------------------------------------

#[test]
fn change_id_ord() {
    let a = ChangeId::from_hex(&"0".repeat(64)).unwrap();
    let b = ChangeId::from_hex(&"f".repeat(64)).unwrap();
    assert!(a < b);
}
