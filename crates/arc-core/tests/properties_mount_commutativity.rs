use std::collections::HashSet;

use arc_change::Change;
use arc_core::algebra::commute::commutes;
use arc_core::algebra::{Atom, SpacetimeCoordinate};
use arc_store_types::author;
use proptest::prelude::*;

fn coordinate() -> SpacetimeCoordinate {
    SpacetimeCoordinate {
        namespace: "org".to_string(),
        repo: "polyrepo".to_string(),
        hash: blake3::hash(b"mount-root"),
    }
}

fn mount_change(path_leaf: String) -> Change {
    let (author, signing_key) = author::test_keypair();
    Change::new(
        HashSet::new(),
        vec![Atom::Mount {
            path: vec!["file".to_string(), path_leaf],
            coordinate: coordinate(),
        }],
        "mount",
        author,
        &signing_key,
    )
}

fn insert_change(path_leaf: String) -> Change {
    let (author, signing_key) = author::test_keypair();
    Change::new(
        HashSet::new(),
        vec![Atom::Insert {
            at: vec!["file".to_string(), path_leaf],
            content_hash: [7u8; 32],
        }],
        "insert",
        author,
        &signing_key,
    )
}

fn rename_change(from_leaf: String, to_leaf: String) -> Change {
    let (author, signing_key) = author::test_keypair();
    Change::new(
        HashSet::new(),
        vec![Atom::Move {
            from: vec!["file".to_string(), from_leaf],
            to: vec!["file".to_string(), to_leaf],
        }],
        "rename",
        author,
        &signing_key,
    )
}

proptest! {
    #[test]
    fn properties_mount_insert_commute_on_disjoint_paths(
        mount_leaf in "[a-z]{3,12}",
        insert_leaf in "[a-z]{3,12}",
    ) {
        prop_assume!(mount_leaf != insert_leaf);
        let mount = mount_change(mount_leaf);
        let insert = insert_change(insert_leaf);
        prop_assert!(commutes(&mount, &insert));
        prop_assert!(commutes(&insert, &mount));
    }

    #[test]
    fn properties_mount_rename_commute_on_disjoint_paths(
        mount_leaf in "[a-z]{3,12}",
        rename_from in "[a-z]{3,12}",
        rename_to in "[a-z]{3,12}",
    ) {
        prop_assume!(mount_leaf != rename_from);
        prop_assume!(mount_leaf != rename_to);
        prop_assume!(rename_from != rename_to);

        let mount = mount_change(mount_leaf);
        let rename = rename_change(rename_from, rename_to);
        prop_assert!(commutes(&mount, &rename));
        prop_assert!(commutes(&rename, &mount));
    }
}
