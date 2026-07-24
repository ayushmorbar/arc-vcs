use std::fs;

use arc_store_types::{author::test_keypair, newtypes::ChangeId, refs::*, tag::Tag};

#[test]
fn read_tag_heads_empty_dir() {
    let dir = tempfile::tempdir().unwrap();
    let heads = read_tag_heads(dir.path()).unwrap();
    assert!(heads.is_empty());
}

#[test]
fn read_tag_heads_returns_target_change_ids() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let tag_dir = root.join(".arc").join("tags");
    fs::create_dir_all(&tag_dir).unwrap();

    let (author, key) = test_keypair();
    let target = [7u8; 32];
    let tag = Tag::new("v1.0.0", target, author, &key);
    fs::write(tag_dir.join("v1.0.0.json"), serde_json::to_vec_pretty(&tag).unwrap()).unwrap();

    let heads = read_tag_heads(root).unwrap();
    assert!(heads.contains(&ChangeId::from(target)));
}

#[test]
fn read_tag_heads_multiple_tags() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let tag_dir = root.join(".arc").join("tags");
    fs::create_dir_all(&tag_dir).unwrap();

    let (author, key) = test_keypair();
    let t1 = [1u8; 32];
    let t2 = [2u8; 32];
    let tag1 = Tag::new("v1.0", t1, author.clone(), &key);
    let tag2 = Tag::new("v2.0", t2, author, &key);
    fs::write(tag_dir.join("v1.0.json"), serde_json::to_vec(&tag1).unwrap()).unwrap();
    fs::write(tag_dir.join("v2.0.json"), serde_json::to_vec(&tag2).unwrap()).unwrap();

    let heads = read_tag_heads(root).unwrap();
    assert_eq!(heads.len(), 2);
}

#[test]
fn read_tag_heads_from_arc_refs_tags_dir() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let tag_dir = root.join(".arc").join("refs").join("tags");
    fs::create_dir_all(&tag_dir).unwrap();

    let (author, key) = test_keypair();
    let target = [9u8; 32];
    let tag = Tag::new("legacy", target, author, &key);
    fs::write(tag_dir.join("legacy.json"), serde_json::to_vec(&tag).unwrap()).unwrap();

    let heads = read_tag_heads(root).unwrap();
    assert!(heads.contains(&ChangeId::from(target)));
}

#[test]
fn read_remote_branch_heads_empty() {
    let dir = tempfile::tempdir().unwrap();
    let heads = read_remote_branch_heads(dir.path()).unwrap();
    assert!(heads.is_empty());
}

#[test]
fn read_remote_branch_heads_from_hex_file() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let remote_dir = root.join(".arc").join("refs").join("remotes").join("origin");
    fs::create_dir_all(&remote_dir).unwrap();

    let head = [3u8; 32];
    let hex = ChangeId::from(head).to_hex();
    fs::write(remote_dir.join("main"), hex).unwrap();

    let heads = read_remote_branch_heads(root).unwrap();
    assert!(heads.contains(&ChangeId::from(head)));
}

#[test]
fn read_bookmark_heads_empty() {
    let dir = tempfile::tempdir().unwrap();
    let heads = read_bookmark_heads(dir.path()).unwrap();
    assert!(heads.is_empty());
}

#[test]
fn read_bookmark_heads_from_hex_file() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let bm_dir = root.join(".arc").join("refs").join("bookmarks").join("feature");
    fs::create_dir_all(&bm_dir).unwrap();

    let head = [11u8; 32];
    let hex = ChangeId::from(head).to_hex();
    fs::write(bm_dir.join("ui"), hex).unwrap();

    let heads = read_bookmark_heads(root).unwrap();
    assert!(heads.contains(&ChangeId::from(head)));
}

#[test]
fn read_tag_map_returns_name_to_id() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let tag_dir = root.join(".arc").join("tags");
    fs::create_dir_all(&tag_dir).unwrap();

    let (author, key) = test_keypair();
    let target = [5u8; 32];
    let tag = Tag::new("my-tag", target, author, &key);
    fs::write(tag_dir.join("my-tag.json"), serde_json::to_vec(&tag).unwrap()).unwrap();

    let map = read_tag_map(root).unwrap();
    assert!(map.contains_key(&ChangeId::from(target)));
    let names = map.get(&ChangeId::from(target)).unwrap();
    assert!(names.contains(&"my-tag".to_string()));
}

#[test]
fn read_bookmark_map_returns_names() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let bm_dir = root.join(".arc").join("refs").join("bookmarks").join("release");
    fs::create_dir_all(&bm_dir).unwrap();

    let head = [0xAA; 32];
    let hex = ChangeId::from(head).to_hex();
    fs::write(bm_dir.join("stable"), hex).unwrap();

    let map = read_bookmark_map(root).unwrap();
    let names = map.get(&ChangeId::from(head)).unwrap();
    // normalize_ref_name strips base (.arc/refs/bookmarks) → "release/stable"
    assert!(names.iter().any(|n| n.ends_with("stable")));
}

