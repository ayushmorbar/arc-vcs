use std::collections::HashMap;

use arc_store_types::newtypes::*;

#[test]
fn change_id_hashmap_key() {
    let id1 = ChangeId::from_hex(&"a".repeat(64)).unwrap();
    let id2 = ChangeId::from_hex(&"b".repeat(64)).unwrap();
    let mut map = HashMap::new();
    map.insert(id1, "first");
    map.insert(id2, "second");
    assert_eq!(map.len(), 2);
    assert_eq!(map.get(&id1), Some(&"first"));
    assert_eq!(map.get(&id2), Some(&"second"));
}

#[test]
fn change_id_sortable() {
    let a = ChangeId::from_hex(&"a".repeat(64)).unwrap();
    let b = ChangeId::from_hex(&"b".repeat(64)).unwrap();
    let c = ChangeId::from_hex(&"c".repeat(64)).unwrap();
    let mut ids = [c, a, b];
    ids.sort();
    assert_eq!(ids[0], a);
    assert_eq!(ids[1], b);
    assert_eq!(ids[2], c);
}

#[test]
fn change_id_as_hashmap_key_multiple_entries() {
    let mut map = HashMap::new();
    for i in 0..10u8 {
        let hex = format!("{:02x}", i).repeat(32);
        let id = ChangeId::from_hex(&hex).unwrap();
        map.insert(id, i);
    }
    assert_eq!(map.len(), 10);
}

#[test]
fn ids_display_in_collection() {
    let change = ChangeId::from_hex(&"1".repeat(64)).unwrap();
    let blob = BlobId::from_hex(&"2".repeat(64)).unwrap();
    let snapshot = SnapshotId::from_hex(&"3".repeat(64)).unwrap();
    let displayed: Vec<String> =
        vec![format!("{change}"), format!("{blob}"), format!("{snapshot}")];
    assert_eq!(displayed.len(), 3);
    for d in &displayed {
        assert_eq!(d.len(), 64);
    }
}

#[test]
fn clone_preserves_equality() {
    let id = ChangeId::from_hex(&"f".repeat(64)).unwrap();
    let cloned = id;
    assert_eq!(id, cloned);
    assert_eq!(format!("{id}"), format!("{cloned}"));
}
