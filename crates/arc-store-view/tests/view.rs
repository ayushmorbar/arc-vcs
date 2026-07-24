use std::collections::HashSet;

use arc_store_view::view::*;

fn h(byte: u8) -> [u8; 32] {
    [byte; 32]
}

#[test]
fn view_new_and_fields() {
    let heads = HashSet::from([h(1), h(2)]);
    let view = View::new("main", heads.clone());
    assert_eq!(view.name, "main");
    assert_eq!(view.heads, heads);
}

#[test]
fn view_new_into_string() {
    let view = View::new(String::from("feature/auth"), HashSet::new());
    assert_eq!(view.name, "feature/auth");
    assert!(view.heads.is_empty());
}

#[test]
fn view_save_load_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let heads = HashSet::from([h(1), h(2), h(3)]);
    let view = View::new("main", heads);
    view.save(dir.path()).unwrap();
    let loaded = View::load(dir.path(), "main").unwrap();
    assert_eq!(loaded, view);
}

#[test]
fn view_empty_heads_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let view = View::new("empty", HashSet::new());
    view.save(dir.path()).unwrap();
    let loaded = View::load(dir.path(), "empty").unwrap();
    assert!(loaded.heads.is_empty());
}

#[test]
fn view_nested_name_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let heads = HashSet::from([h(7)]);
    let view = View::new("feature/auth/admin", heads);
    view.save(dir.path()).unwrap();
    let loaded = View::load(dir.path(), "feature/auth/admin").unwrap();
    assert_eq!(loaded.name, "feature/auth/admin");
}

#[test]
fn view_load_nonexistent_returns_error() {
    let dir = tempfile::tempdir().unwrap();
    let result = View::load(dir.path(), "ghost");
    assert!(result.is_err());
}

#[test]
fn view_overwrite_save() {
    let dir = tempfile::tempdir().unwrap();
    let v1 = View::new("main", HashSet::from([h(1)]));
    let v2 = View::new("main", HashSet::from([h(1), h(2), h(3)]));
    v1.save(dir.path()).unwrap();
    v2.save(dir.path()).unwrap();
    let loaded = View::load(dir.path(), "main").unwrap();
    assert_eq!(loaded.heads.len(), 3);
}

#[test]
fn view_multiple_views_coexist() {
    let dir = tempfile::tempdir().unwrap();
    let v1 = View::new("main", HashSet::from([h(1)]));
    let v2 = View::new("dev", HashSet::from([h(2)]));
    v1.save(dir.path()).unwrap();
    v2.save(dir.path()).unwrap();
    assert_eq!(View::load(dir.path(), "main").unwrap().heads, HashSet::from([h(1)]));
    assert_eq!(View::load(dir.path(), "dev").unwrap().heads, HashSet::from([h(2)]));
}

#[test]
fn view_serde_roundtrip() {
    let heads = HashSet::from([h(10)]);
    let view = View::new("test", heads);
    let json = serde_json::to_string(&view).unwrap();
    let loaded: View = serde_json::from_str(&json).unwrap();
    assert_eq!(loaded, view);
}

#[test]
fn view_debug_format() {
    let view = View::new("main", HashSet::new());
    let dbg = format!("{view:?}");
    assert!(dbg.contains("View"));
    assert!(dbg.contains("main"));
}

// ---------------------------------------------------------------------------
// merge_sorted_overlay
// ---------------------------------------------------------------------------

#[test]
fn merge_sorted_overlay_disjoint_left_first() {
    let left = vec![View::new("a", HashSet::new())];
    let right = vec![View::new("b", HashSet::new())];
    let merged: Vec<View> =
        merge_sorted_overlay(left, right, |v| v.name.clone(), OverlayPrecedence::Left).collect();
    assert_eq!(merged.len(), 2);
    assert_eq!(merged[0].name, "a");
    assert_eq!(merged[1].name, "b");
}

