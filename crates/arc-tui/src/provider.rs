use std::collections::HashSet;

use arc_algebra_types::Atom;
use arc_change::Change;
use arc_store_types::author;

use crate::diff::generator::{DiffGenerator, InMemoryBlobStore};
use crate::model::ChangeEntry;

pub trait ChangeProvider {
    fn list_changes(&self) -> Vec<ChangeEntry>;
}

pub struct MockProvider;

impl ChangeProvider for MockProvider {
    fn list_changes(&self) -> Vec<ChangeEntry> {
        let mut store = InMemoryBlobStore::default();
        let insert_hash = store.insert_blob(b"fn new_feature() {\n    println!(\"hello\");\n}\n");
        let delete_hash = store.insert_blob(b"fn legacy() {\n    unreachable!();\n}\n");
        let modify_new_hash = store.insert_blob(b"let retries = 3;\n");

        let (author, signing_key) = author::test_keypair();
        let mock_change = Change::new(
            HashSet::new(),
            vec![
                Atom::Insert {
                    at: vec!["file".into(), "src/lib.rs".into(), "new_feature".into()],
                    content_hash: insert_hash,
                },
                Atom::Delete {
                    at: vec!["file".into(), "src/lib.rs".into(), "legacy".into()],
                    prior_hash: delete_hash,
                },
                Atom::Insert {
                    at: vec!["file".into(), "src/lib.rs".into(), "legacy".into()],
                    content_hash: modify_new_hash,
                },
            ],
            "mock semantic diff",
            author,
            &signing_key,
        );

        let mut generator = DiffGenerator::new(&store);
        let diff = generator.generate(&mock_change).ok();

        (0..5)
            .map(|idx| ChangeEntry {
                id_short: format!("mock{idx:03}"),
                summary: format!("semantic change #{idx}"),
                author: "mock@arc".to_string(),
                signature: "ed25519:verified".to_string(),
                hash: format!("{:064x}", idx + 1),
                change: mock_change.clone(),
                diff: diff.clone(),
            })
            .collect()
    }
}
