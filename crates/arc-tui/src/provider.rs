use crate::model::ChangeEntry;

pub trait ChangeProvider {
    fn list_changes(&self) -> Vec<ChangeEntry>;
}

pub struct MockProvider;

impl ChangeProvider for MockProvider {
    fn list_changes(&self) -> Vec<ChangeEntry> {
        vec![
            ChangeEntry {
                id_short: "a1f03d9".to_string(),
                summary: "refactor semantic frontier traversal".to_string(),
                author: "alice@arc".to_string(),
                signature: "ed25519:verified".to_string(),
                hash: "3d6f6f3e0df6a5a4a5f0f4a6a0731ebf803f5c45b2278c4f12a0a7f57b6f0001".to_string(),
            },
            ChangeEntry {
                id_short: "b9c11aa".to_string(),
                summary: "tighten sync envelope validation".to_string(),
                author: "bob@arc".to_string(),
                signature: "ed25519:verified".to_string(),
                hash: "6c4a90de83fcb53c8e0da0f4df48df40226e2f5d7e6e5be67842fbe4b7cc0002".to_string(),
            },
            ChangeEntry {
                id_short: "c7e4ff0".to_string(),
                summary: "index snapshot metadata in redb".to_string(),
                author: "carol@arc".to_string(),
                signature: "ed25519:verified".to_string(),
                hash: "fce9a4d9c1fd9dd9c0d2d9f7f16a5b810f6a38a7c4a5c268fe6fabf76f730003".to_string(),
            },
            ChangeEntry {
                id_short: "d24aa31".to_string(),
                summary: "normalize renderer capability probing".to_string(),
                author: "dana@arc".to_string(),
                signature: "ed25519:verified".to_string(),
                hash: "91b3c59f452f20c5585f3390b63f95d3295bca0bd6ca7c376f8f0ab910260004".to_string(),
            },
            ChangeEntry {
                id_short: "e0fbc89".to_string(),
                summary: "stabilize bento event bridge".to_string(),
                author: "eve@arc".to_string(),
                signature: "ed25519:verified".to_string(),
                hash: "f8e718896ec5136ba0a179eea8d688161e68fcc57f6cecd7f0fcb7187d3e0005".to_string(),
            },
        ]
    }
}