#[test]
fn merge_sorted_overlay_disjoint_right_first() {
    let left = vec![View::new("b", HashSet::new())];
    let right = vec![View::new("a", HashSet::new())];
    let merged: Vec<View> =
        merge_sorted_overlay(left, right, |v| v.name.clone(), OverlayPrecedence::Right).collect();
    assert_eq!(merged.len(), 2);
    assert_eq!(merged[0].name, "a");
    assert_eq!(merged[1].name, "b");
}

#[test]
fn merge_sorted_overlay_collision_prefers_right() {
    let left = vec![View::new("c", HashSet::from([h(1)]))];
    let right = vec![View::new("c", HashSet::from([h(99)]))];
    let merged: Vec<View> =
        merge_sorted_overlay(left, right, |v| v.name.clone(), OverlayPrecedence::Right).collect();
    assert_eq!(merged.len(), 1);
    assert!(merged[0].heads.contains(&h(99)));
}

#[test]
fn merge_sorted_overlay_collision_prefers_left() {
    let left = vec![View::new("c", HashSet::from([h(1)]))];
    let right = vec![View::new("c", HashSet::from([h(99)]))];
    let merged: Vec<View> =
        merge_sorted_overlay(left, right, |v| v.name.clone(), OverlayPrecedence::Left).collect();
    assert_eq!(merged.len(), 1);
    assert!(merged[0].heads.contains(&h(1)));
}

#[test]
fn merge_sorted_overlay_empty_both() {
    let left: Vec<View> = vec![];
    let right: Vec<View> = vec![];
    let merged: Vec<View> =
        merge_sorted_overlay(left, right, |v| v.name.clone(), OverlayPrecedence::Left).collect();
    assert!(merged.is_empty());
}

#[test]
fn merge_sorted_overlay_empty_left() {
    let right = vec![View::new("x", HashSet::new())];
    let merged: Vec<View> =
        merge_sorted_overlay(vec![], right, |v| v.name.clone(), OverlayPrecedence::Left).collect();
    assert_eq!(merged.len(), 1);
    assert_eq!(merged[0].name, "x");
}

#[test]
fn merge_sorted_overlay_empty_right() {
    let left = vec![View::new("x", HashSet::new())];
    let merged: Vec<View> =
        merge_sorted_overlay(left, vec![], |v| v.name.clone(), OverlayPrecedence::Left).collect();
    assert_eq!(merged.len(), 1);
    assert_eq!(merged[0].name, "x");
}

// ---------------------------------------------------------------------------
// load_views_with_overlay
// ---------------------------------------------------------------------------

#[test]
fn load_views_with_overlay_empty_persisted() {
    let dir = tempfile::tempdir().unwrap();
    let overlay = vec![View::new("ov", HashSet::from([h(5)]))];
    let views = load_views_with_overlay(dir.path(), &overlay, OverlayPrecedence::Right).unwrap();
    assert_eq!(views.len(), 1);
    assert_eq!(views[0].name, "ov");
}

#[test]
fn load_views_with_overlay_merges_persisted() {
    let dir = tempfile::tempdir().unwrap();
    let persisted = View::new("main", HashSet::from([h(1)]));
    persisted.save(dir.path()).unwrap();
    let overlay = vec![View::new("dev", HashSet::from([h(2)]))];
    let views = load_views_with_overlay(dir.path(), &overlay, OverlayPrecedence::Right).unwrap();
    assert_eq!(views.len(), 2);
}

#[test]
fn load_views_with_overlay_collision() {
    let dir = tempfile::tempdir().unwrap();
    let persisted = View::new("main", HashSet::from([h(1)]));
    persisted.save(dir.path()).unwrap();
    let overlay = vec![View::new("main", HashSet::from([h(99)]))];
    let views = load_views_with_overlay(dir.path(), &overlay, OverlayPrecedence::Right).unwrap();
    assert_eq!(views.len(), 1);
    assert!(views[0].heads.contains(&h(99)));
}