#[test]
fn read_remote_branch_map_returns_correct_mapping() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let remote_dir = root.join(".arc").join("refs").join("remotes").join("origin");
    fs::create_dir_all(&remote_dir).unwrap();

    let target = [0xCD_u8; 32];
    let hex = ChangeId::from(target).to_hex();
    fs::write(remote_dir.join("main"), &hex).unwrap();

    let map = read_remote_branch_map(root).unwrap();
    let id = ChangeId::from(target);
    assert!(map.contains_key(&id));
    let names = map.get(&id).unwrap();
    assert_eq!(names.len(), 1);
    // normalize_ref_name strips the .arc/refs/remotes base → "origin/main"
    assert_eq!(names[0], "origin/main");
}

#[test]
fn multiple_tags_same_change_id_dedup() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let tag_dir = root.join(".arc").join("tags");
    fs::create_dir_all(&tag_dir).unwrap();

    let (author, key) = test_keypair();
    let target = [0xEE_u8; 32];
    let tag1 = Tag::new("release-1", target, author.clone(), &key);
    let tag2 = Tag::new("release-2", target, author, &key);
    fs::write(tag_dir.join("release-1.json"), serde_json::to_vec(&tag1).unwrap()).unwrap();
    fs::write(tag_dir.join("release-2.json"), serde_json::to_vec(&tag2).unwrap()).unwrap();

    let map = read_tag_map(root).unwrap();
    let id = ChangeId::from(target);
    assert!(map.contains_key(&id));
    let names = map.get(&id).unwrap();
    assert_eq!(names.len(), 2);
    assert!(names.contains(&"release-1".to_string()));
    assert!(names.contains(&"release-2".to_string()));
}

#[test]
fn parse_reference_targets_json_string_branch() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let remote_dir = root.join(".arc").join("refs").join("remotes").join("testremote");
    fs::create_dir_all(&remote_dir).unwrap();

    let target = [0x42_u8; 32];
    let hex = ChangeId::from(target).to_hex();
    // Write as a JSON-quoted string so the String branch of parse_reference_targets fires
    let json_str = format!("\"{}\"", hex);
    fs::write(remote_dir.join("branch"), &json_str).unwrap();

    let map = read_remote_branch_map(root).unwrap();
    let id = ChangeId::from(target);
    assert!(map.contains_key(&id), "JSON-quoted hex string should be parsed");
}

#[test]
fn parse_reference_targets_generic_ref_file_branch() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let remote_dir = root.join(".arc").join("refs").join("remotes").join("testremote");
    fs::create_dir_all(&remote_dir).unwrap();

    let target = [0x77_u8; 32];
    let hex = ChangeId::from(target).to_hex();
    let json_obj = format!("{{\"target\": \"{}\"}}", hex);
    fs::write(remote_dir.join("branch"), &json_obj).unwrap();

    let map = read_remote_branch_map(root).unwrap();
    let id = ChangeId::from(target);
    assert!(map.contains_key(&id), "GenericRefFile target field should be extracted");
}

#[test]
fn empty_dirs_return_empty_results() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();

    // Create empty tag dir
    fs::create_dir_all(root.join(".arc").join("tags")).unwrap();
    assert!(read_tag_heads(root).unwrap().is_empty());

    // Create empty bookmark dir
    fs::create_dir_all(root.join(".arc").join("refs").join("bookmarks")).unwrap();
    assert!(read_bookmark_heads(root).unwrap().is_empty());

    // Create empty remotes dir
    fs::create_dir_all(root.join(".arc").join("refs").join("remotes")).unwrap();
    assert!(read_remote_branch_heads(root).unwrap().is_empty());
}

#[test]
fn corrupt_file_handling() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let invalid_bytes = b"\xff\xfe\x00 not-a-valid-hash";

    // Corrupt tag file should be skipped
    let tag_dir = root.join(".arc").join("tags");
    fs::create_dir_all(&tag_dir).unwrap();
    fs::write(tag_dir.join("bad-tag.json"), invalid_bytes).unwrap();
    let heads = read_tag_heads(root).unwrap();
    assert!(heads.is_empty(), "corrupt tag file should be skipped");

    // Corrupt remote ref file should be skipped
    let remote_dir = root.join(".arc").join("refs").join("remotes").join("origin");
    fs::create_dir_all(&remote_dir).unwrap();
    fs::write(remote_dir.join("main"), invalid_bytes).unwrap();
    let heads = read_remote_branch_heads(root).unwrap();
    assert!(heads.is_empty(), "corrupt remote ref should be skipped");

    // Corrupt bookmark ref file should be skipped
    let bm_dir = root.join(".arc").join("refs").join("bookmarks").join("feat");
    fs::create_dir_all(&bm_dir).unwrap();
    fs::write(bm_dir.join("main"), invalid_bytes).unwrap();
    let heads = read_bookmark_heads(root).unwrap();
    assert!(heads.is_empty(), "corrupt bookmark ref should be skipped");
}
