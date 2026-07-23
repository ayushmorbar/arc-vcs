use arc_store_types::newtypes::ChangeId;
use arc_transaction::*;
use std::collections::{BTreeMap, BTreeSet};

fn cid(byte: u8) -> ChangeId {
    ChangeId::from([byte; 32])
}

#[test]
fn new_restack_initial_state() {
    let heads = BTreeSet::from([cid(1), cid(2)]);
    let order = vec![cid(2), cid(1)];
    let pending = PendingRewrite::new_restack("main", heads.clone(), order.clone());

    assert_eq!(pending.version, CHECKPOINT_VERSION);
    assert_eq!(pending.command, "restack");
    assert_eq!(pending.view, "main");
    assert_eq!(pending.before_heads, heads);
    assert_eq!(pending.desired_order, order);
    assert!(pending.rewrite_map.is_empty());
    assert_eq!(pending.attempts, 0);
    assert_eq!(pending.status, RewriteStatus::InProgress);
}

#[test]
fn new_restack_into_string_conversion() {
    let pending = PendingRewrite::new_restack(String::from("feature/x"), BTreeSet::new(), vec![]);
    assert_eq!(pending.view, "feature/x");
}

#[test]
fn with_attempt_incremented_saturating() {
    let pending = PendingRewrite::new_restack("v", BTreeSet::new(), vec![])
        .with_attempt_incremented()
        .with_attempt_incremented()
        .with_attempt_incremented();
    assert_eq!(pending.attempts, 3);
}

#[test]
fn with_attempt_incremented_saturates_at_max() {
    let pending = PendingRewrite {
        attempts: u32::MAX,
        ..PendingRewrite::new_restack("v", BTreeSet::new(), vec![])
    }
    .with_attempt_incremented();
    assert_eq!(pending.attempts, u32::MAX);
}

#[test]
fn with_conflict_sets_status() {
    let pending =
        PendingRewrite::new_restack("v", BTreeSet::new(), vec![]).with_conflict("rebase failed");
    match &pending.status {
        RewriteStatus::Conflict { message } => assert_eq!(message, "rebase failed"),
        _ => panic!("expected Conflict status"),
    }
}

#[test]
fn clear_conflict_resets_to_in_progress() {
    let pending = PendingRewrite::new_restack("v", BTreeSet::new(), vec![])
        .with_conflict("oops")
        .clear_conflict();
    assert_eq!(pending.status, RewriteStatus::InProgress);
}

#[test]
fn resolved_order_no_rewrites() {
    let order = vec![cid(10), cid(20), cid(30)];
    let pending = PendingRewrite::new_restack("v", BTreeSet::new(), order.clone());
    assert_eq!(pending.resolved_order(), order);
}

#[test]
fn resolved_order_single_rewrite() {
    let pending = PendingRewrite {
        desired_order: vec![cid(1), cid(2)],
        rewrite_map: BTreeMap::from([(cid(1), cid(99))]),
        ..PendingRewrite::new_restack("v", BTreeSet::new(), vec![])
    };
    assert_eq!(pending.resolved_order(), vec![cid(99), cid(2)]);
}

#[test]
fn resolved_order_chained_rewrites() {
    let pending = PendingRewrite {
        desired_order: vec![cid(7), cid(8)],
        rewrite_map: BTreeMap::from([(cid(7), cid(9)), (cid(9), cid(10))]),
        ..PendingRewrite::new_restack("v", BTreeSet::new(), vec![])
    };
    assert_eq!(pending.resolved_order(), vec![cid(10), cid(8)]);
}

#[test]
fn resolved_order_cycle_detection() {
    let pending = PendingRewrite {
        desired_order: vec![cid(1)],
        rewrite_map: BTreeMap::from([(cid(1), cid(2)), (cid(2), cid(1))]),
        ..PendingRewrite::new_restack("v", BTreeSet::new(), vec![])
    };
    // Must not infinite-loop; should break and return the last valid id
    let resolved = pending.resolved_order();
    assert_eq!(resolved.len(), 1);
}

#[test]
fn resolved_order_self_rewrite() {
    let pending = PendingRewrite {
        desired_order: vec![cid(5)],
        rewrite_map: BTreeMap::from([(cid(5), cid(5))]),
        ..PendingRewrite::new_restack("v", BTreeSet::new(), vec![])
    };
    let resolved = pending.resolved_order();
    assert_eq!(resolved[0], cid(5));
}

#[test]
fn with_rewrite_map_merges() {
    let existing = BTreeMap::from([(cid(1), cid(10))]);
    let pending =
        PendingRewrite::new_restack("v", BTreeSet::new(), vec![]).with_rewrite_map(&existing);

    let more = BTreeMap::from([(cid(2), cid(20))]);
    let pending = pending.with_rewrite_map(&more);

    assert_eq!(pending.rewrite_map.len(), 2);
    assert_eq!(pending.rewrite_map[&cid(1)], cid(10));
    assert_eq!(pending.rewrite_map[&cid(2)], cid(20));
}

#[test]
fn with_rewrite_map_overwrites_existing() {
    let initial = BTreeMap::from([(cid(1), cid(10))]);
    let overwrite = BTreeMap::from([(cid(1), cid(99))]);
    let pending = PendingRewrite::new_restack("v", BTreeSet::new(), vec![])
        .with_rewrite_map(&initial)
        .with_rewrite_map(&overwrite);
    assert_eq!(pending.rewrite_map[&cid(1)], cid(99));
}

#[test]
fn bincode_roundtrip_pending_rewrite() {
    use bincode;

    let pending = PendingRewrite::new_restack("main", BTreeSet::from([cid(1)]), vec![cid(1)])
        .with_attempt_incremented()
        .with_rewrite_map(&BTreeMap::from([(cid(1), cid(2))]));

    let bytes = bincode::serialize(&pending).unwrap();
    let loaded: PendingRewrite = bincode::deserialize(&bytes).unwrap();
    assert_eq!(loaded, pending);
}

#[test]
fn bincode_roundtrip_conflict_status() {
    use bincode;

    let pending =
        PendingRewrite::new_restack("v", BTreeSet::new(), vec![]).with_conflict("network timeout");
    let bytes = bincode::serialize(&pending).unwrap();
    let loaded: PendingRewrite = bincode::deserialize(&bytes).unwrap();
    assert_eq!(loaded.status, pending.status);
}

#[test]
fn rewrite_status_serde_snake_case() {
    use bincode;

    let ip = bincode::serialize(&RewriteStatus::InProgress).unwrap();
    let decoded: RewriteStatus = bincode::deserialize(&ip).unwrap();
    assert_eq!(decoded, RewriteStatus::InProgress);

    let conflict = bincode::serialize(&RewriteStatus::Conflict { message: "x".into() }).unwrap();
    let decoded: RewriteStatus = bincode::deserialize(&conflict).unwrap();
    match decoded {
        RewriteStatus::Conflict { message } => assert_eq!(message, "x"),
        _ => panic!("expected Conflict"),
    }
}

#[test]
fn pending_rewrite_debug_format() {
    let pending = PendingRewrite::new_restack("v", BTreeSet::new(), vec![]);
    let dbg = format!("{pending:?}");
    assert!(dbg.contains("PendingRewrite"));
    assert!(dbg.contains("restack"));
}
