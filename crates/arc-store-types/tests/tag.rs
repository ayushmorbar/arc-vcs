use arc_store_types::Blake3Hash;
use arc_store_types::author::test_keypair;
use arc_store_types::newtypes::ChangeId;
use arc_store_types::tag::Tag;

#[test]
fn tag_new_and_verify() {
    let (author, signing_key) = test_keypair();
    let target = [7u8; 32];
    let tag = Tag::new("v1.0.0", target, author, &signing_key);
    assert_eq!(tag.name, "v1.0.0");
    assert_eq!(tag.target, target);
    assert!(tag.verify());
}

#[test]
fn tag_tamper_name_fails_verify() {
    let (author, signing_key) = test_keypair();
    let target = [7u8; 32];
    let mut tag = Tag::new("v1.0.0", target, author, &signing_key);
    tag.name = "evil".into();
    assert!(!tag.verify());
}

#[test]
fn tag_tamper_target_fails_verify() {
    let (author, signing_key) = test_keypair();
    let target = [7u8; 32];
    let mut tag = Tag::new("v1.0.0", target, author, &signing_key);
    tag.target = [99u8; 32];
    assert!(!tag.verify());
}

#[test]
fn tag_clone() {
    let (author, signing_key) = test_keypair();
    let target = [1u8; 32];
    let tag = Tag::new("dev", target, author, &signing_key);
    let cloned = tag.clone();
    assert_eq!(tag, cloned);
}

#[test]
fn tag_debug_format() {
    let (author, signing_key) = test_keypair();
    let target = [2u8; 32];
    let tag = Tag::new("test", target, author, &signing_key);
    let dbg = format!("{tag:?}");
    assert!(dbg.contains("Tag"));
    assert!(dbg.contains("test"));
}

#[test]
fn tag_name_field() {
    let (author, signing_key) = test_keypair();
    let target = [3u8; 32];
    let tag = Tag::new("release-2026", target, author, &signing_key);
    assert_eq!(tag.name, "release-2026");
}

#[test]
fn tag_serde_roundtrip() {
    let (author, signing_key) = test_keypair();
    let target = [4u8; 32];
    let tag = Tag::new("serde-test", target, author, &signing_key);
    let json = serde_json::to_string(&tag).unwrap();
    let loaded: Tag = serde_json::from_str(&json).unwrap();
    assert_eq!(tag, loaded);
    assert!(loaded.verify());
}

#[test]
fn tag_with_human_author() {
    let (author, signing_key) = test_keypair();
    let target = [5u8; 32];
    let tag = Tag::new("human-tag", target, author.clone(), &signing_key);
    match &tag.author {
        arc_store_types::Author::Human { name, .. } => assert_eq!(name, "Test User"),
        _ => panic!("expected Human author"),
    }
    assert!(tag.verify());
}

#[test]
fn tag_target_is_blake3_compatible() {
    let (author, signing_key) = test_keypair();
    let target: Blake3Hash = [0xAB; 32];
    let tag = Tag::new("compat", target, author, &signing_key);
    let change_id = ChangeId::from(tag.target);
    let back: Blake3Hash = change_id.into();
    assert_eq!(tag.target, back);
}
